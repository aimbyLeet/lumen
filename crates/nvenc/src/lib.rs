//! Thin, explicit wrapper over NVENC for the Lumen host.
//!
//! Scope of this first version:
//! * D3D11 input (zero-copy: the P010 texture produced by `gpu-convert`
//!   is registered once and mapped every frame);
//! * HEVC Main10 or AV1 Main, 4:2:0, 10 bit, BT.709 full range VUI;
//! * synchronous mode (one frame in flight) — simplest, lowest latency;
//! * rate control: constant QP (for quality studies), CBR, VBR, or constant
//!   quality under a bitrate cap (streaming);
//! * loss recovery by reference invalidation: frames the client lost are no
//!   longer used as references, so the next P-frame heals the stream without
//!   an IDR.
//!
//! The crate takes the D3D11 device / texture as raw COM pointers so that it
//! does not depend on a particular `windows` crate version.

pub use nvenc_sys as sys;
use std::ffi::{CStr, c_void};
use std::fmt;
use std::ptr::null_mut;

#[derive(Debug)]
pub struct NvencError {
    pub status: sys::NVENCSTATUS,
    pub call: &'static str,
    pub detail: String,
}

impl fmt::Display for NvencError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} failed: {} ({})",
            self.call,
            status_name(self.status),
            self.status
        )?;
        if !self.detail.is_empty() {
            write!(f, ": {}", self.detail)?;
        }
        Ok(())
    }
}
impl std::error::Error for NvencError {}

pub type Result<T> = std::result::Result<T, NvencError>;

fn status_name(s: sys::NVENCSTATUS) -> &'static str {
    match s {
        sys::NV_ENC_SUCCESS => "NV_ENC_SUCCESS",
        sys::NV_ENC_ERR_NO_ENCODE_DEVICE => "NV_ENC_ERR_NO_ENCODE_DEVICE",
        sys::NV_ENC_ERR_UNSUPPORTED_DEVICE => "NV_ENC_ERR_UNSUPPORTED_DEVICE",
        sys::NV_ENC_ERR_INVALID_ENCODERDEVICE => "NV_ENC_ERR_INVALID_ENCODERDEVICE",
        sys::NV_ENC_ERR_INVALID_DEVICE => "NV_ENC_ERR_INVALID_DEVICE",
        sys::NV_ENC_ERR_DEVICE_NOT_EXIST => "NV_ENC_ERR_DEVICE_NOT_EXIST",
        sys::NV_ENC_ERR_INVALID_PTR => "NV_ENC_ERR_INVALID_PTR",
        sys::NV_ENC_ERR_INVALID_PARAM => "NV_ENC_ERR_INVALID_PARAM",
        sys::NV_ENC_ERR_INVALID_CALL => "NV_ENC_ERR_INVALID_CALL",
        sys::NV_ENC_ERR_OUT_OF_MEMORY => "NV_ENC_ERR_OUT_OF_MEMORY",
        sys::NV_ENC_ERR_UNSUPPORTED_PARAM => "NV_ENC_ERR_UNSUPPORTED_PARAM",
        sys::NV_ENC_ERR_INVALID_VERSION => "NV_ENC_ERR_INVALID_VERSION",
        sys::NV_ENC_ERR_ENCODER_BUSY => "NV_ENC_ERR_ENCODER_BUSY",
        sys::NV_ENC_ERR_NEED_MORE_INPUT => "NV_ENC_ERR_NEED_MORE_INPUT",
        sys::NV_ENC_ERR_GENERIC => "NV_ENC_ERR_GENERIC",
        _ => "NVENC error",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Hevc,
    Av1,
}

