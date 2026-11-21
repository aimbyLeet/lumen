use std::collections::VecDeque;
use std::ffi::{CStr, c_int, c_void};
use std::ptr::null_mut;
use std::rc::Rc;

use anyhow::{Context, Result, anyhow, bail, ensure};
use gpu_convert::texture_desc;
use nvdec_sys::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::Interface;

/// Output textures kept in flight; a frame stays valid until this many newer
/// frames have been decoded. 4: with decode and display on different threads
/// the display may hold one frame while three newer ones are decoded.
pub const OUTPUT_RING: usize = 4;

/// A decoded picture in D3D11 memory, 10-bit codes in the high bits (P010/P016 layout).
#[derive(Clone)]
pub struct Frame {
    /// `R16_UNORM`, `width` × `height`.
    pub luma: ID3D11Texture2D,
    /// `R16G16_UNORM` (Cb, Cr), `width / 2` × `height / 2`.
    pub chroma: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    /// Timestamp passed to [`Decoder::decode`] with this picture's access unit.
    pub timestamp: i64,
}

struct Api {
    cuda: CudaLibrary,
    cuvid: CuvidLibrary,
}

impl Api {
    fn check(&self, r: CUresult, what: &str) -> Result<()> {
        if r == CUDA_SUCCESS {
            return Ok(());
        }
        let mut name = std::ptr::null();
        let name = unsafe {
            if (self.cuda.get_error_name)(r, &mut name) == CUDA_SUCCESS && !name.is_null() {
                CStr::from_ptr(name).to_string_lossy().into_owned()
            } else {
                "unknown".into()
            }
        };
        Err(anyhow!("{what} failed: {name} ({r})"))
    }
}

