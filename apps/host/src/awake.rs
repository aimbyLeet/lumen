//! «Не давать компьютеру засыпать»: while the switch is on and the service runs, the
//! service holds a power request and Windows does not go to sleep on its idle timer, whatever its power plan says.
//! The request belongs to the process: a stopped or dead service holds nothing. Choosing «Сон» by hand and closing
//! a laptop's lid still put the computer to sleep; the display still turns off on its own timer.
//!
//! The switch is the machine's, `%ProgramData%\Lumen\power.json` (written by the service), like the start with Windows.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use desktop::log::Log;
use serde::{Deserialize, Serialize};
use windows::Win32::System::Power::{
    PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows::core::PWSTR;

use crate::winsec::{Handle, wide};

/// What `powercfg /requests` shows.
const REASON: &str = "Lumen Host: не давать компьютеру засыпать";
/// POWER_REQUEST_CONTEXT_VERSION: its one feature of the crate is not worth it.
const CONTEXT_VERSION: u32 = 0;

/// A held power request; dropped, it is let go.
struct Hold {
    _request: Handle,
}
// A kernel handle, not tied to a thread.
unsafe impl Send for Hold {}

static HOLD: Mutex<Option<Hold>> = Mutex::new(None);

fn hold() -> Result<Hold> {
    let mut reason = wide(REASON);
    let context = REASON_CONTEXT {
        Version: CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: PWSTR(reason.as_mut_ptr()),
        },
    };
    unsafe {
        // SAFETY: the handle comes from `PowerCreateRequest`; the wrapper owns it now.
        let request = Handle::from_raw(PowerCreateRequest(&context).context("PowerCreateRequest")?);
        PowerSetRequest(request.get(), PowerRequestSystemRequired).context("PowerSetRequest")?;
        Ok(Hold { _request: request })
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    keep_awake: bool,
}

fn path() -> PathBuf {
    crate::registry::program_data().join("power.json")
}

fn parse(text: &str) -> bool {
    serde_json::from_str::<Saved>(text).is_ok_and(|s| s.keep_awake)
}

/// The switch.
pub fn is_on() -> bool {
    std::fs::read_to_string(path()).is_ok_and(|t| parse(&t))
}

/// The service started: as the switch says.
pub fn start(log: &Log) {
    if !is_on() {
        return;
    }
    match hold() {
        Ok(h) => {
            *HOLD.lock().unwrap() = Some(h);
            log.line("keep awake: on");
        }
        Err(e) => log.line(&format!("keep awake: {e:#}")),
    }
}

/// The service is stopping.
pub fn release() {
    *HOLD.lock().unwrap() = None;
}

/// The switch from Lumen Host: in force before the answer.
pub fn set(on: bool) -> Result<()> {
    set_in(&path(), on)
}

/// Nothing changes if it cannot be kept: the request and the file stay as they were.
fn set_in(file: &Path, on: bool) -> Result<()> {
    let mut held = HOLD.lock().unwrap();
    let fresh = if on && held.is_none() {
        Some(hold()?)
    } else {
        None
    };
    let text = serde_json::to_string(&Saved { keep_awake: on })?;
    std::fs::write(file, text).context("power.json")?;
    if !on {
        *held = None;
    } else if fresh.is_some() {
        *held = fresh;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_off_unless_it_says_on() {
        assert!(parse(r#"{"keep_awake":true}"#));
        assert!(!parse(r#"{"keep_awake":false}"#));
        assert!(!parse("{}"));
        assert!(!parse("not json"));
    }

    /// That Windows takes the request. Whether it shows: `powercfg /requests` as an administrator
    /// (SYSTEM section); the system's execution state says nothing, others hold it too.
    /// One test for `HOLD`, the process's one request.
    #[test]
    fn the_request_follows_the_switch_unless_it_cannot_be_kept() {
        let held = || HOLD.lock().unwrap().is_some();
        let dir = std::env::temp_dir().join(format!("lumen-awake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("power.json");
        let nowhere = dir.join("no-such-folder").join("power.json");
        let saved = || parse(&std::fs::read_to_string(&file).unwrap());

        // Cannot be written: stays off, and stays on.
        assert!(set_in(&nowhere, true).is_err());
        assert!(!held());
        set_in(&file, true).unwrap();
        assert!(held() && saved());
        assert!(set_in(&nowhere, false).is_err());
        assert!(held(), "still on, as the file says");
        assert!(set_in(&nowhere, true).is_err());
        assert!(held(), "a repeat does not let go of it");

        set_in(&file, true).unwrap();
        assert!(held() && saved());
        set_in(&file, false).unwrap();
        assert!(!held() && !saved());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