#[derive(Clone, Copy, Debug)]
pub enum RateControl {
    /// Fixed quantiser for every frame. Best tool to study quality vs bitrate.
    ConstQp { qp: u32 },
    /// Constant bitrate with a one-frame VBV: classic low-latency streaming.
    Cbr { bitrate_bps: u32 },
    /// Variable bitrate, capped. Cheaper on static desktops than CBR.
    Vbr { avg_bps: u32, max_bps: u32 },
    /// Never finer than `min_qp` and never above `max_bps`, with a VBV of
    /// `vbv_frames` frames: the quality of `ConstQp { qp: min_qp }` while the
    /// picture fits under the cap, coarser only when it does not. (`targetQuality`
    /// would be the obvious knob, but Pascal NVENC ignores it under
    /// ULTRA_LOW_LATENCY.)
    CappedQuality {
        min_qp: u32,
        max_bps: u32,
        vbv_frames: u32,
    },
}

#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// NVENC preset 1 (fastest) ..= 7 (best quality).
    pub preset: u32,
    pub rate_control: RateControl,
    /// Decoded picture buffer size in frames (HEVC; 0 = driver default). It
    /// bounds how far back [`Encoder::invalidate_ref_frame`] can fall back
    /// before the encoder has to send an intra frame.
    pub ref_frames: u32,
    /// Per-block QP deltas (`NV_ENC_QP_MAP_DELTA`): one signed value per
    /// [`Encoder::qp_map_block`] block in raster order, filled through
    /// [`Encoder::qp_map_mut`] before each frame. Off: no map.
    pub qp_map: bool,
}

/// One encoded access unit.
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    pub size: usize,
    pub is_keyframe: bool,
    pub avg_qp: u32,
    pub timestamp: u64,
}

/// A D3D11 texture registered with the encoder.
#[derive(Clone, Copy, Debug)]
pub struct InputHandle(usize);

pub struct Encoder {
    _lib: sys::NvencLibrary,
    api: sys::NV_ENCODE_API_FUNCTION_LIST,
    enc: *mut c_void,
    bitstream: sys::NV_ENC_OUTPUT_PTR,
    inputs: Vec<sys::NV_ENC_REGISTERED_PTR>,
    cfg: EncoderConfig,
    frame_idx: u32,
    /// Parameters the session was initialised with (`encodeConfig` points into
    /// `config`), kept for `nvEncReconfigureEncoder`.
    init: sys::NV_ENC_INITIALIZE_PARAMS,
    config: Box<sys::NV_ENC_CONFIG>,
    /// The QP delta map handed to every frame (empty without `cfg.qp_map`).
    qp_map: Vec<i8>,
}

// The encoder session is used from one thread at a time; moving it is fine.
unsafe impl Send for Encoder {}

macro_rules! call {
    ($self:expr, $f:ident ( $($arg:expr),* $(,)? )) => {{
        let f = $self.api.$f.expect(concat!(stringify!($f), " missing"));
        let st = f($($arg),*);
        if st != sys::NV_ENC_SUCCESS {
            Err($self.error(st, stringify!($f)))
        } else {
            Ok(())
        }
    }};
}

