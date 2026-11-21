//! GPU colour conversion on D3D11.
//!
//! - Host: desktop BGRA8 → P010 (4:2:0, 10 bit, BT.709 full range), `win.rs`.
//! - Client: decoded 4:2:0 → 10-bit R'G'B' for display, `display.rs`.
//!
//! The math is specified by the `color` crate; `tools/bench --verify` and
//! `tools/decode --rgb-check` check the GPU output against it.

#[cfg(any(windows, lumen_check))]
mod d3d;
#[cfg(any(windows, lumen_check))]
mod display;
#[cfg(any(windows, lumen_check))]
mod win;
#[cfg(any(windows, lumen_check))]
pub use d3d::*;
#[cfg(any(windows, lumen_check))]
pub use display::*;
#[cfg(any(windows, lumen_check))]
pub use win::*;
