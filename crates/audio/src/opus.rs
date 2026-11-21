//! Opus as Lumen streams sound: 48 kHz stereo, 10 ms frames, CELT only
//! (`RESTRICTED_LOWDELAY`: 2.5 ms of codec look-ahead), music at a fixed
//! bitrate. Inband FEC is not used: it needs SILK, which Opus avoids for music
//! at 128 kbit/s; losses are covered by sending each frame twice (see the
//! host) and by the decoder's concealment.

use std::ffi::{CStr, c_int};
use std::fmt;

use opus_sys as ffi;

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Frames (samples per channel) in one Opus frame: 10 ms.
pub const FRAME: usize = 480;
/// The largest Opus packet we accept or make, bytes.
pub const MAX_PACKET: usize = 1275;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusError(pub i32);

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = unsafe { CStr::from_ptr(ffi::opus_strerror(self.0)) };
        write!(f, "opus: {}", s.to_string_lossy())
    }
}

impl std::error::Error for OpusError {}

fn check(r: c_int) -> Result<c_int, OpusError> {
    if r < 0 { Err(OpusError(r)) } else { Ok(r) }
}

pub struct Encoder(*mut ffi::OpusEncoder);

// SAFETY: the state is only used through &mut self.
unsafe impl Send for Encoder {}

impl Encoder {
    /// An encoder at `bitrate`, bit/s.
    pub fn new(bitrate: i32) -> Result<Encoder, OpusError> {
        let mut err = 0;
        let st = unsafe {
            ffi::opus_encoder_create(
                RATE as i32,
                CHANNELS as c_int,
                ffi::OPUS_APPLICATION_RESTRICTED_LOWDELAY,
                &mut err,
            )
        };
        check(err)?;
        let e = Encoder(st);
        unsafe {
            check(ffi::opus_encoder_ctl(
                st,
                ffi::OPUS_SET_BITRATE_REQUEST,
                bitrate,
            ))?;
            check(ffi::opus_encoder_ctl(
                st,
                ffi::OPUS_SET_SIGNAL_REQUEST,
                ffi::OPUS_SIGNAL_MUSIC,
            ))?;
        }
        Ok(e)
    }

    /// Samples of codec delay (decoded sound lags the input by this much).
    pub fn lookahead(&mut self) -> usize {
        let mut v: i32 = 0;
        unsafe {
            ffi::opus_encoder_ctl(self.0, ffi::OPUS_GET_LOOKAHEAD_REQUEST, &mut v as *mut i32)
        };
        v.max(0) as usize
    }

    /// Encodes one frame (`FRAME` × `CHANNELS` samples, interleaved) into
    /// `out`; returns the packet's length.
    pub fn encode(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize, OpusError> {
        assert_eq!(pcm.len(), FRAME * CHANNELS, "one 10 ms stereo frame");
        let n = unsafe {
            ffi::opus_encode_float(
                self.0,
                pcm.as_ptr(),
                FRAME as c_int,
                out.as_mut_ptr(),
                out.len().min(MAX_PACKET) as i32,
            )
        };
        Ok(check(n)? as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_encoder_destroy(self.0) };
    }
}

pub struct Decoder(*mut ffi::OpusDecoder);

// SAFETY: the state is only used through &mut self.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Decoder, OpusError> {
        let mut err = 0;
        let st = unsafe { ffi::opus_decoder_create(RATE as i32, CHANNELS as c_int, &mut err) };
        check(err)?;
        Ok(Decoder(st))
    }

    /// Decodes one packet into `pcm` (room for `FRAME` × `CHANNELS`); `None`:
    /// the packet was lost, the decoder conceals it. Returns frames written.
    pub fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32]) -> Result<usize, OpusError> {
        assert!(pcm.len() >= FRAME * CHANNELS);
        let (data, len) = match packet {
            Some(p) => (p.as_ptr(), p.len() as i32),
            None => (std::ptr::null(), 0),
        };
        let n = unsafe {
            ffi::opus_decode_float(
                self.0,
                data,
                len,
                pcm.as_mut_ptr(),
                (pcm.len() / CHANNELS) as c_int,
                0,
            )
        };
        Ok(check(n)? as usize)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_decoder_destroy(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tone;

    /// The tone as stereo frames through the encoder; the packets.
    fn encode_tone(seconds: f64) -> (Vec<Vec<u8>>, usize) {
        let mono = tone::sine(RATE, 1000.0, 0.1, seconds);
        let mut e = Encoder::new(128_000).unwrap();
        let delay = e.lookahead();
        let mut packets = Vec::new();
        let mut out = [0u8; MAX_PACKET];
        for chunk in mono.as_chunks::<FRAME>().0 {
            let stereo: Vec<f32> = chunk.iter().flat_map(|&s| [s, s]).collect();
            let n = e.encode(&stereo, &mut out).unwrap();
            packets.push(out[..n].to_vec());
        }
        (packets, delay)
    }

    fn decode_left(packets: &[Option<Vec<u8>>]) -> Vec<f32> {
        let mut d = Decoder::new().unwrap();
        let mut pcm = [0f32; FRAME * CHANNELS];
        let mut left = Vec::new();
        for p in packets {
            let n = d.decode(p.as_deref(), &mut pcm).unwrap();
            assert_eq!(n, FRAME);
            left.extend(pcm[..n * CHANNELS].iter().step_by(CHANNELS));
        }
        left
    }

    #[test]
    fn a_tone_comes_through_whole() {
        let (packets, delay) = encode_tone(2.0);
        assert_eq!(delay, 120, "RESTRICTED_LOWDELAY: 2.5 ms");
        // 128 kbit/s × 10 ms = 160 bytes a packet, give or take (VBR).
        let avg = packets.iter().map(Vec::len).sum::<usize>() / packets.len();
        assert!((120..=200).contains(&avg), "{avg} bytes");
        let left = decode_left(&packets.into_iter().map(Some).collect::<Vec<_>>());
        let r = tone::analyse(&left, RATE, 1000.0);
        assert!(r.intact(), "{r:?}");
        assert!((r.amplitude - 0.1).abs() < 0.005, "{r:?}");
    }

    #[test]
    fn a_lost_packet_is_concealed() {
        let (packets, _) = encode_tone(2.0);
        let mut with_loss: Vec<Option<Vec<u8>>> = packets.into_iter().map(Some).collect();
        with_loss[100] = None;
        let left = decode_left(&with_loss);
        // The concealed 10 ms keep the tone going (no silent block).
        let r = tone::analyse(&left, RATE, 1000.0);
        assert_eq!(r.dropouts, 0, "{r:?}");
    }
}
