//! The service's registration: the machine's, one for
//! all its Windows accounts, `%ProgramData%\Lumen\host`, and the pipe through
//! which any signed-in user manages it (`desktop::hostsvc`).
//!
//! Its host runs as SYSTEM and writes into the folder, so users must not be
//! able to write there (a junction would send SYSTEM's writes anywhere): the
//! service makes the folder — SYSTEM and administrators full, signed-in users
//! read-only — and changes it only on their requests.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use desktop::hostsvc::{HostStatus, PIPE, Request, Response, Settings, State};
use desktop::log::Log;
use serde_json::Value;
use transport::HostToken;
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSID, RevertToSelf, SetFileSecurityW, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};
use windows::core::PCWSTR;

use crate::monitors::Keeper;
use crate::winsec::{ADMINS_SID, Handle, SYSTEM_SID, Security, is_user_sid, wide};
use crate::worker::Spec;

/// The settings file of the registration (written by the service only).
const SETTINGS: &str = "host.json";
const SERVER_CA: &str = "server-ca.pem";
/// The host's token and its device credentials on the server (the host's own files).
const TOKEN: &str = "host.token";
const CREDS: &str = "host.creds";
/// How much of the host's log a `State` carries.
const LOG_TAIL: u64 = 64 << 10;

/// `%ProgramData%\Lumen`: SYSTEM and administrators full (inherited),
/// signed-in users may only look in (not inherited).
const ROOT_SDDL: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;AU)";
/// The registration's folder: signed-in users read it, only SYSTEM /
/// administrators write.
const REGISTRATION_SDDL: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
/// The pipe: SYSTEM, and signed-in users read / write.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GRGW;;;AU)";

pub use desktop::hostsvc::{program_data, registration_dir};

/// What the host's log files are named after (`desktop::log`): files
/// `lumen-host-<time>.log` in `<registration>\logs`.
pub fn host_log(dir: &Path) -> PathBuf {
    dir.join("logs").join("lumen-host.log")
}

/// What the pipe and the worker share: the host's status and the restart
/// the pipe asks for.
#[derive(Default)]
pub struct Shared {
    host: Mutex<Option<Entry>>,
    restart: AtomicBool,
    runs: AtomicU64,
}

/// The host's status and which run of it (the reader of an old one may still
/// be finishing when a new one starts).
struct Entry {
    status: HostStatus,
    run: u64,
}

pub type Statuses = Arc<Shared>;

impl Shared {
    /// The host starts: its run.
    pub fn started(&self) -> u64 {
        let run = self.runs.fetch_add(1, Ordering::Relaxed) + 1;
        *self.host.lock().unwrap() = Some(Entry {
            status: HostStatus::starting(),
            run,
        });
        run
    }

    /// The entry of run `run`, if it is still the current one.
    fn with_run(&self, run: u64, f: impl FnOnce(&mut HostStatus)) {
        if let Some(e) = self.host.lock().unwrap().as_mut()
            && e.run == run
        {
            f(&mut e.status);
        }
    }

    /// One `@status` event of run `run`. `update_ready`: the host downloaded
    /// the update — the service runs its installer (which stops the service).
    pub fn event(&self, run: u64, v: &Value) {
        if v["event"] == "update_ready" {
            let path = Path::new(v["path"].as_str().unwrap_or_default());
            if let Err(e) = crate::update::run_installer(path) {
                self.with_run(run, |s| {
                    s.apply(&crate::update::failed_event("err_update_run"))
                });
                eprintln!("update: {e:#}");
            }
            return;
        }
        self.with_run(run, |s| s.apply(v));
    }

    /// Run `run` ended; `last` is its last line of output. Killed for a
    /// restart, a newer run owns the status by now.
    pub fn exited(&self, run: u64, last: &str) {
        self.with_run(run, |s| {
            *s = HostStatus {
                state: "error".into(),
                detail: if last.is_empty() {
                    "err_host_exited".into()
                } else {
                    last.to_string()
                },
                ..HostStatus::stopped()
            }
        });
    }

    /// The service stopped the host for good.
    pub fn stopped(&self) {
        *self.host.lock().unwrap() = None;
    }

    pub(crate) fn status(&self) -> Option<HostStatus> {
        self.host.lock().unwrap().as_ref().map(|e| e.status.clone())
    }

    fn ask_restart(&self) {
        self.restart.store(true, Ordering::Relaxed);
    }

    /// The pipe asked to restart the host (once).
    pub fn take_restart(&self) -> bool {
        self.restart.swap(false, Ordering::Relaxed)
    }
}