struct Slot {
    luma: ID3D11Texture2D,
    chroma: ID3D11Texture2D,
    res: [CUgraphicsResource; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SeqFormat {
    coded_width: u32,
    coded_height: u32,
    width: u32,
    height: u32,
    display: [i32; 4],
    min_surfaces: u32,
}

/// State reachable from the parser callbacks; boxed so its address is stable.
struct Inner {
    gpu: Rc<Gpu>,
    device: ID3D11Device,
    lock: CUvideoctxlock,
    stream: CUstream,
    decoder: CUvideodecoder,
    format: Option<SeqFormat>,
    slots: Vec<Slot>,
    next_slot: usize,
    ready: VecDeque<Frame>,
    error: Option<anyhow::Error>,
}

/// A CUDA context on the GPU of a D3D11 device. Creating one takes ~60 ms
/// (the bulk of a [`Decoder::new`]): the decoders of a session share one.
/// Like the decoders, it stays on the thread that made it.
pub struct Gpu {
    api: Api,
    ctx: CUcontext,
}

impl Gpu {
    pub fn new(device: &ID3D11Device) -> Result<Rc<Self>> {
        unsafe {
            let api = Api {
                cuda: CudaLibrary::load().context("load nvcuda.dll")?,
                cuvid: CuvidLibrary::load().context("load nvcuvid.dll")?,
            };
            api.check((api.cuda.init)(0), "cuInit")?;
            let adapter = device.cast::<IDXGIDevice>()?.GetAdapter()?;
            let mut cu_dev: CUdevice = 0;
            api.check(
                (api.cuda.d3d11_get_device)(&mut cu_dev, adapter.as_raw()),
                "cuD3D11GetDevice",
            )?;
            let mut ctx = null_mut();
            api.check(
                (api.cuda.ctx_create)(&mut ctx, CU_CTX_SCHED_BLOCKING_SYNC, cu_dev),
                "cuCtxCreate",
            )?;
            // cuCtxCreate leaves the context current; keep it current only while working.
            let mut popped = null_mut();
            (api.cuda.ctx_pop_current)(&mut popped);
            Ok(Rc::new(Gpu { api, ctx }))
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            (self.api.cuda.ctx_destroy)(self.ctx);
        }
    }
}

/// HEVC Main10 decoder on NVDEC; output lands in D3D11 textures without CPU copies.
pub struct Decoder {
    parser: CUvideoparser,
    inner: Box<Inner>,
}

impl Decoder {
    /// Creates a decoder on the GPU that owns `device`, with a CUDA context
    /// of its own (creating one takes ~60 ms: decoders that live together
    /// share one, [`Decoder::on`]).
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Self::on(&Gpu::new(device)?, device)
    }

    /// Creates a decoder in `gpu`'s CUDA context (a few ms); `device` must be
    /// the one `gpu` was made for.
    pub fn on(gpu: &Rc<Gpu>, device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let mut inner = Box::new(Inner {
                gpu: gpu.clone(),
                device: device.clone(),
                lock: null_mut(),
                stream: null_mut(),
                decoder: null_mut(),
                format: None,
                slots: Vec::new(),
                next_slot: 0,
                ready: VecDeque::new(),
                error: None,
            });
            let (api, ctx) = (&gpu.api, gpu.ctx);
            api.check(
                (api.cuvid.ctx_lock_create)(&mut inner.lock, ctx),
                "cuvidCtxLockCreate",
            )?;
            api.check((api.cuda.ctx_push_current)(ctx), "cuCtxPushCurrent")?;
            let mut popped = null_mut();
            let r = (api.cuda.stream_create)(&mut inner.stream, CU_STREAM_NON_BLOCKING);
            (api.cuda.ctx_pop_current)(&mut popped);
            api.check(r, "cuStreamCreate")?;

            let mut pp: CUVIDPARSERPARAMS = std::mem::zeroed();
            pp.CodecType = cudaVideoCodec_HEVC;
            pp.ulMaxNumDecodeSurfaces = 1; // raised by the sequence callback
            pp.ulMaxDisplayDelay = 0; // low latency: display as soon as decoded
            pp.pUserData = &mut *inner as *mut Inner as *mut c_void;
            pp.pfnSequenceCallback = Some(on_sequence);
            pp.pfnDecodePicture = Some(on_decode);
            pp.pfnDisplayPicture = Some(on_display);
            let mut parser = null_mut();
            inner.gpu.api.check(
                (inner.gpu.api.cuvid.create_video_parser)(&mut parser, &mut pp),
                "cuvidCreateVideoParser",
            )?;
            Ok(Decoder { parser, inner })
        }
    }

    /// Name of the CUDA device the decoder runs on.
    pub fn device_name(&self) -> String {
        let mut buf = [0i8; 256];
        unsafe {
            let mut dev: CUdevice = 0;
            let adapter = match self
                .inner
                .device
                .cast::<IDXGIDevice>()
                .and_then(|d| d.GetAdapter())
            {
                Ok(a) => a,
                Err(_) => return String::new(),
            };
            (self.inner.gpu.api.cuda.d3d11_get_device)(&mut dev, adapter.as_raw());
            (self.inner.gpu.api.cuda.device_get_name)(buf.as_mut_ptr(), buf.len() as c_int, dev);
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        }
    }

    /// Decodes one complete access unit. With display delay 0, its picture is
    /// normally available from [`next_frame`](Self::next_frame) right after this returns.
    pub fn decode(&mut self, access_unit: &[u8], timestamp: i64) -> Result<()> {
        let mut pkt: CUVIDSOURCEDATAPACKET = unsafe { std::mem::zeroed() };
        pkt.flags = (CUVID_PKT_ENDOFPICTURE | CUVID_PKT_TIMESTAMP) as _;
        pkt.payload_size = access_unit.len() as _;
        pkt.payload = access_unit.as_ptr();
        pkt.timestamp = timestamp;
        self.parse(&mut pkt)
    }

    /// Signals end of stream; flushes any pictures still held by the parser.
    pub fn flush(&mut self) -> Result<()> {
        let mut pkt: CUVIDSOURCEDATAPACKET = unsafe { std::mem::zeroed() };
        pkt.flags = CUVID_PKT_ENDOFSTREAM as _;
        self.parse(&mut pkt)
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        self.inner.ready.pop_front()
    }

