//! What the host tells the server about itself besides its addresses
//! (`desktop::net`): whether its Windows session is the one on screen (a
//! host run by hand, in a user's session; the service's host is always
//! available); and the console's lock, which the service's host uses after a
//! session.

use anyhow::{Context, Result};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_SESSIONSTATE_LOCK, WTSFreeMemory, WTSGetActiveConsoleSessionId,
    WTSINFOEXW, WTSQuerySessionInformationW, WTSQueryUserToken, WTSSessionInfoEx,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::PWSTR;

use crate::winsec::{Handle, start_on_desktop};

/// This process runs in the session on the console (not another user's,
/// switched away from, nor a disconnected one).
pub fn session_active() -> bool {
    let mut mine = 0u32;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut mine) }.is_err() {
        return true;
    }
    unsafe { WTSGetActiveConsoleSessionId() == mine }
}

/// The console session is locked (its lock screen is up).
pub fn console_locked() -> bool {
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    let mut buf = PWSTR::null();
    let mut len = 0u32;
    if unsafe { WTSQuerySessionInformationW(None, session, WTSSessionInfoEx, &mut buf, &mut len) }
        .is_err()
    {
        return false;
    }
    let locked = (len as usize) >= std::mem::size_of::<WTSINFOEXW>() && {
        let info = unsafe { &*(buf.0 as *const WTSINFOEXW) };
        info.Level == 1
            && unsafe { info.Data.WTSInfoExLevel1.SessionFlags } == WTS_SESSIONSTATE_LOCK as i32
    };
    unsafe { WTSFreeMemory(buf.0 as *mut _) };
    locked
}

/// Locks the console session, if someone is signed in on it and it is not
/// locked yet: `rundll32 user32.dll,LockWorkStation` as that user, on their
/// desktop (`LockWorkStation` is meant for the signed-in user's processes).
/// Returns whether a lock was started. Only a service (SYSTEM) may do it.
pub fn lock_console() -> Result<bool> {
    if console_locked() {
        return Ok(false);
    }
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    let mut token = HANDLE::default();
    if unsafe { WTSQueryUserToken(session, &mut token) }.is_err() {
        // Nobody signed in: the sign-in screen, nothing to lock.
        return Ok(false);
    }
    // SAFETY: `token` comes from `WTSQueryUserToken`, which succeeded; the wrapper owns it now.
    let token = unsafe { Handle::from_raw(token) };
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let cmd = format!(r#""{system}\System32\rundll32.exe" user32.dll,LockWorkStation"#);
    start_on_desktop(token.get(), &cmd, None, None).context("rundll32 LockWorkStation")?;
    Ok(true)
}
