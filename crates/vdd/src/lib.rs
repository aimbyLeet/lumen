//! Lumen VDD virtual monitors on the host side: the
//! driver's contract and device, the monitors' settings and layout, and
//! Windows' display configuration.

pub mod contract;
pub mod layout;

#[cfg(any(windows, lumen_check))]
pub mod device;
#[cfg(any(windows, lumen_check))]
pub mod display;