fn wide_path(p: &Path) -> Vec<u16> {
    wide(&p.to_string_lossy())
}

/// The owner of `path` is SYSTEM or the administrators.
fn owned_by_system(path: &Path) -> Result<bool> {
    let mut owner = PSID::default();
    let mut sd = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
    let err = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(wide_path(path).as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut sd,
        )
    };
    if err.is_err() {
        bail!("owner of {}: {err:?}", path.display());
    }
    let sid = crate::winsec::psid_string(owner);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    let sid = sid?;
    Ok(sid == SYSTEM_SID || sid == ADMINS_SID)
}

fn is_reparse_point(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0)
}

/// Makes `path` a real directory with exactly the DACL of `sddl`.
fn make_dir(path: &Path, sddl: &str) -> Result<()> {
    let sec = Security::from_sddl(sddl)?;
    if is_reparse_point(path) {
        bail!("{} is a link, not a folder", path.display());
    }
    let name = wide_path(path);
    if !path.exists() {
        unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&sec.attrs)) }
            .with_context(|| format!("create {}", path.display()))?;
    }
    unsafe {
        SetFileSecurityW(
            PCWSTR(name.as_ptr()),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sec.descriptor(),
        )
    }
    .ok()
    .with_context(|| format!("set the access of {}", path.display()))
}

/// `%ProgramData%\Lumen`, made by SYSTEM. One that someone else made first (it
/// could hold links of theirs) is moved aside: call before opening anything
/// in it. Returns what to log.
pub fn ensure_root() -> Result<Option<String>> {
    let mut note = None;
    let root = program_data();
    if root.exists() && (is_reparse_point(&root) || !owned_by_system(&root)?) {
        let aside = root.with_file_name(format!(
            "Lumen.untrusted-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        ));
        std::fs::rename(&root, &aside).with_context(|| format!("move {} aside", root.display()))?;
        note = Some(format!(
            "{} was not made by the service: moved to {}",
            root.display(),
            aside.display()
        ));
    }
    make_dir(&root, ROOT_SDDL)?;
    Ok(note)
}

fn read_settings(dir: &Path) -> Option<Settings> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(SETTINGS)).ok()?).ok()
}

fn read_token(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(TOKEN))
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

/// What the service runs: the machine's host, once it has a server set.
pub fn worker_spec() -> Option<Spec> {
    let dir = registration_dir();
    let s = read_settings(&dir)?;
    if s.server.trim().is_empty() {
        return None;
    }
    Some(Spec {
        args: worker_args(&dir, &s),
        token: read_token(&dir),
        dir,
    })
}

/// The host's command line: only from checked fields, nothing the user
/// passes through (the process is SYSTEM's).
fn worker_args(dir: &Path, s: &Settings) -> Vec<String> {
    let mut args = vec![
        "--server".to_string(),
        s.server.trim().to_string(),
        "--service".into(),
        "--status-json".into(),
    ];
    if !s.machine.trim().is_empty() {
        args.extend(["--machine".into(), s.machine.trim().to_string()]);
    }
    if !s.server_ca_pem.trim().is_empty() {
        args.extend([
            "--server-ca".into(),
            dir.join(SERVER_CA).display().to_string(),
        ]);
    }
    if let Some(fp) = signal::pin::normalize_fingerprint(&s.server_fingerprint) {
        args.extend(["--server-pin".into(), fp]);
    }
    for a in s
        .manual_addrs
        .split([',', ' ', ';'])
        .filter(|a| a.parse::<std::net::SocketAddr>().is_ok())
    {
        args.extend(["--addr".into(), a.to_string()]);
    }
    args
}

/// Checks what the user sent before it lands in SYSTEM's files.
fn check(s: &Settings) -> Result<()> {
    let short = |what: &str, v: &str| {
        if v.len() > 128 || v.chars().any(char::is_control) {
            bail!("err_field_long|{what}")
        }
        Ok(())
    };
    short("server", &s.server)?;
    short("machine", &s.machine)?;
    short("addresses", &s.manual_addrs)?;
    if s.server.trim().is_empty() {
        bail!("err_server_empty");
    }
    if s.server.trim().starts_with('-') || s.machine.trim().starts_with('-') {
        bail!("err_leading_dash");
    }
    if s.server_ca_pem.len() > 64 << 10 {
        bail!("err_ca_big");
    }
    if !s.server_fingerprint.trim().is_empty()
        && signal::pin::normalize_fingerprint(&s.server_fingerprint).is_none()
    {
        bail!("err_cert_fingerprint");
    }
    Ok(())
}

