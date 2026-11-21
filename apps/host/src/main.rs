//! lumen-host: streams one monitor (or a synthetic source) to a Lumen client.
//!
//! source (BGRA8 on the GPU) → BGRA→P010 shader → NVENC HEVC Main10 →
//! FEC packetizer → paced UDP. The client drives the session: `Hello` starts
//! it, `RequestRecovery` makes the next frame skip lost references,
//! `RequestKeyframe` forces an IDR.

#[cfg(any(windows, lumen_check))]
mod app;
#[cfg(any(windows, lumen_check))]
mod audio;
#[cfg(any(windows, lumen_check))]
mod audio_probe;
#[cfg(any(windows, lumen_check))]
mod awake;
#[cfg(any(windows, lumen_check))]
mod clip_agent;
#[cfg(any(windows, lumen_check))]
mod clipboard;
#[cfg(any(windows, lumen_check))]
mod control;
#[cfg(any(windows, lumen_check))]
mod cursor;
#[cfg(any(windows, lumen_check))]
mod files;
#[cfg(any(windows, lumen_check))]
mod headless;
#[cfg(any(windows, lumen_check))]
mod headless_probe;
#[cfg(any(windows, lumen_check))]
mod monitors;
#[cfg(any(windows, lumen_check))]
mod netinfo;
#[cfg(any(windows, lumen_check))]
mod reboot;
#[cfg(any(windows, lumen_check))]
mod registry;
#[cfg(any(windows, lumen_check))]
mod remote_input;
#[cfg(any(windows, lumen_check))]
mod service;
#[cfg(any(windows, lumen_check))]
mod session;
#[cfg(any(windows, lumen_check))]
mod source;
#[cfg(any(windows, lumen_check))]
mod station;
#[cfg(any(windows, lumen_check))]
mod streamer;
#[cfg(any(windows, lumen_check))]
mod update;
#[cfg(any(windows, lumen_check))]
mod user_files;
#[cfg(any(windows, lumen_check))]
mod winsec;
#[cfg(any(windows, lumen_check))]
mod worker;

#[cfg(any(windows, lumen_check))]
fn main() -> anyhow::Result<()> {
    app::main()
}

#[cfg(not(any(windows, lumen_check)))]
fn main() {
    eprintln!("lumen-host runs on Windows with an NVIDIA GPU.");
    std::process::exit(1);
}