    fn parse(&mut self, pkt: &mut CUVIDSOURCEDATAPACKET) -> Result<()> {
        let inner = &mut *self.inner;
        unsafe {
            inner.gpu.api.check(
                (inner.gpu.api.cuda.ctx_push_current)(inner.gpu.ctx),
                "cuCtxPushCurrent",
            )?;
            let r = (inner.gpu.api.cuvid.parse_video_data)(self.parser, pkt);
            let mut popped = null_mut();
            (inner.gpu.api.cuda.ctx_pop_current)(&mut popped);
            if let Some(e) = inner.error.take() {
                return Err(e);
            }
            inner.gpu.api.check(r, "cuvidParseVideoData")
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let inner = &mut *self.inner;
            let gpu = inner.gpu.clone();
            let api = &gpu.api;
            (api.cuda.ctx_push_current)(gpu.ctx);
            if !self.parser.is_null() {
                (api.cuvid.destroy_video_parser)(self.parser);
            }
            inner.release_decoder();
            if !inner.stream.is_null() {
                (api.cuda.stream_destroy)(inner.stream);
            }
            let mut popped = null_mut();
            (api.cuda.ctx_pop_current)(&mut popped);
            if !inner.lock.is_null() {
                (api.cuvid.ctx_lock_destroy)(inner.lock);
            }
        }
    }
}

impl Inner {
    /// Destroys the decoder and output textures. Context must be current.
    unsafe fn release_decoder(&mut self) {
        unsafe {
            for s in self.slots.drain(..) {
                for r in s.res {
                    (self.gpu.api.cuda.graphics_unregister_resource)(r);
                }
            }
            if !self.decoder.is_null() {
                (self.gpu.api.cuvid.destroy_decoder)(self.decoder);
                self.decoder = null_mut();
            }
            self.format = None;
            self.ready.clear();
        }
    }