fn state(statuses: &Statuses, monitors: &Keeper) -> State {
    let dir = registration_dir();
    let settings = read_settings(&dir);
    let registered = settings.is_some();
    State {
        registered,
        settings: settings.unwrap_or_default(),
        token: read_token(&dir),
        status: match statuses.status() {
            Some(s) => s,
            None if registered => HostStatus::starting(),
            None => HostStatus::stopped(),
        },
        autostart: crate::service::autostart().unwrap_or(true),
        keep_awake: crate::awake::is_on(),
        auto_update: crate::update::auto(),
        note: String::new(),
        monitors: monitors.state(),
    }
}

/// The end of the host's newest log file (the current or the last session).
pub fn log_tail() -> String {
    desktop::log::latest(&host_log(&registration_dir()))
        .map_or_else(String::new, |p| desktop::log::tail(&p, LOG_TAIL))
}

fn write(dir: &Path, name: &str, text: &str) -> Result<()> {
    std::fs::write(dir.join(name), text).context(name.to_string())
}

fn new_token(dir: &Path) -> Result<()> {
    write(dir, TOKEN, &format!("{}\n", HostToken::generate()))
}

/// The registration's folder, made if missing.
fn open_folder() -> Result<PathBuf> {
    let dir = registration_dir();
    make_dir(&dir, REGISTRATION_SDDL)?;
    Ok(dir)
}

/// The registration's folder, if the machine has one.
fn registered() -> Result<PathBuf> {
    let dir = registration_dir();
    if read_settings(&dir).is_none() {
        bail!("err_not_registered");
    }
    Ok(dir)
}

fn save_settings(dir: &Path, s: &Settings) -> Result<()> {
    check(s)?;
    if s.server_ca_pem.trim().is_empty() {
        let _ = std::fs::remove_file(dir.join(SERVER_CA));
    } else {
        write(dir, SERVER_CA, &s.server_ca_pem)?;
    }
    write(dir, SETTINGS, &serde_json::to_string_pretty(s)?)
}

fn set(settings: &Settings) -> Result<()> {
    check(settings)?;
    let dir = open_folder()?;
    if read_token(&dir).is_empty() {
        new_token(&dir)?;
    }
    same_server(read_settings(&dir).as_ref(), settings)?;
    save_settings(&dir, settings)
}

/// A registration stays on its server: another one is «Удалить», then a new
/// registration (no silent re-registration behind the user's back).
fn same_server(old: Option<&Settings>, new: &Settings) -> Result<()> {
    if old.is_some_and(|old| old.server.trim() != new.server.trim()) {
        bail!("err_server_locked");
    }
    Ok(())
}

/// The registration's files go but the token (and the logs).
fn forget_here(dir: &Path) {
    for f in [SETTINGS, CREDS, SERVER_CA] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// «Удалить»: the host stops (no settings: the service's next look), the
/// server is asked to remove its record. Returns what became of it.
fn unregister(statuses: &Statuses) -> Result<String> {
    let dir = registered()?;
    let (server, pin) = read_settings(&dir)
        .map(|s| (s.server, s.server_fingerprint))
        .unwrap_or_default();
    let creds: Option<signal::Credentials> = std::fs::read_to_string(dir.join(CREDS))
        .ok()
        .and_then(|t| t.trim().parse().ok());
    let ca = dir.join(SERVER_CA);
    let trust = signal::Trust::choose(
        ca.exists().then_some(ca.as_path()),
        Some(pin.as_str()).filter(|p| !p.trim().is_empty()),
    )
    .ok();
    forget_here(&dir);
    statuses.stopped();
    let (Some(creds), Some(trust)) = (creds, trust) else {
        return Ok("note_removed_local".into());
    };
    let cfg = signal::ServerConfig {
        server: server.trim().to_string(),
        trust,
    };
    Ok(match signal::unregister(&cfg, signal::Role::Host, &creds) {
        Ok(()) => "note_removed".into(),
        Err(signal::LoginError::Rejected(signal::LoginFailure::BadCredentials)) => {
            "note_removed_unknown".into()
        }
        Err(e) => format!("note_removed_here|{e}"),
    })
}

fn remove(statuses: &Statuses) {
    let dir = registration_dir();
    // The host stops at the service's next look; its files go after it.
    let _ = std::fs::remove_file(dir.join(SETTINGS));
    std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                break;
            }
        }
    });
    statuses.stopped();
}

