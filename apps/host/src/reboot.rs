//! A client asked to restart this machine (signaling `RebootWanted`): restart
//! now, closing whatever runs without asking (no "this app is preventing
//! restart" screen) and without installing pending Windows updates.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{
    ERROR_NOT_ALL_ASSIGNED, ERROR_SUCCESS, GetLastError, HANDLE, LUID,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Shutdown::{
    InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
    SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FORCE_OTHERS, SHUTDOWN_FORCE_SELF, SHUTDOWN_RESTART,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::w;

use crate::winsec::Handle;

/// Starts the restart (it happens right after this returns).
pub fn restart_machine() -> Result<()> {
    enable_shutdown_privilege().context("SeShutdownPrivilege")?;
    let err = unsafe {
        InitiateShutdownW(
            None,
            w!("Lumen: a client restarts this machine"),
            0,
            SHUTDOWN_RESTART | SHUTDOWN_FORCE_OTHERS | SHUTDOWN_FORCE_SELF,
            SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
        )
    };
    if err != ERROR_SUCCESS.0 {
        bail!("InitiateShutdownW: error {err}");
    }
    Ok(())
}

/// SYSTEM holds the privilege, but disabled: enable it in this process.
fn enable_shutdown_privilege() -> Result<()> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }?;
    // SAFETY: `token` was opened by `OpenProcessToken` above; the wrapper owns it now.
    let token = unsafe { Handle::from_raw(token) };
    let mut luid = LUID::default();
    unsafe { LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid) }?;
    let mut tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        ..Default::default()
    };
    tp.Privileges[0].Luid = luid;
    tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
    unsafe { AdjustTokenPrivileges(token.get(), false, Some(&tp), 0, None, None) }?;
    // Succeeds without the privilege too, with ERROR_NOT_ALL_ASSIGNED.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        bail!("this account does not hold it");
    }
    Ok(())
}