    unsafe fn sequence(&mut self, fmt: &CUVIDEOFORMAT) -> Result<u32> {
        ensure!(
            fmt.codec == cudaVideoCodec_HEVC,
            "codec {} is not HEVC",
            fmt.codec
        );
        ensure!(
            fmt.chroma_format == cudaVideoChromaFormat_420,
            "chroma format {} is not 4:2:0",
            fmt.chroma_format
        );
        ensure!(
            fmt.bit_depth_luma_minus8 == 2 && fmt.bit_depth_chroma_minus8 == 2,
            "stream is not 10-bit"
        );
        let d = fmt.display_area;
        let seq = SeqFormat {
            coded_width: fmt.coded_width,
            coded_height: fmt.coded_height,
            width: (d.right - d.left) as u32,
            height: (d.bottom - d.top) as u32,
            display: [d.left, d.top, d.right, d.bottom],
            min_surfaces: fmt.min_num_decode_surfaces as u32,
        };
        if self.format == Some(seq) {
            return Ok(seq.min_surfaces);
        }
        ensure!(
            seq.width.is_multiple_of(2) && seq.height.is_multiple_of(2),
            "odd picture size {}x{}",
            seq.width,
            seq.height
        );
        unsafe {
            self.release_decoder();

            let mut caps: CUVIDDECODECAPS = std::mem::zeroed();
            caps.eCodecType = cudaVideoCodec_HEVC;
            caps.eChromaFormat = cudaVideoChromaFormat_420;
            caps.nBitDepthMinus8 = 2;
            self.gpu.api.check(
                (self.gpu.api.cuvid.get_decoder_caps)(&mut caps),
                "cuvidGetDecoderCaps",
            )?;
            ensure!(caps.bIsSupported != 0, "GPU cannot decode HEVC Main10");
            ensure!(
                seq.coded_width <= caps.nMaxWidth && seq.coded_height <= caps.nMaxHeight,
                "{}x{} exceeds decoder limit {}x{}",
                seq.coded_width,
                seq.coded_height,
                caps.nMaxWidth,
                caps.nMaxHeight
            );
            ensure!(
                caps.nOutputFormatMask & (1 << cudaVideoSurfaceFormat_P016) != 0,
                "decoder cannot output P016"
            );

            let mut ci: CUVIDDECODECREATEINFO = std::mem::zeroed();
            ci.ulWidth = seq.coded_width as _;
            ci.ulHeight = seq.coded_height as _;
            ci.ulNumDecodeSurfaces = seq.min_surfaces as _;
            ci.CodecType = cudaVideoCodec_HEVC;
            ci.ChromaFormat = cudaVideoChromaFormat_420;
            ci.ulCreationFlags = cudaVideoCreate_PreferCUVID as _;
            ci.bitDepthMinus8 = 2;
            ci.ulMaxWidth = seq.coded_width as _;
            ci.ulMaxHeight = seq.coded_height as _;
            ci.display_area.left = d.left as i16;
            ci.display_area.top = d.top as i16;
            ci.display_area.right = d.right as i16;
            ci.display_area.bottom = d.bottom as i16;
            ci.OutputFormat = cudaVideoSurfaceFormat_P016;
            ci.DeinterlaceMode = cudaVideoDeinterlaceMode_Weave;
            ci.ulTargetWidth = seq.width as _;
            ci.ulTargetHeight = seq.height as _;
            ci.ulNumOutputSurfaces = 2;
            ci.vidLock = self.lock;
            let mut dec = null_mut();
            self.gpu.api.check(
                (self.gpu.api.cuvid.create_decoder)(&mut dec, &mut ci),
                "cuvidCreateDecoder",
            )?;
            self.decoder = dec;

            for _ in 0..OUTPUT_RING {
                let slot = self.create_slot(seq.width, seq.height)?;
                self.slots.push(slot);
            }
            self.next_slot = 0;
            self.format = Some(seq);
        }
        Ok(seq.min_surfaces)
    }

    unsafe fn create_slot(&self, width: u32, height: u32) -> Result<Slot> {
        let tex = |w: u32, h: u32, fmt: DXGI_FORMAT| -> Result<ID3D11Texture2D> {
            let td = texture_desc(w, h, fmt, D3D11_BIND_SHADER_RESOURCE);
            let mut t = None;
            unsafe { self.device.CreateTexture2D(&td, None, Some(&mut t)) }
                .context("create output texture")?;
            t.context("output texture")
        };
        let luma = tex(width, height, DXGI_FORMAT_R16_UNORM)?;
        let chroma = tex(width / 2, height / 2, DXGI_FORMAT_R16G16_UNORM)?;
        let mut res = [null_mut(); 2];
        for (r, t) in res.iter_mut().zip([&luma, &chroma]) {
            self.gpu.api.check(
                unsafe {
                    (self.gpu.api.cuda.graphics_d3d11_register_resource)(
                        r,
                        t.as_raw(),
                        CU_GRAPHICS_REGISTER_FLAGS_NONE as u32,
                    )
                },
                "cuGraphicsD3D11RegisterResource",
            )?;
        }
        Ok(Slot { luma, chroma, res })
    }

