//! Talking to the "Lumen Host" service: one request,
//! one answer, a JSON line each way over the named pipe [`PIPE`].
//!
//! The registration is the machine's (one key, one token), whoever is signed
//! in: any signed-in user of the machine sees and changes it.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PIPE: &str = r"\\.\pipe\lumen-host";

/// `%ProgramData%\Lumen`: what the service keeps (it alone writes; signed-in users read).
pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
        .join("Lumen")
}

/// The machine's registration: `%ProgramData%\Lumen\host`.
pub fn registration_dir() -> PathBuf {
    program_data().join("host")
}

/// The registration's settings, as the user sets them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `host:port` of the Lumen server.
    pub server: String,
    /// CA of the server's certificate (PEM text; development servers), or empty.
    pub server_ca_pem: String,
    /// The key the user trusted, of a server with its own certificate (a local
    /// network; `signal::pin`), or empty: the system's check.
    pub server_fingerprint: String,
    /// This machine's name as clients see it (empty: the computer name).
    pub machine: String,
    /// Announce these `ip:port` (comma separated) instead of the machine's addresses.
    pub manual_addrs: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// What the service has.
    State,
    /// Creates the registration if there is none (its host registers on the
    /// server), or sets its settings and restarts its host. The server of a
    /// registration does not change: [`Request::Unregister`] first.
    Set { settings: Settings },
    /// A new token: clients with the old one can no longer connect.
    NewToken,
    /// Removes the machine's registration with its token (the server keeps
    /// its record until the admin deletes it).
    Remove,
    /// «Удалить»: the host stops, the server is asked to remove its record
    /// (if it can be reached), the registration goes; the token stays (a new
    /// registration keeps the clients' cards right). [`State::note`] says
    /// what became of the server's record.
    Unregister,
    /// Forgets the host's registration on the server (the admin removed it):
    /// the host registers anew.
    Reregister,
    /// Ctrl+Alt+Del on this machine (a client asked). Only from the service's
    /// own hosts (SYSTEM): `SendSAS` works only in a service.
    SecureAttention,
    /// Turns the host off: the service stops (after locking Windows, if a
    /// client is connected) until someone starts it again ([`start_service`],
    /// Lumen Host) or Windows starts it with itself ([`Request::SetAutostart`]).
    Stop,
    /// Whether the service starts with Windows (before anyone signs in).
    SetAutostart { on: bool },
    /// «Не давать компьютеру засыпать»: while it is on and the service runs,
    /// Windows does not go to sleep on its idle timer.
    SetKeepAwake { on: bool },
    /// «Обновляться, когда подключается клиент новее».
    SetAutoUpdate { on: bool },
    /// The footer's click, confirmed: the host is restarted (a session is cut
    /// off) and updates to the server's newer version; Lumen Host closes and
    /// the service opens it again after.
    UpdateNow,
    /// Virtual monitors (Lumen VDD): «Создать» / «Применить», «Отключить» /
    /// «Включить» (`enabled`). The service keeps them while it runs.
    SetMonitors { settings: vdd::layout::Settings },
    /// «Сбросить»: the virtual monitors go, their settings too.
    ResetMonitors,
    /// Lumen Host on a host without a physical monitor, in session: the set
    /// of client `client` (key, hex) becomes `set`. Only while its set is up
    /// (another client by now: `err_client_monitors_gone`).
    SetClientMonitors {
        client: String,
        set: vdd::layout::ClientSet,
    },
    /// From the service's own hosts (SYSTEM): what the
    /// host sees — whether Windows has a physical monitor, and the client in
    /// session (its key, hex), if any. `lost`: the session that ended was
    /// cut off (its monitors stay a while for the client to come back), not
    /// closed. The answer's [`Monitors::client`] says what the service did.
    HostMonitors {
        physical: bool,
        client: Option<String>,
        lost: bool,
    },
    /// From the service's own hosts (SYSTEM): the monitor client `client`
    /// chose when asked (it had no set yet; one that has is kept).
    ClientChoice {
        client: String,
        width: u32,
        height: u32,
        scale: u16,
    },
}

impl Request {
    /// Only the service's own hosts (SYSTEM) may ask this.
    pub fn system_only(&self) -> bool {
        matches!(
            self,
            Request::SecureAttention | Request::HostMonitors { .. } | Request::ClientChoice { .. }
        )
    }
}

