//! What the Lumen desktop apps (Lumen Host, Lumen Client) share. Their data
//! folder is `transport::default_dir`, the same as the streaming programs'.

#[cfg(any(windows, lumen_check))]
pub mod clip_probe;
#[cfg(any(windows, lumen_check))]
pub mod clipboard;
#[cfg(any(windows, lumen_check))]
pub mod cpu;
#[cfg(any(windows, lumen_check))]
pub mod files;
#[cfg(any(windows, lumen_check))]
mod handle;
#[cfg(any(windows, lumen_check))]
pub mod hostsvc;
pub mod lang;
#[cfg(any(windows, lumen_check))]
pub mod log;
#[cfg(any(windows, lumen_check))]
pub mod nap;
pub mod net;
#[cfg(any(windows, lumen_check))]
pub mod netpath;
#[cfg(any(windows, lumen_check))]
pub mod secret;
#[cfg(any(windows, lumen_check))]
pub mod update;

use std::path::{Path, PathBuf};

#[cfg(any(windows, lumen_check))]
pub use handle::Handle;

/// `%LOCALAPPDATA%\Lumen\logs\<name>`, or `<data>\logs\<name>` with `LUMEN_DATA`.
pub fn log_path(data: &Path, name: &str) -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(d) if std::env::var_os("LUMEN_DATA").is_none() => {
            PathBuf::from(d).join("Lumen").join("logs").join(name)
        }
        _ => data.join("logs").join(name),
    }
}

/// Opens a folder in Explorer.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("err_open_folder|{}: {e}", dir.display()))
}

/// This computer's name.
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "host".into())
}