    unsafe fn display(&mut self, info: &CUVIDPARSERDISPINFO) -> Result<()> {
        let seq = self.format.context("picture before sequence header")?;
        let api = &self.gpu.api;
        unsafe {
            let mut vpp: CUVIDPROCPARAMS = std::mem::zeroed();
            vpp.progressive_frame = info.progressive_frame;
            vpp.top_field_first = info.top_field_first;
            vpp.output_stream = self.stream;
            let mut src: u64 = 0;
            let mut pitch: u32 = 0;
            api.check(
                (api.cuvid.map_video_frame)(
                    self.decoder,
                    info.picture_index,
                    &mut src,
                    &mut pitch,
                    &mut vpp,
                ),
                "cuvidMapVideoFrame",
            )?;

            let slot = &mut self.slots[self.next_slot];
            let copy = (|| -> Result<()> {
                api.check(
                    (api.cuda.graphics_map_resources)(2, slot.res.as_mut_ptr(), self.stream),
                    "cuGraphicsMapResources",
                )?;
                let mut result = Ok(());
                // Luma rows, then the interleaved CbCr plane below them.
                let planes = [(0, seq.height as usize), (1, seq.height as usize / 2)];
                for (i, rows) in planes {
                    let mut array = null_mut();
                    result = api.check(
                        (api.cuda.graphics_sub_resource_get_mapped_array)(
                            &mut array,
                            slot.res[i],
                            0,
                            0,
                        ),
                        "cuGraphicsSubResourceGetMappedArray",
                    );
                    if result.is_err() {
                        break;
                    }
                    let mut m: CUDA_MEMCPY2D = std::mem::zeroed();
                    m.srcMemoryType = CU_MEMORYTYPE_DEVICE;
                    m.srcDevice = src + (i * pitch as usize * seq.height as usize) as u64;
                    m.srcPitch = pitch as usize;
                    m.dstMemoryType = CU_MEMORYTYPE_ARRAY;
                    m.dstArray = array;
                    m.WidthInBytes = seq.width as usize * 2;
                    m.Height = rows;
                    result = api.check(
                        (api.cuda.memcpy_2d_async)(&m, self.stream),
                        "cuMemcpy2DAsync",
                    );
                    if result.is_err() {
                        break;
                    }
                }
                let unmap = api.check(
                    (api.cuda.graphics_unmap_resources)(2, slot.res.as_mut_ptr(), self.stream),
                    "cuGraphicsUnmapResources",
                );
                result?;
                unmap?;
                api.check(
                    (api.cuda.stream_synchronize)(self.stream),
                    "cuStreamSynchronize",
                )
            })();
            let unmap = api.check(
                (api.cuvid.unmap_video_frame)(self.decoder, src),
                "cuvidUnmapVideoFrame",
            );
            copy?;
            unmap?;

            self.ready.push_back(Frame {
                luma: slot.luma.clone(),
                chroma: slot.chroma.clone(),
                width: seq.width,
                height: seq.height,
                timestamp: info.timestamp,
            });
            self.next_slot = (self.next_slot + 1) % self.slots.len();
        }
        Ok(())
    }
}

/// Parser callbacks: errors are parked in `Inner::error` and reported by `parse`.
unsafe fn with_inner(user: *mut c_void, f: impl FnOnce(&mut Inner) -> Result<c_int>) -> c_int {
    let inner = unsafe { &mut *(user as *mut Inner) };
    if inner.error.is_some() {
        return 0;
    }
    match f(inner) {
        Ok(v) => v,
        Err(e) => {
            inner.error = Some(e);
            0
        }
    }
}

unsafe extern "C" fn on_sequence(user: *mut c_void, fmt: *mut CUVIDEOFORMAT) -> c_int {
    unsafe { with_inner(user, |s| Ok(s.sequence(&*fmt)? as c_int)) }
}

unsafe extern "C" fn on_decode(user: *mut c_void, pic: *mut CUVIDPICPARAMS) -> c_int {
    unsafe {
        with_inner(user, |s| {
            if s.decoder.is_null() {
                bail!("picture before sequence header");
            }
            s.gpu.api.check(
                (s.gpu.api.cuvid.decode_picture)(s.decoder, pic),
                "cuvidDecodePicture",
            )?;
            Ok(1)
        })
    }
}

unsafe extern "C" fn on_display(user: *mut c_void, info: *mut CUVIDPARSERDISPINFO) -> c_int {
    unsafe {
        with_inner(user, |s| {
            if !info.is_null() {
                s.display(&*info)?;
            }
            Ok(1)
        })
    }
}
