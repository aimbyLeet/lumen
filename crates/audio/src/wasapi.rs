//! WASAPI: output devices, loopback capture of the default one, and output to it.
//!
//! Loopback gives the mix Windows sends to the device, in the device's mix
//! format (usually 48 kHz float, stereo). While nothing plays, it may give no
//! packets at all; each packet carries its position in the device's stream,
//! so a gap shows as a jump in it.

use std::fmt;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, DEVICE_STATEMASK_ALL, IAudioCaptureClient,
    IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::core::{GUID, PWSTR};

use crate::framer::Packet;

#[derive(Debug)]
pub enum AudioError {
    Windows(windows::core::Error),
    /// The device's mix format is not one we read.
    Format(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Windows(e) => write!(f, "{e}"),
            AudioError::Format(s) => write!(f, "unsupported mix format: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<windows::core::Error> for AudioError {
    fn from(e: windows::core::Error) -> Self {
        AudioError::Windows(e)
    }
}

type Result<T> = std::result::Result<T, AudioError>;

/// COM on this thread for as long as it lives.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // S_FALSE (already on): still to be balanced; RPC_E_CHANGED_MODE: not ours.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
}

/// A COM-allocated string, copied and freed.
fn take_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn friendly_name(d: &IMMDevice) -> String {
    let name = || -> windows::core::Result<String> {
        let store = unsafe { d.OpenPropertyStore(STGM_READ)? };
        let v = unsafe { store.GetValue(&PKEY_Device_FriendlyName)? };
        Ok(take_string(unsafe { PropVariantToStringAlloc(&v)? }))
    };
    name().unwrap_or_default()
}

/// An output device, as Windows lists it.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    pub id: String,
    /// Plugged in and enabled (only these can be captured).
    pub active: bool,
    /// The default output device.
    pub default: bool,
}

/// All output devices (active or not).
pub fn render_endpoints() -> Result<Vec<Endpoint>> {
    let _com = Com::init();
    let e = enumerator()?;
    let default_id = unsafe { e.GetDefaultAudioEndpoint(eRender, eConsole) }
        .ok()
        .and_then(|d| unsafe { d.GetId() }.ok())
        .map(take_string);
    let all = unsafe {
        e.EnumAudioEndpoints(
            eRender,
            windows::Win32::Media::Audio::DEVICE_STATE(DEVICE_STATEMASK_ALL),
        )?
    };
    let mut out = Vec::new();
    for i in 0..unsafe { all.GetCount()? } {
        let d = unsafe { all.Item(i)? };
        let id = take_string(unsafe { d.GetId()? });
        out.push(Endpoint {
            name: friendly_name(&d),
            active: unsafe { d.GetState()? } == DEVICE_STATE_ACTIVE,
            default: default_id.as_deref() == Some(id.as_str()),
            id,
        });
    }
    Ok(out)
}

/// How samples come from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sample {
    F32,
    I16,
    I24,
    I32,
}

/// The device's mix format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
    sample: Sample,
}