impl Encoder {
    /// Opens an encode session on a D3D11 device and initialises it.
    ///
    /// # Safety
    /// `d3d11_device` must be a valid `ID3D11Device*` that outlives the encoder.
    pub unsafe fn new(d3d11_device: *mut c_void, cfg: EncoderConfig) -> Result<Self> {
        let err = |status, call, detail: String| NvencError {
            status,
            call,
            detail,
        };

        let lib = unsafe { sys::NvencLibrary::load() }.map_err(|e| {
            err(
                sys::NV_ENC_ERR_NO_ENCODE_DEVICE,
                "load nvEncodeAPI",
                e.to_string(),
            )
        })?;

        // Driver must support the API version we were compiled against.
        let mut max_ver = 0u32;
        unsafe { (lib.get_max_supported_version)(&mut max_ver) };
        let (drv_major, drv_minor) = (max_ver >> 4, max_ver & 0xF);
        let need = (sys::NVENCAPI_MAJOR_VERSION, sys::NVENCAPI_MINOR_VERSION);
        if (drv_major, drv_minor) < need {
            return Err(err(
                sys::NV_ENC_ERR_INVALID_VERSION,
                "version check",
                format!(
                    "driver supports NVENC API {drv_major}.{drv_minor}, need {}.{} — update the NVIDIA driver",
                    need.0, need.1
                ),
            ));
        }

        let mut api: sys::NV_ENCODE_API_FUNCTION_LIST = unsafe { std::mem::zeroed() };
        api.version = sys::NV_ENCODE_API_FUNCTION_LIST_VER;
        let st = unsafe { (lib.create_instance)(&mut api) };
        if st != sys::NV_ENC_SUCCESS {
            return Err(err(st, "NvEncodeAPICreateInstance", String::new()));
        }

        let mut open: sys::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = unsafe { std::mem::zeroed() };
        open.version = sys::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
        open.deviceType = sys::NV_ENC_DEVICE_TYPE_DIRECTX;
        open.device = d3d11_device;
        open.apiVersion = sys::NVENCAPI_VERSION;
        let mut enc = null_mut();
        let open_fn = api
            .nvEncOpenEncodeSessionEx
            .expect("nvEncOpenEncodeSessionEx");
        let st = unsafe { open_fn(&mut open, &mut enc) };
        if st != sys::NV_ENC_SUCCESS {
            return Err(err(st, "nvEncOpenEncodeSessionEx", String::new()));
        }

        let mut this = Self {
            _lib: lib,
            api,
            enc,
            bitstream: null_mut(),
            inputs: Vec::new(),
            cfg,
            frame_idx: 0,
            init: unsafe { std::mem::zeroed() },
            config: Box::new(unsafe { std::mem::zeroed() }),
            qp_map: Vec::new(),
        };
        unsafe { this.initialize()? };
        Ok(this)
    }

    fn error(&self, status: sys::NVENCSTATUS, call: &'static str) -> NvencError {
        let detail = match self.api.nvEncGetLastErrorString {
            Some(f) if !self.enc.is_null() => unsafe {
                let p = f(self.enc);
                if p.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            },
            _ => String::new(),
        };
        NvencError {
            status,
            call,
            detail,
        }
    }

