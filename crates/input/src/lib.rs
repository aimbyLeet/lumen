//! Remote input (Windows only).
//!
//! - client: [`from_window_message`] turns client-window messages into
//!   [`protocol::InputEvent`]s (pure function); [`Held`] remembers what is
//!   held down, to release it when the window loses the focus;
//! - host: [`Injector`] replays events with `SendInput`. Keys are sent by scan
//!   code, the mouse by absolute coordinates over the whole virtual desktop, so
//!   it works with several monitors and any DPI scaling (physical pixels).

#[cfg(any(windows, lumen_check))]
mod held;
#[cfg(any(windows, lumen_check))]
mod inject;
#[cfg(any(windows, lumen_check))]
mod paste;
#[cfg(any(windows, lumen_check))]
mod window;

#[cfg(any(windows, lumen_check))]
pub use held::Held;
#[cfg(any(windows, lumen_check))]
pub use inject::*;
#[cfg(any(windows, lumen_check))]
pub use paste::{PasteKeys, ctrl_v};
#[cfg(any(windows, lumen_check))]
pub use window::{WM_SECURE_ATTENTION, from_window_message};