/// The machine's registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The machine has a registration.
    pub registered: bool,
    pub settings: Settings,
    pub token: String,
    /// Its host process, as its `@status` lines tell.
    pub status: HostStatus,
    /// The service starts with Windows.
    pub autostart: bool,
    /// The service keeps the computer from sleeping.
    pub keep_awake: bool,
    /// The host updates itself when a newer client connects.
    pub auto_update: bool,
    /// What the request did, for the user (empty: nothing to say).
    pub note: String,
    /// The virtual monitors.
    pub monitors: Monitors,
}

/// The machine's virtual monitors (Lumen VDD), as the service keeps them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Monitors {
    /// Lumen VDD is installed (no driver: no «Мониторы» in Lumen Host).
    pub installed: bool,
    /// Its version, once the service has opened it.
    pub driver: String,
    pub settings: vdd::layout::Settings,
    /// The virtual monitors up now.
    pub up: Vec<MonitorUp>,
    /// What went wrong last (empty: nothing).
    pub error: String,
    /// Windows has a physical monitor, as the host last told (true until it
    /// does): the machine's virtual monitors are up only then.
    pub physical: bool,
    /// No physical monitor and a client in session: its own monitors.
    pub client: Option<ClientMonitors>,
}

/// A client's own monitors on a host without any.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientMonitors {
    /// Its key, hex.
    pub key: String,
    /// Up as this says; `None`: it has none yet — the host asks it.
    pub set: Option<vdd::layout::ClientSet>,
    /// Nobody in session: the last client's set, parked (kept in the
    /// driver; on in Windows only with no other display).
    pub parked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorUp {
    /// 1 (a client's own monitor 1), 2 or 3.
    pub monitor: u8,
    pub width: u32,
    pub height: u32,
}

/// What Lumen Host shows about a registration's host.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostStatus {
    /// stopped, starting, online, offline, refused, session, error.
    pub state: String,
    /// Why (offline, error), or how the last session ended. Refused: the
    /// server's reason — `BadCredentials` (it does not know this host),
    /// `TokenTaken` (another host has its token there), `BadName`, `Version`.
    pub detail: String,
    /// In a session: the client, its frame rate and cap (0 = auto), since when (Unix time).
    pub client: String,
    pub fps: u16,
    pub mbps: f64,
    pub since: u64,
    /// `ip:port` the host announces.
    pub addrs: Vec<String>,
    /// The server's version, as its update door said ("": not known yet, or
    /// no installer there).
    pub server_version: String,
    /// The version being installed now ("": no update under way).
    pub updating: String,
    /// The last update failed: its code (`err_update_…`).
    pub update_error: String,
}

impl HostStatus {
    pub fn stopped() -> HostStatus {
        HostStatus {
            state: "stopped".into(),
            ..HostStatus::default()
        }
    }

    pub fn starting() -> HostStatus {
        HostStatus {
            state: "starting".into(),
            ..HostStatus::stopped()
        }
    }

