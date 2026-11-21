//! Video stream format and capability negotiation.
//!
//! Colour description uses ITU-T H.273 code points, so the values map 1:1 onto
//! HEVC VUI / AV1 sequence header fields.

use crate::DecodeError;
use crate::wire::{Reader, WriteExt, wire_enum};

wire_enum! {
    pub enum Codec {
        HevcMain10 = 1,
        Av1 = 2,
    }
}

wire_enum! {
    pub enum ChromaFormat {
        Yuv420 = 1,
        Yuv444 = 2,
    }
}

/// H.273 `colour_primaries` / `transfer_characteristics` / `matrix_coefficients`: BT.709.
pub const H273_BT709: u8 = 1;
/// HEVC `chroma_sample_loc_type` 0 ("left"); AV1 `CSP_VERTICAL`.
pub const CHROMA_LOC_LEFT: u8 = 0;

/// Everything the client needs to decode and display the stream exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    pub codec: Codec,
    pub chroma: ChromaFormat,
    pub bit_depth: u8,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
    pub chroma_loc: u8,
    pub width: u16,
    pub height: u16,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl VideoFormat {
    /// The fixed Lumen format: HEVC Main10, 4:2:0, BT.709 full range, left siting.
    pub fn lumen_default(width: u16, height: u16, fps: u32) -> Self {
        VideoFormat {
            codec: Codec::HevcMain10,
            chroma: ChromaFormat::Yuv420,
            bit_depth: 10,
            primaries: H273_BT709,
            transfer: H273_BT709,
            matrix: H273_BT709,
            full_range: true,
            chroma_loc: CHROMA_LOC_LEFT,
            width,
            height,
            fps_num: fps,
            fps_den: 1,
        }
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.put_u8(self.codec as u8);
        out.put_u8(self.chroma as u8);
        out.put_u8(self.bit_depth);
        out.put_u8(self.primaries);
        out.put_u8(self.transfer);
        out.put_u8(self.matrix);
        out.put_bool(self.full_range);
        out.put_u8(self.chroma_loc);
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.put_u32(self.fps_num);
        out.put_u32(self.fps_den);
    }

    pub(crate) fn decode(r: &mut Reader) -> Result<Self, DecodeError> {
        let f = VideoFormat {
            codec: Codec::try_from(r.u8()?)?,
            chroma: ChromaFormat::try_from(r.u8()?)?,
            bit_depth: r.u8()?,
            primaries: r.u8()?,
            transfer: r.u8()?,
            matrix: r.u8()?,
            full_range: r.bool()?,
            chroma_loc: r.u8()?,
            width: r.u16()?,
            height: r.u16()?,
            fps_num: r.u32()?,
            fps_den: r.u32()?,
        };
        if !matches!(f.bit_depth, 8 | 10) {
            return Err(DecodeError::InvalidValue("VideoFormat.bit_depth"));
        }
        if f.width == 0 || f.height == 0 {
            return Err(DecodeError::InvalidValue("VideoFormat.size"));
        }
        if f.fps_num == 0 || f.fps_den == 0 {
            return Err(DecodeError::InvalidValue("VideoFormat.fps"));
        }
        Ok(f)
    }
}

/// What a client can decode and show; sent in `Hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Bit set indexed by `Codec as u8`.
    pub codecs: u8,
    /// Bit set indexed by `ChromaFormat as u8`.
    pub chroma_formats: u8,
    pub max_bit_depth: u8,
    /// Client display size in pixels (informational: output is 1:1, never scaled).
    pub display_width: u16,
    pub display_height: u16,
}

impl Capabilities {
    pub fn supports_codec(&self, c: Codec) -> bool {
        self.codecs & (1 << c as u8) != 0
    }

    pub fn supports_chroma(&self, c: ChromaFormat) -> bool {
        self.chroma_formats & (1 << c as u8) != 0
    }

    pub fn with_codec(mut self, c: Codec) -> Self {
        self.codecs |= 1 << c as u8;
        self
    }

    pub fn with_chroma(mut self, c: ChromaFormat) -> Self {
        self.chroma_formats |= 1 << c as u8;
        self
    }

    /// Whether the client can take `format` as is.
    pub fn accepts(&self, format: &VideoFormat) -> bool {
        self.supports_codec(format.codec)
            && self.supports_chroma(format.chroma)
            && format.bit_depth <= self.max_bit_depth
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.put_u8(self.codecs);
        out.put_u8(self.chroma_formats);
        out.put_u8(self.max_bit_depth);
        out.put_u16(self.display_width);
        out.put_u16(self.display_height);
    }

    pub(crate) fn decode(r: &mut Reader) -> Result<Self, DecodeError> {
        Ok(Capabilities {
            codecs: r.u8()?,
            chroma_formats: r.u8()?,
            max_bit_depth: r.u8()?,
            display_width: r.u16()?,
            display_height: r.u16()?,
        })
    }
}