fn stop(statuses: &Statuses) {
    // Whoever is connected must not leave the desktop open behind them.
    if statuses.status().is_some_and(|s| s.state == "session") {
        let _ = crate::netinfo::lock_console();
    }
    crate::service::request_stop();
}

fn handle(req: Request, statuses: &Statuses, monitors: &Keeper) -> Result<State> {
    match req {
        Request::State => {}
        Request::Set { settings } => set(&settings)?,
        Request::Unregister => {
            let note = unregister(statuses)?;
            return Ok(State {
                note,
                ..state(statuses, monitors)
            });
        }
        Request::Reregister => {
            let _ = std::fs::remove_file(registered()?.join(CREDS));
            statuses.ask_restart();
        }
        Request::NewToken => new_token(&registered()?)?,
        Request::SecureAttention => unreachable!("answered in serve_one"),
        Request::Stop => stop(statuses),
        Request::SetAutostart { on } => crate::service::set_autostart(on)?,
        Request::SetKeepAwake { on } => crate::awake::set(on)?,
        Request::SetAutoUpdate { on } => crate::update::set_auto(on)?,
        Request::UpdateNow => {
            registered()?;
            crate::update::ask_now()?;
            statuses.ask_restart();
        }
        Request::Remove => {
            remove(statuses);
            return Ok(State {
                monitors: monitors.state(),
                ..State::default()
            });
        }
        Request::SetMonitors { settings } => monitors.set(settings)?,
        Request::ResetMonitors => monitors.reset()?,
        Request::SetClientMonitors { client, set } => monitors.set_client(client, set)?,
        Request::HostMonitors {
            physical,
            client,
            lost,
        } => monitors.host_report(physical, client, lost)?,
        Request::ClientChoice {
            client,
            width,
            height,
            scale,
        } => monitors.choose(
            client,
            vdd::layout::Screen {
                width,
                height,
                scale,
            },
        )?,
    }
    Ok(state(statuses, monitors))
}

/// The account at the other end of the pipe.
fn caller(pipe: HANDLE) -> Result<String> {
    unsafe {
        ImpersonateNamedPipeClient(pipe).context("ImpersonateNamedPipeClient")?;
        let mut token = HANDLE::default();
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token);
        let _ = RevertToSelf();
        opened.context("OpenThreadToken")?;
        // SAFETY: `token` was opened by `OpenThreadToken` (checked above); the wrapper owns it now.
        crate::winsec::token_user(Handle::from_raw(token).get())
    }
}

/// Answers one connected caller.
fn serve_one(pipe: HANDLE, statuses: &Statuses, monitors: &Keeper) -> Result<()> {
    // Borrowed: the loop below closes the pipe.
    let file = std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(pipe.0) });
    let mut line = String::new();
    BufReader::new(&*file)
        .take(1 << 20)
        .read_line(&mut line)
        .context("read the request")?;
    let answer = match (caller(pipe), serde_json::from_str::<Request>(&line)) {
        (Err(e), _) => Response::Error(format!("err_caller|{e:#}")),
        (_, Err(e)) => Response::Error(format!("err_bad_request|{e}")),
        // The service's own hosts run as SYSTEM.
        (Ok(sid), Ok(Request::SecureAttention)) if sid != SYSTEM_SID => {
            Response::Error("err_sas_only".into())
        }
        (Ok(sid), Ok(req)) if req.system_only() && sid != SYSTEM_SID => {
            Response::Error("err_system_only".into())
        }
        (Ok(_), Ok(Request::SecureAttention)) => match crate::winsec::send_sas() {
            Ok(()) => Response::Ok(Box::default()),
            Err(e) => Response::Error(format!("{e:#}")),
        },
        (Ok(sid), Ok(req)) if !req.system_only() && !is_user_sid(&sid) => {
            Response::Error("err_user_only".into())
        }
        (Ok(_), Ok(req)) => match handle(req, statuses, monitors) {
            Ok(s) => Response::Ok(Box::new(s)),
            Err(e) => Response::Error(format!("{e:#}")),
        },
    };
    let mut out = serde_json::to_string(&answer)?;
    out.push('\n');
    (&*file).write_all(out.as_bytes())?;
    let _ = file.sync_all();
    Ok(())
}