    unsafe fn initialize(&mut self) -> Result<()> {
        let cfg = self.cfg.clone();
        let codec_guid = match cfg.codec {
            Codec::Hevc => sys::NV_ENC_CODEC_HEVC_GUID,
            Codec::Av1 => sys::NV_ENC_CODEC_AV1_GUID,
        };
        let preset_guid = match cfg.preset {
            1 => sys::NV_ENC_PRESET_P1_GUID,
            2 => sys::NV_ENC_PRESET_P2_GUID,
            3 => sys::NV_ENC_PRESET_P3_GUID,
            4 => sys::NV_ENC_PRESET_P4_GUID,
            5 => sys::NV_ENC_PRESET_P5_GUID,
            6 => sys::NV_ENC_PRESET_P6_GUID,
            _ => sys::NV_ENC_PRESET_P7_GUID,
        };
        let tuning = sys::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;

        // Start from the preset defaults, then override what matters.
        let mut preset: sys::NV_ENC_PRESET_CONFIG = unsafe { std::mem::zeroed() };
        preset.version = sys::NV_ENC_PRESET_CONFIG_VER;
        preset.presetCfg.version = sys::NV_ENC_CONFIG_VER;
        unsafe {
            call!(
                self,
                nvEncGetEncodePresetConfigEx(
                    self.enc,
                    codec_guid,
                    preset_guid,
                    tuning,
                    &mut preset
                )
            )?
        };
        let mut ec: sys::NV_ENC_CONFIG = preset.presetCfg;
        ec.version = sys::NV_ENC_CONFIG_VER;

        // Low latency: no B-frames, infinite GOP (keyframes only on demand).
        ec.gopLength = sys::NVENC_INFINITE_GOPLENGTH;
        ec.frameIntervalP = 1;

        let rc = &mut ec.rcParams;
        rc.version = sys::NV_ENC_RC_PARAMS_VER;
        rc.multiPass = sys::NV_ENC_MULTI_PASS_DISABLED;
        let frame_bits = |bps: u32| (bps as u64 / cfg.fps.max(1) as u64) as u32;
        match cfg.rate_control {
            RateControl::ConstQp { qp } => {
                rc.rateControlMode = sys::NV_ENC_PARAMS_RC_CONSTQP;
                rc.constQP = sys::NV_ENC_QP {
                    qpInterP: qp,
                    qpInterB: qp,
                    qpIntra: qp,
                };
            }
            RateControl::Cbr { bitrate_bps } => {
                rc.rateControlMode = sys::NV_ENC_PARAMS_RC_CBR;
                rc.averageBitRate = bitrate_bps;
                rc.maxBitRate = bitrate_bps;
                rc.vbvBufferSize = frame_bits(bitrate_bps);
                rc.vbvInitialDelay = rc.vbvBufferSize;
            }
            RateControl::Vbr { avg_bps, max_bps } => {
                rc.rateControlMode = sys::NV_ENC_PARAMS_RC_VBR;
                rc.averageBitRate = avg_bps;
                rc.maxBitRate = max_bps;
                rc.vbvBufferSize = frame_bits(max_bps);
                rc.vbvInitialDelay = rc.vbvBufferSize;
            }
            RateControl::CappedQuality {
                min_qp,
                max_bps,
                vbv_frames,
            } => {
                rc.rateControlMode = sys::NV_ENC_PARAMS_RC_VBR;
                rc.averageBitRate = max_bps;
                rc.maxBitRate = max_bps;
                rc.vbvBufferSize = frame_bits(max_bps) * vbv_frames.max(1);
                rc.vbvInitialDelay = rc.vbvBufferSize;
                rc.set_enableMinQP(1);
                rc.minQP = sys::NV_ENC_QP {
                    qpInterP: min_qp,
                    qpInterB: min_qp,
                    qpIntra: min_qp,
                };
            }
        }

        if cfg.qp_map {
            rc.qpMapMode = sys::NV_ENC_QP_MAP_DELTA;
        }

        match cfg.codec {
            Codec::Hevc => unsafe {
                ec.profileGUID = sys::NV_ENC_HEVC_PROFILE_MAIN10_GUID;
                let h = &mut ec.encodeCodecConfig.hevcConfig;
                h.set_chromaFormatIDC(1); // 4:2:0
                h.inputBitDepth = sys::NV_ENC_BIT_DEPTH_10;
                h.outputBitDepth = sys::NV_ENC_BIT_DEPTH_10;
                h.idrPeriod = sys::NVENC_INFINITE_GOPLENGTH;
                h.set_repeatSPSPPS(1);
                h.maxNumRefFramesInDPB = cfg.ref_frames;
                if cfg.qp_map {
                    // The map has one value per CTB: fix its size.
                    h.maxCUSize = sys::NV_ENC_HEVC_CUSIZE_32x32;
                }
                let vui = &mut h.hevcVUIParameters;
                vui.videoSignalTypePresentFlag = 1;
                vui.videoFormat = sys::NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
                vui.videoFullRangeFlag = 1;
                vui.colourDescriptionPresentFlag = 1;
                vui.colourPrimaries = sys::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                vui.transferCharacteristics = sys::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
                vui.colourMatrix = sys::NV_ENC_VUI_MATRIX_COEFFS_BT709;
                vui.chromaSampleLocationFlag = 1;
                vui.chromaSampleLocationTop = 0; // "left" siting, matches gpu-convert
                vui.chromaSampleLocationBot = 0;
            },
            Codec::Av1 => unsafe {
                ec.profileGUID = sys::NV_ENC_AV1_PROFILE_MAIN_GUID;
                let a = &mut ec.encodeCodecConfig.av1Config;
                a.set_chromaFormatIDC(1);
                a.inputBitDepth = sys::NV_ENC_BIT_DEPTH_10;
                a.outputBitDepth = sys::NV_ENC_BIT_DEPTH_10;
                a.idrPeriod = sys::NVENC_INFINITE_GOPLENGTH;
                a.set_repeatSeqHdr(1);
                a.colorPrimaries = sys::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
                a.transferCharacteristics = sys::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
                a.matrixCoefficients = sys::NV_ENC_VUI_MATRIX_COEFFS_BT709;
                a.colorRange = 1; // full
                // CSP_VERTICAL (1): horizontally co-sited with the left luma sample,
                // vertically between two rows — same siting as HEVC type 0 / gpu-convert.
                a.chromaSamplePosition = 1;
            },
        }

        let mut init: sys::NV_ENC_INITIALIZE_PARAMS = unsafe { std::mem::zeroed() };
        init.version = sys::NV_ENC_INITIALIZE_PARAMS_VER;
        init.encodeGUID = codec_guid;
        init.presetGUID = preset_guid;
        init.tuningInfo = tuning;
        init.encodeWidth = cfg.width;
        init.encodeHeight = cfg.height;
        init.darWidth = cfg.width;
        init.darHeight = cfg.height;
        init.maxEncodeWidth = cfg.width;
        init.maxEncodeHeight = cfg.height;
        init.frameRateNum = cfg.fps;
        init.frameRateDen = 1;
        init.enablePTD = 1;
        init.enableEncodeAsync = 0;
        init.bufferFormat = sys::NV_ENC_BUFFER_FORMAT_YUV420_10BIT;
        *self.config = ec;
        init.encodeConfig = &mut *self.config;
        unsafe { call!(self, nvEncInitializeEncoder(self.enc, &mut init))? };
        self.init = init;

        let mut bs: sys::NV_ENC_CREATE_BITSTREAM_BUFFER = unsafe { std::mem::zeroed() };
        bs.version = sys::NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        unsafe { call!(self, nvEncCreateBitstreamBuffer(self.enc, &mut bs))? };
        self.bitstream = bs.bitstreamBuffer;
        if cfg.qp_map {
            let (cols, rows) = self.qp_map_size();
            self.qp_map = vec![0; (cols * rows) as usize];
        }
        Ok(())
    }

