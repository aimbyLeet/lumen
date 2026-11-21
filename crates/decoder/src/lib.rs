//! Client-side video decoding: HEVC Main10 on NVDEC via the CUDA driver API,
//! output copied GPU-to-GPU into D3D11 textures (CUDA ↔ D3D11 interop).
//!
//! The decoded picture is exposed as two textures, luma `R16_UNORM` and
//! chroma `R16G16_UNORM`, in the same layout the host shader writes (10-bit
//! codes in the high bits), ready for the P010 → RGB display shader.

pub mod annexb;

#[cfg(any(windows, lumen_check))]
mod readback;
#[cfg(any(windows, lumen_check))]
mod win;
#[cfg(any(windows, lumen_check))]
pub use readback::{frame_hash, read_frame};
#[cfg(any(windows, lumen_check))]
pub use win::*;
