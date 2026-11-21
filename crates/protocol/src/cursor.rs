//! Cursor shape, as handed out by Desktop Duplication.
//!
//! The shape is too large for one datagram, so it travels fragmented
//! ([`PacketKind::CursorShape`](crate::PacketKind)); this is the reassembled payload.
//!
//! Layout (little-endian):
//! ```text
//!  0  id       u32  changes whenever the shape changes
//!  4  kind     u8   CursorKind
//!  5  width    u16
//!  7  height   u16  for Monochrome: twice the visible height (AND mask, then XOR mask)
//!  9  pitch    u32  bytes per row
//! 13  hot_x    u16
//! 15  hot_y    u16
//! 17  data     pitch * height bytes
//! ```

use crate::DecodeError;
use crate::wire::{Reader, WriteExt, wire_enum};

pub const MAX_CURSOR_DIM: u16 = 1024;

wire_enum! {
    /// Mirrors `DXGI_OUTDUPL_POINTER_SHAPE_TYPE`.
    pub enum CursorKind {
        /// 1 bpp AND mask followed by 1 bpp XOR mask.
        Monochrome = 1,
        /// 32 bpp BGRA with alpha.
        Color = 2,
        /// 32 bpp BGR; alpha 0xFF means XOR with the screen, 0 means replace.
        MaskedColor = 3,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    pub id: u32,
    pub kind: CursorKind,
    pub width: u16,
    pub height: u16,
    pub pitch: u32,
    pub hot_x: u16,
    pub hot_y: u16,
    pub data: Vec<u8>,
}

impl CursorShape {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u32(self.id);
        out.put_u8(self.kind as u8);
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.put_u32(self.pitch);
        out.put_u16(self.hot_x);
        out.put_u16(self.hot_y);
        out.extend_from_slice(&self.data);
    }

    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        let id = r.u32()?;
        let kind = CursorKind::try_from(r.u8()?)?;
        let width = r.u16()?;
        let height = r.u16()?;
        let pitch = r.u32()?;
        let hot_x = r.u16()?;
        let hot_y = r.u16()?;
        if width == 0 || height == 0 || width > MAX_CURSOR_DIM || height > MAX_CURSOR_DIM {
            return Err(DecodeError::InvalidValue("CursorShape.size"));
        }
        let min_pitch = match kind {
            CursorKind::Monochrome => u32::from(width).div_ceil(8),
            CursorKind::Color | CursorKind::MaskedColor => u32::from(width) * 4,
        };
        if pitch < min_pitch || pitch > u32::from(MAX_CURSOR_DIM) * 4 {
            return Err(DecodeError::InvalidValue("CursorShape.pitch"));
        }
        let data = r.take(pitch as usize * height as usize)?.to_vec();
        r.finish()?;
        Ok(CursorShape {
            id,
            kind,
            width,
            height,
            pitch,
            hot_x,
            hot_y,
            data,
        })
    }
}