    /// Takes one `@status` event of the host process (apps/host).
    pub fn apply(&mut self, v: &Value) {
        let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        match v["event"].as_str().unwrap_or_default() {
            "online" => {
                self.state = "online".into();
                self.detail.clear();
            }
            "offline" => {
                self.state = "offline".into();
                self.detail = text("reason");
                self.client.clear();
            }
            "refused" => {
                self.state = "refused".into();
                self.detail = text("reason");
            }
            "listening" => {
                self.addrs = v["addrs"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "session" => {
                self.state = "session".into();
                self.client = text("client");
                self.fps = v["fps"].as_u64().unwrap_or(0) as u16;
                self.mbps = v["mbps"].as_f64().unwrap_or(0.0);
                self.since = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
            }
            "session_end" => {
                if self.state == "session" {
                    self.state = "online".into();
                }
                self.detail = format!("note_last_session|{}|{}", self.client, text("reason"));
                self.client.clear();
            }
            "server_version" => self.server_version = text("version"),
            "updating" => {
                self.updating = text("version");
                self.update_error.clear();
            }
            "update_failed" => {
                self.updating.clear();
                self.update_error = text("code");
            }
            _ => {}
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok(Box<State>),
    Error(String),
}

/// Sends `req` to the service, waits for its answer.
pub fn call(req: &Request) -> Result<State, String> {
    let mut pipe = open().map_err(|e| format!("err_service_down|{e}"))?;
    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    pipe.write_all(line.as_bytes())
        .map_err(|e| format!("err_service|{e}"))?;
    let mut answer = String::new();
    BufReader::new(pipe)
        .read_line(&mut answer)
        .map_err(|e| format!("err_service|{e}"))?;
    match serde_json::from_str(&answer) {
        Ok(Response::Ok(s)) => Ok(*s),
        Ok(Response::Error(e)) => Err(e),
        Err(e) => Err(format!("err_service_answer|{e}")),
    }
}

/// The service, as the Service Control Manager tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Running,
    /// Starting or stopping.
    Pending,
    Stopped,
    /// Not installed (or not ours to look at).
    Missing,
}

/// The service's name for the Service Control Manager.
pub const SERVICE: &str = "LumenHost";

/// Opens the service with `access`, calls `f`, closes it.
fn with_service<T>(
    access: u32,
    f: impl FnOnce(windows::Win32::System::Services::SC_HANDLE) -> T,
) -> Option<T> {
    use windows::Win32::System::Services::*;
    use windows::core::PCWSTR;
    let name: Vec<u16> = SERVICE.encode_utf16().chain([0]).collect();
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
        let svc = OpenServiceW(scm, PCWSTR(name.as_ptr()), access);
        let r = svc.as_ref().ok().map(|s| f(*s));
        if let Ok(s) = svc {
            let _ = CloseServiceHandle(s);
        }
        let _ = CloseServiceHandle(scm);
        r
    }
}

/// Where the service is now.
pub fn service() -> Service {
    use windows::Win32::System::Services::*;
    with_service(SERVICE_QUERY_STATUS, |svc| {
        let mut st = SERVICE_STATUS::default();
        match unsafe { QueryServiceStatus(svc, &mut st) } {
            Err(_) => Service::Missing,
            Ok(()) if st.dwCurrentState == SERVICE_RUNNING => Service::Running,
            Ok(()) if st.dwCurrentState == SERVICE_STOPPED => Service::Stopped,
            Ok(()) => Service::Pending,
        }
    })
    .unwrap_or(Service::Missing)
}

/// Turns the host on: starts the service (any signed-in user may: the
/// installer lets them) and waits until its pipe answers.
pub fn start_service() -> Result<(), String> {
    use windows::Win32::System::Services::*;
    let started = with_service(SERVICE_START | SERVICE_QUERY_STATUS, |svc| unsafe {
        StartServiceW(svc, None)
    })
    .ok_or("err_service_missing")?;
    if let Err(e) = started
        && e.code() != windows::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING.to_hresult()
    {
        return Err(format!("err_service_start|{e}"));
    }
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        if call(&Request::State).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("err_service_silent".into())
}

/// Opens the pipe; while the service is busy with another caller, waits a little.
fn open() -> std::io::Result<std::fs::File> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE)
        {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers_are_json_lines() {
        let r = Request::Set {
            settings: Settings {
                server: "lumen.example:7443".into(),
                ..Settings::default()
            },
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.starts_with(r#"{"op":"set""#), "{s}");
        assert!(!s.contains('\n'));
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(
            matches!(back, Request::Set { settings } if settings.server == "lumen.example:7443")
        );
        let a = serde_json::to_string(&Response::Error("нет".into())).unwrap();
        assert_eq!(a, r#"{"error":"нет"}"#);
        let st: Response = serde_json::from_str(r#"{"ok":{"registered":true}}"#).unwrap();
        assert!(matches!(st, Response::Ok(s) if s.registered));
    }

    #[test]
    fn status_follows_the_host_events() {
        let mut s = HostStatus::starting();
        s.apply(
            &serde_json::json!({"event": "listening", "addrs": ["10.0.0.2:9000"], "active": false}),
        );
        s.apply(&serde_json::json!({"event": "online"}));
        assert_eq!(s.state, "online");
        assert_eq!(s.addrs, ["10.0.0.2:9000"]);
        s.apply(&serde_json::json!({"event": "session", "client": "anna", "fps": 60, "mbps": 0}));
        assert_eq!(
            (s.state.as_str(), s.client.as_str(), s.fps),
            ("session", "anna", 60)
        );
        s.apply(&serde_json::json!({"event": "session_end", "reason": "Bye"}));
        assert_eq!(s.state, "online");
        assert_eq!(s.detail, "note_last_session|anna|Bye");
    }
}
