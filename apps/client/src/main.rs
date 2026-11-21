//! lumen-client: window + strictly 1:1 swapchain presenter.

#[cfg(any(windows, lumen_check))]
mod app;
#[cfg(any(windows, lumen_check))]
mod arrival;
#[cfg(any(windows, lumen_check))]
mod audio;
#[cfg(any(windows, lumen_check))]
mod autofec;
#[cfg(any(windows, lumen_check))]
mod clipboard;
#[cfg(any(windows, lumen_check))]
mod clock;
#[cfg(any(windows, lumen_check))]
mod composite;
#[cfg(any(windows, lumen_check))]
mod cursor;
#[cfg(any(windows, lumen_check))]
mod extra;
mod layout;
#[cfg(any(windows, lumen_check))]
mod live;
#[cfg(any(windows, lumen_check))]
mod memory;
#[cfg(any(windows, lumen_check))]
mod monitor_pick;
#[cfg(any(windows, lumen_check))]
mod net;
#[cfg(any(windows, lumen_check))]
mod overlay;
#[cfg(any(windows, lumen_check))]
#[cfg(any(windows, lumen_check))]
mod pace;
#[cfg(any(windows, lumen_check))]
mod paste;
#[cfg(any(windows, lumen_check))]
mod present;
#[cfg(any(windows, lumen_check))]
mod render;
#[cfg(any(windows, lumen_check))]
mod scene;
#[cfg(any(windows, lumen_check))]
mod shortcuts;
#[cfg(any(windows, lumen_check))]
mod status;
#[cfg(any(windows, lumen_check))]
mod tune;
#[cfg(any(windows, lumen_check))]
#[cfg(any(windows, lumen_check))]
#[cfg(any(windows, lumen_check))]
mod ui_api;
#[cfg(any(windows, lumen_check))]
mod window;

#[cfg(any(windows, lumen_check))]
fn main() -> anyhow::Result<()> {
    app::main()
}

#[cfg(not(any(windows, lumen_check)))]
fn main() {
    eprintln!("lumen-client runs on Windows with an NVIDIA GPU.");
    std::process::exit(1);
}
