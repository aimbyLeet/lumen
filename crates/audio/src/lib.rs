//! Sound from the host to the client.
//!
//! Windows: what the machine plays, captured from the default output device
//! with WASAPI loopback ([`Loopback`]): the mix Windows sends to the device.
//! Any OS: Opus as Lumen streams it ([`opus`]), float WAV files ([`wav`]) and
//! checks of a test tone ([`tone`]).

pub mod framer;
pub mod opus;
pub mod playout;
pub mod resample;
pub mod tone;
pub mod wav;

#[cfg(any(windows, lumen_check))]
mod wasapi;
#[cfg(any(windows, lumen_check))]
pub use wasapi::{
    AudioError, Endpoint, Format, Loopback, Render, audio_thread_priority, qpc_now_100ns,
    render_endpoints,
};

pub use framer::Packet;
