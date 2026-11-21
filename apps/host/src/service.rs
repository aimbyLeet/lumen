//! `lumen-host service install | uninstall | run`: the Windows service
//! "Lumen Host". It runs as LocalSystem from boot and
//! keeps the workers (`worker`) going in the console session.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use desktop::log::Log;
use windows::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_SERVICE_DOES_NOT_EXIST, NO_ERROR,
};
use windows::Win32::Security::DACL_SECURITY_INFORMATION;
use windows::Win32::System::Services::*;
use windows::core::{PCWSTR, PWSTR};

use crate::registry::program_data;
use crate::winsec::wide;
use crate::worker::Workers;

pub const NAME: &str = desktop::hostsvc::SERVICE;
const DISPLAY_NAME: &str = "Lumen Host";
const DESCRIPTION: &str = "Streams this computer's screen to Lumen clients: also the sign-in \
                           and lock screens, UAC prompts and elevated windows.";
/// Who may do what with the service: SYSTEM and administrators as usual;
/// signed-in users may also query and start it (Lumen Host turns the host on;
/// it stops itself on the pipe's request).
const SERVICE_SDDL: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)\
                            (A;;CCLCRPLORC;;;AU)";

/// A service-control handle, closed when dropped.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

/// The Service Control Manager with `access`.
fn manager(access: u32) -> Result<ScHandle> {
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }
        .map(ScHandle)
        .context("OpenSCManager")
}

/// This service with `access`.
fn open(scm: &ScHandle, access: u32) -> windows::core::Result<ScHandle> {
    unsafe { OpenServiceW(scm.0, PCWSTR(wide(NAME).as_ptr()), access) }.map(ScHandle)
}

/// Installs the service (auto start, restarts on failure) and starts it.
/// Updating keeps the start type the users chose. Needs an elevated prompt.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let cmd = format!("\"{}\" service run", exe.display());
    let scm = manager(SC_MANAGER_ALL_ACCESS).context("run as administrator")?;
    let svc = match open(&scm, SERVICE_ALL_ACCESS) {
        // Already there: point it at this program.
        Ok(svc) => {
            unsafe {
                ChangeServiceConfigW(
                    svc.0,
                    ENUM_SERVICE_TYPE(SERVICE_WIN32_OWN_PROCESS.0),
                    SERVICE_START_TYPE(SERVICE_NO_CHANGE),
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(wide(&cmd).as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(wide(DISPLAY_NAME).as_ptr()),
                )
            }
            .context("ChangeServiceConfig")?;
            svc
        }
        Err(_) => unsafe {
            CreateServiceW(
                scm.0,
                PCWSTR(wide(NAME).as_ptr()),
                PCWSTR(wide(DISPLAY_NAME).as_ptr()),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                PCWSTR(wide(&cmd).as_ptr()),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(), // LocalSystem
                PCWSTR::null(),
            )
        }
        .map(ScHandle)
        .context("CreateService")?,
    };
    configure(&svc)?;
    match unsafe { StartServiceW(svc.0, None) } {
        Ok(()) => println!("service {NAME} installed and started: {cmd}"),
        Err(e)
            if e.code()
                == windows::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING.to_hresult() =>
        {
            println!("service {NAME} updated (already running: restart it to use this program)")
        }
        Err(e) => return Err(e).context("StartService"),
    }
    Ok(())
}

/// Who may control the service, its description, restarts after a crash.
fn configure(svc: &ScHandle) -> Result<()> {
    let sec = crate::winsec::Security::from_sddl(SERVICE_SDDL)?;
    unsafe {
        SetServiceObjectSecurity(svc.0, DACL_SECURITY_INFORMATION, sec.descriptor())
            .context("SetServiceObjectSecurity")?;
        let mut desc = wide(DESCRIPTION);
        let d = SERVICE_DESCRIPTIONW {
            lpDescription: PWSTR(desc.as_mut_ptr()),
        };
        let _ = ChangeServiceConfig2W(
            svc.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some(&d as *const _ as *const c_void),
        );
        // Restart 5 s after a crash, every time.
        let mut actions = [SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5000,
        }; 3];
        let f = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: 86_400,
            lpRebootMsg: PWSTR::null(),
            lpCommand: PWSTR::null(),
            cActions: actions.len() as u32,
            lpsaActions: actions.as_mut_ptr(),
        };
        let _ = ChangeServiceConfig2W(
            svc.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some(&f as *const _ as *const c_void),
        );
    }
    Ok(())
}