    /// Side of one QP map block in pixels: the HEVC CTB (32, fixed when the
    /// map is on) or the AV1 superblock (64).
    pub fn qp_map_block(&self) -> u32 {
        match self.cfg.codec {
            Codec::Hevc => 32,
            Codec::Av1 => 64,
        }
    }

    /// Columns and rows of the QP map.
    pub fn qp_map_size(&self) -> (u32, u32) {
        let b = self.qp_map_block();
        (self.cfg.width.div_ceil(b), self.cfg.height.div_ceil(b))
    }

    /// The QP deltas the next frames are encoded with (raster order, see
    /// [`qp_map_size`](Self::qp_map_size)); `None` without `qp_map`.
    pub fn qp_map_mut(&mut self) -> Option<&mut [i8]> {
        self.cfg.qp_map.then_some(self.qp_map.as_mut_slice())
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    /// Queries one `NV_ENC_CAPS_*` value for the configured codec.
    pub fn cap(&self, cap: sys::NV_ENC_CAPS) -> Result<i32> {
        let guid = match self.cfg.codec {
            Codec::Hevc => sys::NV_ENC_CODEC_HEVC_GUID,
            Codec::Av1 => sys::NV_ENC_CODEC_AV1_GUID,
        };
        let mut param: sys::NV_ENC_CAPS_PARAM = unsafe { std::mem::zeroed() };
        param.version = sys::NV_ENC_CAPS_PARAM_VER;
        param.capsToQuery = cap;
        let mut value = 0i32;
        unsafe {
            call!(
                self,
                nvEncGetEncodeCaps(self.enc, guid, &mut param, &mut value)
            )?
        };
        Ok(value)
    }

    /// Changes the bitrate of a CBR / VBR / capped session in place (no IDR):
    /// average and maximum rate become `bps`; the VBV keeps its length in frames.
    pub fn set_bitrate(&mut self, bps: u32) -> Result<()> {
        let rc = &mut self.config.rcParams;
        rc.averageBitRate = bps;
        rc.maxBitRate = bps;
        let vbv_frames = match self.cfg.rate_control {
            RateControl::CappedQuality { vbv_frames, .. } => vbv_frames.max(1),
            _ => 1,
        };
        rc.vbvBufferSize = (u64::from(bps) / u64::from(self.cfg.fps.max(1))) as u32 * vbv_frames;
        rc.vbvInitialDelay = rc.vbvBufferSize;
        let mut params: sys::NV_ENC_RECONFIGURE_PARAMS = unsafe { std::mem::zeroed() };
        params.version = sys::NV_ENC_RECONFIGURE_PARAMS_VER;
        params.reInitEncodeParams = self.init;
        params.reInitEncodeParams.encodeConfig = &mut *self.config;
        unsafe { call!(self, nvEncReconfigureEncoder(self.enc, &mut params)) }
    }

    /// Marks the frame encoded with `timestamp` as lost: later frames will not
    /// reference it (nor frames that depend on it). If nothing valid is left in
    /// the DPB the next frame comes out intra.
    pub fn invalidate_ref_frame(&mut self, timestamp: u64) -> Result<()> {
        unsafe { call!(self, nvEncInvalidateRefFrames(self.enc, timestamp)) }
    }

    /// Registers a P010 `ID3D11Texture2D*` of the configured size.
    ///
    /// # Safety
    /// `texture` must be a valid texture created on the encoder's device and
    /// must outlive the encoder.
    pub unsafe fn register_d3d11_p010(&mut self, texture: *mut c_void) -> Result<InputHandle> {
        let mut reg: sys::NV_ENC_REGISTER_RESOURCE = unsafe { std::mem::zeroed() };
        reg.version = sys::NV_ENC_REGISTER_RESOURCE_VER;
        reg.resourceType = sys::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
        reg.width = self.cfg.width;
        reg.height = self.cfg.height;
        reg.resourceToRegister = texture;
        reg.bufferFormat = sys::NV_ENC_BUFFER_FORMAT_YUV420_10BIT;
        reg.bufferUsage = sys::NV_ENC_INPUT_IMAGE;
        unsafe { call!(self, nvEncRegisterResource(self.enc, &mut reg))? };
        self.inputs.push(reg.registeredResource);
        Ok(InputHandle(self.inputs.len() - 1))
    }

    /// Encodes one frame synchronously and appends the bitstream to `out`.
    pub fn encode(
        &mut self,
        input: InputHandle,
        timestamp: u64,
        force_idr: bool,
        out: &mut Vec<u8>,
    ) -> Result<EncodedFrame> {
        let registered = self.inputs[input.0];
        unsafe {
            let mut map: sys::NV_ENC_MAP_INPUT_RESOURCE = std::mem::zeroed();
            map.version = sys::NV_ENC_MAP_INPUT_RESOURCE_VER;
            map.registeredResource = registered;
            call!(self, nvEncMapInputResource(self.enc, &mut map))?;

            let mut pic: sys::NV_ENC_PIC_PARAMS = std::mem::zeroed();
            pic.version = sys::NV_ENC_PIC_PARAMS_VER;
            pic.inputWidth = self.cfg.width;
            pic.inputHeight = self.cfg.height;
            pic.inputBuffer = map.mappedResource;
            pic.bufferFmt = map.mappedBufferFmt;
            pic.outputBitstream = self.bitstream;
            pic.pictureStruct = sys::NV_ENC_PIC_STRUCT_FRAME;
            pic.inputTimeStamp = timestamp;
            pic.frameIdx = self.frame_idx;
            if self.cfg.qp_map {
                pic.qpDeltaMap = self.qp_map.as_mut_ptr();
                pic.qpDeltaMapSize = self.qp_map.len() as u32;
            }
            if force_idr {
                pic.encodePicFlags =
                    sys::NV_ENC_PIC_FLAG_FORCEIDR | sys::NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
            }
            self.frame_idx = self.frame_idx.wrapping_add(1);
            let enc_res = call!(self, nvEncEncodePicture(self.enc, &mut pic));

            let result = enc_res.and_then(|_| {
                let mut lock: sys::NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
                lock.version = sys::NV_ENC_LOCK_BITSTREAM_VER;
                lock.outputBitstream = self.bitstream;
                call!(self, nvEncLockBitstream(self.enc, &mut lock))?;
                let size = lock.bitstreamSizeInBytes as usize;
                let data = std::slice::from_raw_parts(lock.bitstreamBufferPtr as *const u8, size);
                out.extend_from_slice(data);
                let frame = EncodedFrame {
                    size,
                    is_keyframe: lock.pictureType == sys::NV_ENC_PIC_TYPE_IDR
                        || lock.pictureType == sys::NV_ENC_PIC_TYPE_I,
                    avg_qp: lock.frameAvgQP,
                    timestamp: lock.outputTimeStamp,
                };
                call!(self, nvEncUnlockBitstream(self.enc, self.bitstream))?;
                Ok(frame)
            });

            let unmap = call!(self, nvEncUnmapInputResource(self.enc, map.mappedResource));
            let frame = result?;
            unmap?;
            Ok(frame)
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if self.enc.is_null() {
                return;
            }
            // Flush: an EOS picture with no input.
            if let Some(f) = self.api.nvEncEncodePicture {
                let mut pic: sys::NV_ENC_PIC_PARAMS = std::mem::zeroed();
                pic.version = sys::NV_ENC_PIC_PARAMS_VER;
                pic.encodePicFlags = sys::NV_ENC_PIC_FLAG_EOS;
                f(self.enc, &mut pic);
            }
            if let Some(f) = self.api.nvEncUnregisterResource {
                for r in self.inputs.drain(..) {
                    f(self.enc, r);
                }
            }
            if let (Some(f), false) = (
                self.api.nvEncDestroyBitstreamBuffer,
                self.bitstream.is_null(),
            ) {
                f(self.enc, self.bitstream);
            }
            if let Some(f) = self.api.nvEncDestroyEncoder {
                f(self.enc);
            }
        }
    }
}

/// Minimal IVF container writer (AV1 elementary streams are OBUs and need one;
/// HEVC is written as raw Annex B and needs nothing).
pub struct IvfWriter<W: std::io::Write> {
    w: W,
    frames: u32,
}

impl<W: std::io::Write> IvfWriter<W> {
    pub fn new(mut w: W, width: u16, height: u16, fps: u32) -> std::io::Result<Self> {
        let mut h = Vec::with_capacity(32);
        h.extend_from_slice(b"DKIF");
        h.extend_from_slice(&0u16.to_le_bytes()); // version
        h.extend_from_slice(&32u16.to_le_bytes()); // header size
        h.extend_from_slice(b"AV01");
        h.extend_from_slice(&width.to_le_bytes());
        h.extend_from_slice(&height.to_le_bytes());
        h.extend_from_slice(&fps.to_le_bytes()); // timebase denominator
        h.extend_from_slice(&1u32.to_le_bytes()); // timebase numerator
        h.extend_from_slice(&0u32.to_le_bytes()); // frame count (patched never; readers ignore)
        h.extend_from_slice(&0u32.to_le_bytes()); // unused
        w.write_all(&h)?;
        Ok(Self { w, frames: 0 })
    }

    pub fn write_frame(&mut self, data: &[u8], pts: u64) -> std::io::Result<()> {
        self.w.write_all(&(data.len() as u32).to_le_bytes())?;
        self.w.write_all(&pts.to_le_bytes())?;
        self.w.write_all(data)?;
        self.frames += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ivf_header_is_32_bytes() {
        let mut buf = Vec::new();
        let mut w = IvfWriter::new(&mut buf, 1920, 1080, 60).unwrap();
        w.write_frame(&[1, 2, 3], 0).unwrap();
        assert_eq!(&buf[..4], b"DKIF");
        assert_eq!(buf.len(), 32 + 12 + 3);
    }
}