impl Format {
    /// Interleaved stereo float at `rate` (a test source's).
    pub fn stereo_float(rate: u32) -> Format {
        Format {
            rate,
            channels: 2,
            sample: Sample::F32,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.sample {
            Sample::F32 => "float32",
            Sample::I16 => "int16",
            Sample::I24 => "int24",
            Sample::I32 => "int32",
        };
        write!(f, "{} Hz, {} ch, {s}", self.rate, self.channels)
    }
}

/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` and `_PCM` (the WAVE_FORMAT_EXTENSIBLE sub-formats).
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Reads a WAVEFORMATEX (or its extensible form) that `p` points to.
unsafe fn parse_format(p: *const WAVEFORMATEX) -> Result<Format> {
    // Packed structs: fields copied out, never referenced.
    let wf = unsafe { p.read_unaligned() };
    let tag = wf.wFormatTag;
    let (float, pcm) = match tag {
        WAVE_FORMAT_IEEE_FLOAT => (true, false),
        WAVE_FORMAT_PCM => (false, true),
        WAVE_FORMAT_EXTENSIBLE => {
            let ext = unsafe { (p as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
            let sub = ext.SubFormat;
            (sub == SUBTYPE_FLOAT, sub == SUBTYPE_PCM)
        }
        _ => (false, false),
    };
    let (rate, channels, bits) = (wf.nSamplesPerSec, wf.nChannels, wf.wBitsPerSample);
    let sample = match (float, pcm, bits) {
        (true, _, 32) => Sample::F32,
        (_, true, 16) => Sample::I16,
        (_, true, 24) => Sample::I24,
        (_, true, 32) => Sample::I32,
        _ => {
            return Err(AudioError::Format(format!("tag {tag:#x}, {bits} bit")));
        }
    };
    Ok(Format {
        rate,
        channels,
        sample,
    })
}

/// A shared-mode stream on the default output device: what loopback capture
/// and output have in common. Stopped when dropped.
struct Stream {
    client: IAudioClient,
    format: Format,
    device: String,
    /// The device's id: is it still the default one?
    id: String,
    // Last: COM goes after the interfaces.
    _com: Com,
}

fn default_device() -> Result<IMMDevice> {
    Ok(unsafe { enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)? })
}

impl Stream {
    /// Opens it in the device's mix format with `flags`, `buffer_100ns` of
    /// buffer, and starts it.
    fn open_default(flags: u32, buffer_100ns: i64) -> Result<Stream> {
        let com = Com::init();
        let device = default_device()?;
        let id = take_string(unsafe { device.GetId()? });
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let mix = unsafe { client.GetMixFormat()? };
        let format = unsafe { parse_format(mix) };
        let init = format.as_ref().ok().map(|_| unsafe {
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer_100ns, 0, mix, None)
        });
        unsafe { CoTaskMemFree(Some(mix as *const _)) };
        let format = format?;
        init.expect("format parsed")?;
        Ok(Stream {
            client,
            format,
            device: friendly_name(&device),
            id,
            _com: com,
        })
    }

    fn start(&self) -> Result<()> {
        unsafe { self.client.Start()? };
        Ok(())
    }

    /// The device is still the default output (headphones plugged in, the
    /// user chose another: use that one instead). Cannot tell: yes.
    fn is_default(&self) -> bool {
        default_device()
            .and_then(|d| Ok(take_string(unsafe { d.GetId()? })))
            .map_or(true, |id| id == self.id)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Loopback capture of the default output device.
pub struct Loopback {
    // First: released before the stream (and its COM).
    capture: IAudioCaptureClient,
    stream: Stream,
}

/// The capture buffer: room for this long between reads.
const BUFFER_100NS: i64 = 2_000_000;

impl Loopback {
    /// Starts capturing what the default output device plays.
    pub fn open_default() -> Result<Loopback> {
        let stream = Stream::open_default(AUDCLNT_STREAMFLAGS_LOOPBACK, BUFFER_100NS)?;
        let capture: IAudioCaptureClient = unsafe { stream.client.GetService()? };
        stream.start()?;
        Ok(Loopback { capture, stream })
    }

    pub fn format(&self) -> Format {
        self.stream.format
    }

    /// The device's name.
    pub fn device(&self) -> &str {
        &self.stream.device
    }

    /// The device is still the default output.
    pub fn is_default(&self) -> bool {
        self.stream.is_default()
    }

    /// Takes every packet that is ready: samples (interleaved, as float) to
    /// `out`, what each packet was to `packets`.
    pub fn read(&mut self, out: &mut Vec<f32>, packets: &mut Vec<Packet>) -> Result<()> {
        let format = self.stream.format;
        let ch = usize::from(format.channels);
        loop {
            if unsafe { self.capture.GetNextPacketSize()? } == 0 {
                return Ok(());
            }
            let mut data = std::ptr::null_mut();
            let (mut frames, mut flags, mut pos, mut qpc) = (0u32, 0u32, 0u64, 0u64);
            unsafe {
                self.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut pos),
                    Some(&mut qpc),
                )?
            };
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            let n = frames as usize * ch;
            if silent || data.is_null() {
                out.resize(out.len() + n, 0.0);
            } else {
                // SAFETY: the buffer holds `frames` frames of the mix format.
                unsafe { convert(data, n, format.sample, out) };
            }
            unsafe { self.capture.ReleaseBuffer(frames)? };
            packets.push(Packet {
                frames,
                device_position: pos,
                qpc_100ns: qpc,
                silent,
                discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
            });
        }
    }
}

/// The default output device, fed stereo float at its mix format's rate.
pub struct Render {
    // First: released before the stream (and its COM).
    render: IAudioRenderClient,
    stream: Stream,
    /// The device's buffer, frames.
    size: u32,
}

/// The output buffer asked for: room for the client's level and more.
const RENDER_BUFFER_100NS: i64 = 1_000_000;

impl Render {
    pub fn open_default() -> Result<Render> {
        let stream = Stream::open_default(0, RENDER_BUFFER_100NS)?;
        let size = unsafe { stream.client.GetBufferSize()? };
        let render: IAudioRenderClient = unsafe { stream.client.GetService()? };
        stream.start()?;
        Ok(Render {
            render,
            stream,
            size,
        })
    }

    pub fn format(&self) -> Format {
        self.stream.format
    }

    pub fn device(&self) -> &str {
        &self.stream.device
    }

    /// The device is still the default output.
    pub fn is_default(&self) -> bool {
        self.stream.is_default()
    }

    /// Frames written and not played yet.
    pub fn padding(&self) -> Result<u32> {
        Ok(unsafe { self.stream.client.GetCurrentPadding()? })
    }

    /// Room in the device's buffer, frames.
    pub fn room(&self) -> Result<u32> {
        Ok(self.size.saturating_sub(self.padding()?))
    }

    /// The engine's period, frames: it takes the sound in pieces this long
    /// (10 ms as a rule), so the buffer holds 0 right after a take.
    pub fn period_frames(&self) -> u32 {
        let mut period = 0i64;
        let _ = unsafe { self.stream.client.GetDevicePeriod(Some(&mut period), None) };
        let period = if period > 0 { period } else { 100_000 };
        (period as u64 * u64::from(self.stream.format.rate) / 10_000_000) as u32
    }

    /// The device's own latency after the buffer (the stream latency), µs.
    pub fn latency_us(&self) -> u64 {
        unsafe { self.stream.client.GetStreamLatency() }.map_or(0, |v| v.max(0) as u64 / 10)
    }

    /// Writes stereo frames (interleaved float) as far as there is room;
    /// returns how many. Other channels get silence.
    pub fn write(&mut self, stereo: &[f32]) -> Result<usize> {
        let frames = (stereo.len() / 2).min(self.room()? as usize);
        if frames == 0 {
            return Ok(0);
        }
        let format = self.stream.format;
        let data = unsafe { self.render.GetBuffer(frames as u32)? };
        let ch = usize::from(format.channels);
        // SAFETY: the buffer holds `frames` frames of the mix format.
        unsafe { write_frames(data, &stereo[..frames * 2], ch, format.sample) };
        unsafe { self.render.ReleaseBuffer(frames as u32, 0)? };
        Ok(frames)
    }
}

/// Stereo frames to a device buffer of `ch` channels in `sample` format
/// (left and right to the first two, silence to the rest; mono: their mean).
unsafe fn write_frames(data: *mut u8, stereo: &[f32], ch: usize, sample: Sample) {
    let frames = stereo.len() / 2;
    let value = |f: usize, c: usize| -> f32 {
        let (l, r) = (stereo[2 * f], stereo[2 * f + 1]);
        match (ch, c) {
            (1, _) => 0.5 * (l + r),
            (_, 0) => l,
            (_, 1) => r,
            _ => 0.0,
        }
        .clamp(-1.0, 1.0)
    };
    let n = frames * ch;
    match sample {
        Sample::F32 => {
            let out = unsafe { std::slice::from_raw_parts_mut(data as *mut f32, n) };
            for (i, o) in out.iter_mut().enumerate() {
                *o = value(i / ch, i % ch);
            }
        }
        Sample::I16 => {
            let out = unsafe { std::slice::from_raw_parts_mut(data as *mut i16, n) };
            for (i, o) in out.iter_mut().enumerate() {
                *o = (value(i / ch, i % ch) * 32767.0).round() as i16;
            }
        }
        Sample::I24 => {
            let out = unsafe { std::slice::from_raw_parts_mut(data, n * 3) };
            for (i, o) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let v = (value(i / ch, i % ch) * 8_388_607.0).round() as i32;
                o.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        Sample::I32 => {
            let out = unsafe { std::slice::from_raw_parts_mut(data as *mut i32, n) };
            for (i, o) in out.iter_mut().enumerate() {
                *o = (f64::from(value(i / ch, i % ch)) * 2_147_483_647.0).round() as i32;
            }
        }
    }
}

/// `n` samples at `data` in `sample` format, appended to `out` as float.
unsafe fn convert(data: *const u8, n: usize, sample: Sample, out: &mut Vec<f32>) {
    out.reserve(n);
    match sample {
        Sample::F32 => {
            let s = unsafe { std::slice::from_raw_parts(data as *const f32, n) };
            out.extend_from_slice(s);
        }
        Sample::I16 => {
            let s = unsafe { std::slice::from_raw_parts(data as *const i16, n) };
            out.extend(s.iter().map(|&v| f32::from(v) / 32768.0));
        }
        Sample::I24 => {
            let s = unsafe { std::slice::from_raw_parts(data, n * 3) };
            out.extend(s.as_chunks::<3>().0.iter().map(|b| {
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                v as f32 / 8_388_608.0
            }));
        }
        Sample::I32 => {
            let s = unsafe { std::slice::from_raw_parts(data as *const i32, n) };
            out.extend(s.iter().map(|&v| v as f32 / 2_147_483_648.0));
        }
    }
}

/// This thread feeds or reads a sound device: woken in time even when the
/// machine is busy (a late wake-up empties the device: an audible gap).
pub fn audio_thread_priority() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

/// The performance counter now, in 100 ns (as WASAPI gives packet times).
pub fn qpc_now_100ns() -> u64 {
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / i128::from(f.max(1))) as u64
}