/// Serves the pipe, one caller at a time (their requests are short), forever.
pub fn serve_pipe(statuses: Statuses, monitors: Arc<Keeper>, log: Arc<Log>) {
    let sec = match Security::from_sddl(PIPE_SDDL) {
        Ok(s) => s,
        Err(e) => {
            log.line(&format!("pipe: {e:#}"));
            return;
        }
    };
    let name = wide(PIPE);
    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0),
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 << 10,
                64 << 10,
                0,
                Some(&sec.attrs),
            )
        };
        if pipe.is_invalid() {
            log.line(&format!(
                "pipe {PIPE}: {}",
                windows::core::Error::from_thread()
            ));
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }
        // Connected, or the client came between create and connect.
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok()
            || windows::core::Error::from_thread().code()
                == windows::Win32::Foundation::ERROR_PIPE_CONNECTED.to_hresult();
        if connected && let Err(e) = serve_one(pipe, &statuses, &monitors) {
            log.line(&format!("pipe: {e:#}"));
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
        // SAFETY: `pipe` is the handle from `CreateNamedPipeW` above, closed by nobody else.
        drop(unsafe { Handle::from_raw(pipe) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_checked_fields_reach_the_command_line() {
        let s = Settings {
            server: " lumen.example:7443 ".into(),
            machine: "WPC-10".into(),
            manual_addrs: "10.0.0.2:9000, junk;192.168.1.5:9001".into(),
            server_ca_pem: String::new(),
            server_fingerprint: String::new(),
        };
        let args = worker_args(Path::new(r"C:\x"), &s);
        assert_eq!(
            args,
            [
                "--server",
                "lumen.example:7443",
                "--service",
                "--status-json",
                "--machine",
                "WPC-10",
                "--addr",
                "10.0.0.2:9000",
                "--addr",
                "192.168.1.5:9001"
            ]
        );
        assert!(check(&s).is_ok());
        let dash = Settings {
            server: "--evil".into(),
            ..s.clone()
        };
        assert_eq!(check(&dash).unwrap_err().to_string(), "err_leading_dash");
        let empty = Settings {
            server: " ".into(),
            ..s.clone()
        };
        assert_eq!(check(&empty).unwrap_err().to_string(), "err_server_empty");
        // A trusted key goes as one argument, in the canonical form.
        let pinned = Settings {
            server_fingerprint: "9500ef55438e47f8f47291b2ce45dcd6998a8f06edbc34bf914b13e599dfe36a"
                .into(),
            ..s.clone()
        };
        assert!(check(&pinned).is_ok());
        let args = worker_args(Path::new(r"C:\x"), &pinned);
        let at = args.iter().position(|a| a == "--server-pin").unwrap();
        assert_eq!(
            args[at + 1],
            "9500 EF55 438E 47F8 F472 91B2 CE45 DCD6 998A 8F06 EDBC 34BF 914B 13E5 99DF E36A"
        );
        let junk = Settings {
            server_fingerprint: "-1234".into(),
            ..s
        };
        assert_eq!(
            check(&junk).unwrap_err().to_string(),
            "err_cert_fingerprint"
        );
    }

    #[test]
    fn a_registration_keeps_its_server_and_its_token_outlives_it() {
        let old = Settings {
            server: "lumen.example:7443".into(),
            ..Settings::default()
        };
        let renamed = Settings {
            machine: "WPC-10".into(),
            ..old.clone()
        };
        assert!(same_server(None, &old).is_ok(), "a new registration");
        assert!(same_server(Some(&old), &renamed).is_ok());
        let moved = Settings {
            server: "other.example:7443".into(),
            ..old.clone()
        };
        assert!(same_server(Some(&old), &moved).is_err());

        let dir = std::env::temp_dir().join(format!("lumen-registry-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        for f in [SETTINGS, CREDS, SERVER_CA, TOKEN] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        forget_here(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["host.token", "logs"]);
        assert!(read_settings(&dir).is_none(), "not registered any more");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_status_of_an_old_run_is_ignored() {
        let shared = Shared::default();
        let old = shared.started();
        let new = shared.started();
        shared.event(old, &serde_json::json!({"event": "online"}));
        assert_eq!(shared.status().unwrap().state, "starting");
        shared.event(new, &serde_json::json!({"event": "online"}));
        shared.exited(old, "killed for a restart");
        assert_eq!(shared.status().unwrap().state, "online");
        shared.exited(new, "");
        assert_eq!(shared.status().unwrap().detail, "err_host_exited");
        shared.stopped();
        assert!(shared.status().is_none());
        shared.ask_restart();
        assert!(shared.take_restart() && !shared.take_restart());
    }
}