/// Stops and removes the service. Needs an elevated prompt.
pub fn uninstall() -> Result<()> {
    let scm = manager(SC_MANAGER_ALL_ACCESS).context("run as administrator")?;
    let svc = match open(&scm, SERVICE_ALL_ACCESS) {
        Ok(s) => s,
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => {
            println!("service {NAME} is not installed");
            return Ok(());
        }
        Err(e) => return Err(e).context("OpenService"),
    };
    unsafe {
        let mut st = SERVICE_STATUS::default();
        let _ = ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st);
        for _ in 0..100 {
            if QueryServiceStatus(svc.0, &mut st).is_err() || st.dwCurrentState == SERVICE_STOPPED {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        DeleteService(svc.0).context("DeleteService")?;
    }
    println!("service {NAME} stopped and removed");
    Ok(())
}

static STOP: AtomicBool = AtomicBool::new(false);

/// The pipe asked to turn the host off: the service stops as if told by the SCM.
pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
    set_state(SERVICE_STOP_PENDING);
}

/// Opens this service with `access` (from the service itself: SYSTEM).
fn open_self(access: u32) -> Result<ScHandle> {
    open(&manager(SC_MANAGER_CONNECT)?, access).context("OpenService")
}

/// The service starts with Windows.
pub fn autostart() -> Result<bool> {
    let svc = open_self(SERVICE_QUERY_CONFIG)?;
    unsafe {
        let mut need = 0u32;
        let _ = QueryServiceConfigW(svc.0, None, 0, &mut need);
        let mut buf = vec![0u64; (need as usize).div_ceil(8).max(1)];
        let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        QueryServiceConfigW(svc.0, Some(cfg), (buf.len() * 8) as u32, &mut need)
            .context("QueryServiceConfig")?;
        Ok((*cfg).dwStartType == SERVICE_AUTO_START)
    }
}

/// Starts the service with Windows, or only when Lumen Host starts it.
pub fn set_autostart(on: bool) -> Result<()> {
    let svc = open_self(SERVICE_CHANGE_CONFIG)?;
    unsafe {
        ChangeServiceConfigW(
            svc.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            if on {
                SERVICE_AUTO_START
            } else {
                SERVICE_DEMAND_START
            },
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
        .context("ChangeServiceConfig")
    }
}

static STATUS: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn set_state(state: SERVICE_STATUS_CURRENT_STATE) {
    let handle = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::Relaxed));
    if handle.is_invalid() {
        return;
    }
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 10_000,
    };
    unsafe {
        let _ = SetServiceStatus(handle, &st);
    }
}

unsafe extern "system" fn handler(control: u32, _: u32, _: *mut c_void, _: *mut c_void) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::Relaxed);
            set_state(SERVICE_STOP_PENDING);
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(NAME);
    let Ok(h) =
        (unsafe { RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None) })
    else {
        return;
    };
    STATUS.store(h.0, Ordering::Relaxed);
    set_state(SERVICE_RUNNING);
    // Before the log: it lives in the folder that may have to move.
    let root = crate::registry::ensure_root();
    let log = Arc::new(Log::new(program_data().join("logs").join("service.log")));
    log.new_file(&format!("service started ({})", env!("CARGO_PKG_VERSION")));
    match root {
        Ok(note) => {
            if let Some(n) = note {
                log.line(&n);
            }
        }
        Err(e) => {
            // Not a folder of ours: no registration, no host.
            log.line(&format!("{e:#}; not serving"));
            while !STOP.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
            }
            set_state(SERVICE_STOPPED);
            return;
        }
    }
    crate::awake::start(&log);
    // Just updated (or tried): the installer closed Lumen Host, it comes back
    // for the user signed in on the console (its window, if the update was
    // clicked there). The host finds out whether it worked.
    if let Some(p) = crate::update::pending() {
        log.line(&format!(
            "an update to Lumen {} was run; Lumen Host back for the user",
            p.version
        ));
        let log = log.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            if let Err(e) = crate::update::bring_back_lumen_host(p.reopen) {
                log.line(&format!("Lumen Host not started for the user: {e:#}"));
            }
        });
    }
    let statuses = crate::registry::Statuses::default();
    // The virtual monitors live while the service does.
    let monitors = crate::monitors::Keeper::new(log.clone());
    {
        let (statuses, monitors, log) = (statuses.clone(), monitors.clone(), log.clone());
        std::thread::spawn(move || crate::registry::serve_pipe(statuses, monitors, log));
    }
    match Workers::new(log.clone(), statuses) {
        Ok(mut workers) => {
            while !STOP.load(Ordering::Relaxed) {
                workers.tick();
                monitors.tick();
                std::thread::sleep(Duration::from_millis(500));
            }
            workers.stop_all();
        }
        Err(e) => log.line(&format!("cannot supervise: {e:#}")),
    }
    crate::awake::release();
    log.line("--- service stopped");
    set_state(SERVICE_STOPPED);
}

/// Entry point when the Service Control Manager starts the service.
pub fn run() -> Result<()> {
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }
        .context("StartServiceCtrlDispatcher (only the Service Control Manager runs `service run`)")
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_access_parses() {
        crate::winsec::Security::from_sddl(super::SERVICE_SDDL).unwrap();
    }
}
