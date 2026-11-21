//! Desktop capture via DXGI Desktop Duplication (Windows only), plus a
//! synthetic GPU frame source for single-machine tests.
//!
//! The captured frame stays in video memory: `AcquireNextFrame` → GPU copy
//! into our own BGRA texture → colour conversion → NVENC. The CPU never
//! touches pixels except in debug dumps (`read_bgra`).

pub mod barcode;
pub mod qpmap;
pub mod regions;

#[cfg(any(windows, lumen_check))]
mod synthetic;
#[cfg(any(windows, lumen_check))]
mod win;
#[cfg(any(windows, lumen_check))]
pub use synthetic::*;
#[cfg(any(windows, lumen_check))]
pub use win::*;
