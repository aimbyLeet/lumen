//! The host updated from its server.
//!
//! The worker (`station`) does what needs the server: it asks the update door
//! for the server's version (Lumen Host's yellow footer), and when a newer
//! client wants it (`UpdateWanted`), when the server refuses its signaling
//! version, or when the user clicked the footer (`now.json`, written by the
//! service, which restarts the worker), it downloads the installer, checks
//! it, tells the server it is updating and asks the service to run it.
//!
//! The service runs the installer (the worker lives in a job that dies with
//! the service): silently, for all users, and the service started again after
//! it whatever the installer did. After the update the service brings the
//! tray back for the signed-in user; the worker finds out whether it worked
//! (`pending.json`: the version it went for) and tells the server if not.
//!
//! Files: `%ProgramData%\Lumen\update.json` — the machine's switch
//! «Обновляться, когда подключается клиент новее» (the service writes it);
//! `<registration>\updates\` — installers, `now.json`, `pending.json`,
//! `install.log` (SYSTEM writes, users read).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use signal::update::{Offer, download, query};
use signal::{APP_VERSION, HostAgent, Trust, UpdateStep};

use crate::app::status;

/// Waits on the server's door (each, during a download).
const DOOR_WAIT: Duration = Duration::from_secs(10);

/// The service writes it on the footer's click ([`Now`]).
const NOW: &str = "now.json";
/// The worker writes it before the installer runs ([`Pending`]).
const PENDING: &str = "pending.json";

/// The `@status` event of a failed update (Lumen Host's footer says why).
pub fn failed_event(code: &str) -> serde_json::Value {
    serde_json::json!({ "event": "update_failed", "code": code })
}

/// The update failed: the server and Lumen Host are told.
fn fail(agent: &HostAgent, code: &str) {
    agent.update_state(UpdateStep::Failed, code);
    status(failed_event(code));
}

/// The machine's update switch.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Switch {
    /// Update when a newer client connects (on by default).
    pub auto: bool,
}

impl Default for Switch {
    fn default() -> Self {
        Switch { auto: true }
    }
}

fn switch_path() -> PathBuf {
    desktop::hostsvc::program_data().join("update.json")
}

/// The switch as the service last wrote it (on if never).
pub fn auto() -> bool {
    auto_in(&switch_path())
}

fn auto_in(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Switch>(&t).ok())
        .unwrap_or_default()
        .auto
}

/// The service: sets the switch.
pub fn set_auto(on: bool) -> Result<()> {
    let path = switch_path();
    std::fs::write(&path, serde_json::to_string(&Switch { auto: on })?)
        .with_context(|| format!("write {}", path.display()))
}

/// `<registration>\updates`.
pub fn updates_dir() -> PathBuf {
    desktop::hostsvc::registration_dir().join("updates")
}

/// The user clicked the footer: the worker updates when it starts.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Now {}

/// An update under way: the version the installer was run for.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pending {
    pub version: String,
    /// Lumen Host's window opens again after (else only the tray).
    pub reopen: bool,
}

fn read<T: for<'a> Deserialize<'a>>(dir: &Path, name: &str) -> Option<T> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn write<T: Serialize>(dir: &Path, name: &str, v: &T) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::write(dir.join(name), serde_json::to_string(v)?)
        .with_context(|| format!("write {name}"))
}

fn remove(dir: &Path, name: &str) {
    let _ = std::fs::remove_file(dir.join(name));
}

/// The service, on the footer's click: the worker is restarted and updates.
pub fn ask_now() -> Result<()> {
    write(&updates_dir(), NOW, &Now {})
}

/// The service at its start: an update just happened (or was tried).
pub fn pending() -> Option<Pending> {
    read(&updates_dir(), PENDING)
}

/// Why the worker updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A newer client wants to connect: only with the switch on.
    Wanted,
    /// The server refuses this signaling version: only with the switch on.
    Refused,
    /// The user clicked the footer.
    Clicked,
}

/// The worker's side.
pub struct Updater {
    server: String,
    trust: Trust,
    /// Installers, `now.json`, `pending.json` ([`updates_dir`]).
    dir: PathBuf,
    /// The machine's switch ([`auto`]).
    switch: PathBuf,
}

impl Updater {
    pub fn new(server: &str, trust: Trust) -> Updater {
        Updater::at(server, trust, updates_dir(), switch_path())
    }

    /// With its files elsewhere (checks).
    fn at(server: &str, trust: Trust, dir: PathBuf, switch: PathBuf) -> Updater {
        Updater {
            server: server.to_string(),
            trust,
            dir,
            switch,
        }
    }

    /// Logged in: what the last update did (told to the server if it did
    /// not work), the server's version for Lumen Host, and the update the
    /// user clicked for, if any.
    pub fn online(&mut self, agent: &HostAgent) {
        if let Some(p) = read::<Pending>(&self.dir, PENDING) {
            remove(&self.dir, PENDING);
            if p.version == APP_VERSION {
                println!("update: now Lumen {APP_VERSION}");
            } else {
                println!(
                    "update: still Lumen {APP_VERSION}, not {} (install.log)",
                    p.version
                );
                fail(agent, "err_update_install");
            }
        }
        let offer = self.ask();
        if read::<Now>(&self.dir, NOW).is_some() {
            remove(&self.dir, NOW);
            self.update(agent, Why::Clicked, offer);
        }
    }

    /// The server's version, told to Lumen Host (through the service).
    fn ask(&mut self) -> Option<Offer> {
        match query(&self.server, &self.trust, DOOR_WAIT) {
            Ok(offer) => {
                status(serde_json::json!({
                    "event": "server_version",
                    "version": offer.as_ref().map(|o| o.version.clone()).unwrap_or_default(),
                }));
                offer
            }
            Err(e) => {
                println!("update: the server's door did not answer ({e})");
                None
            }
        }
    }

    /// Updates if the server has a newer version and this is let to: the
    /// installer downloaded and checked, the server told, the service asked
    /// to run it (it stops this process). False: nothing to do, or failed
    /// (the server is told).
    pub fn update(&mut self, agent: &HostAgent, why: Why, offer: Option<Offer>) -> bool {
        if why != Why::Clicked && !auto_in(&self.switch) {
            println!("update: wanted ({why:?}), but this machine does not update by itself");
            agent.update_state(UpdateStep::Off, "");
            return false;
        }
        let offer = match offer {
            Some(o) => Some(o),
            None => self.ask(),
        };
        let Some(offer) = offer.filter(Offer::is_newer) else {
            println!("update: the server has nothing newer than Lumen {APP_VERSION}");
            return false;
        };
        // Clicked in Lumen Host's window: it opens again after.
        let reopen = why == Why::Clicked;
        status(serde_json::json!({ "event": "updating", "version": offer.version }));
        println!("update ({why:?}): downloading Lumen {}", offer.version);
        let setup = match download(&self.server, &self.trust, &offer, &self.dir, DOOR_WAIT) {
            Ok(p) => p,
            Err(e) => {
                println!("update: {e}");
                fail(agent, e.code());
                return false;
            }
        };
        if let Err(e) = write(
            &self.dir,
            PENDING,
            &Pending {
                version: offer.version.clone(),
                reopen,
            },
        ) {
            println!("update: {e:#}");
            fail(agent, "err_update_disk");
            return false;
        }
        agent.update_state(UpdateStep::Updating, "");
        // The agent sends it within its poll; then the service may stop us.
        std::thread::sleep(Duration::from_millis(1500));
        println!(
            "update: installing Lumen {} ({})",
            offer.version,
            setup.display()
        );
        status(serde_json::json!({ "event": "update_ready", "path": setup }));
        true
    }
}

/// The service: runs the installer the worker downloaded — only one in the
/// updates folder, named as an installer — silently for all users, and starts
/// the service again after it, whatever it did.
pub fn run_installer(path: &Path) -> Result<()> {
    let dir = updates_dir();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if path.parent() != Some(dir.as_path())
        || !name.starts_with("lumen-setup-")
        || !name.ends_with(".exe")
    {
        bail!("not an installer of the updates folder: {}", path.display());
    }
    use desktop::update::{
        BREAKAWAY, DETACHED, NEW_GROUP, NO_WINDOW, Part, installer_line, start_cmd,
    };
    let line = installer_line(
        path,
        Part::Host,
        &dir.join("install.log"),
        &format!("sc.exe start {}", desktop::hostsvc::SERVICE),
    );
    // Out of the service's job if Windows lets it.
    let flags = NO_WINDOW | NEW_GROUP | DETACHED;
    start_cmd(&line, flags | BREAKAWAY)
        .or_else(|_| start_cmd(&line, flags))
        .context("start the installer")?;
    Ok(())
}

/// The service at its start after an update: Lumen Host for the user signed
/// in on the console (the installer closed it) — the tray, or its window if
/// the update was clicked there.
pub fn bring_back_lumen_host(reopen: bool) -> Result<()> {
    let Some(user) = crate::user_files::ConsoleUser::get() else {
        return Ok(()); // nobody signed in: Windows starts the tray at sign-in
    };
    let exe = std::env::current_exe()?.with_file_name("lumen-host-app.exe");
    let cmd = if reopen {
        format!("\"{}\"", exe.display())
    } else {
        format!("\"{}\" --hidden", exe.display())
    };
    crate::winsec::start_as_user(user.token(), &cmd)?;
    Ok(())
}

/// The worker's side against a real server on loopback with a test
/// installer: asked by a newer client, it downloads, says «updating» and
/// asks for the installer; with the switch off it says so; a damaged
/// download is a failure the server hears of; after the update, a version
/// that did not change is a failure too.
#[cfg(test)]
mod tests {
    use std::time::Instant;

    use lumen_server::update::UpdateFile;
    use lumen_server::{Config, Server, dev_cert, tls_config};
    use signal::{Credentials, HostEvent, HostInfo, ServerConfig, SignalClient, SignalMessage};

    use super::*;

    const T: Duration = Duration::from_secs(5);

    struct Bench {
        dir: PathBuf,
        server: Server,
        trust: Trust,
    }

    impl Bench {
        fn new(name: &str) -> Bench {
            let dir = std::env::temp_dir()
                .join(format!("lumen-host-update-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("offer")).unwrap();
            let data: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
            std::fs::write(dir.join("offer").join("lumen-setup-99.0.0.exe"), data).unwrap();
            let cert = dev_cert(&["localhost".into(), "127.0.0.1".into()]).unwrap();
            let ca = dir.join("ca.pem");
            std::fs::write(&ca, &cert.ca_pem).unwrap();
            let server = Server::start(Config {
                update: UpdateFile::find(&dir.join("offer")).unwrap(),
                ..Config::new(
                    "127.0.0.1:0".parse().unwrap(),
                    dir.join("lumen.db"),
                    tls_config(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap(),
                )
            })
            .unwrap();
            let trust = Trust::from_pem_file(&ca).unwrap();
            Bench { dir, server, trust }
        }

        fn addr(&self) -> String {
            self.server.addr().to_string()
        }

        /// A registered host's agent, online, and its updater.
        fn host(&self) -> (HostAgent, Updater, Credentials) {
            let agent = HostAgent::start(
                ServerConfig {
                    server: self.addr(),
                    trust: self.trust.clone(),
                },
                None,
                [5; 32],
                HostInfo {
                    machine: "WPC-10".into(),
                    account: String::new(),
                    lookup: [0x11; 32],
                    addrs: vec!["127.0.0.1:9000".into()],
                    active: true,
                    auto_update: true,
                },
            );
            let mut creds = None;
            let deadline = Instant::now() + T;
            while Instant::now() < deadline {
                match agent.events.recv_timeout(Duration::from_millis(100)) {
                    Ok(HostEvent::Registered(c)) => creds = Some(c),
                    Ok(HostEvent::Online { .. }) => break,
                    _ => {}
                }
            }
            let u = Updater::at(
                &self.addr(),
                self.trust.clone(),
                self.dir.join("updates"),
                self.dir.join("update.json"),
            );
            (agent, u, creds.expect("registered"))
        }

        /// What the server says of the host, as a client sees it: (presence, update_failed).
        fn seen(&self) -> (signal::Presence, String) {
            let mut c = SignalClient::connect(&self.addr(), &self.trust, T).unwrap();
            c.register(signal::Role::Client, "anna", [0xA; 32], T)
                .unwrap();
            c.send(&SignalMessage::Status {
                lookups: vec![[0x11; 32]],
            })
            .unwrap();
            loop {
                if let SignalMessage::Cards { cards, .. } = c.expect(T).unwrap() {
                    let h = cards[0].host.clone().unwrap();
                    return (h.presence, h.update_failed);
                }
            }
        }
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_newer_client_wanted_it() {
        let b = Bench::new("wanted");
        let (agent, mut u, _) = b.host();
        assert!(u.update(&agent, Why::Wanted, None));
        let setup = b.dir.join("updates").join("lumen-setup-99.0.0.exe");
        assert!(setup.exists());
        let p: Pending = read(&b.dir.join("updates"), PENDING).unwrap();
        assert_eq!((p.version.as_str(), p.reopen), ("99.0.0", false));
        assert_eq!(b.seen().0, signal::Presence::Updating);
        // Only the updates folder's installers are run.
        assert!(run_installer(&b.dir.join("offer").join("lumen-setup-99.0.0.exe")).is_err());

        // Back on the same version: the update did not work, the server hears it.
        u.online(&agent);
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(b.seen().1, "err_update_install");
        assert!(read::<Pending>(&b.dir.join("updates"), PENDING).is_none());
    }

    #[test]
    fn the_switch_off_or_a_damaged_download() {
        let b = Bench::new("off");
        let (agent, mut u, _) = b.host();
        std::fs::write(b.dir.join("update.json"), r#"{"auto":false}"#).unwrap();
        assert!(!u.update(&agent, Why::Wanted, None));
        assert!(!b.dir.join("updates").join(PENDING).exists());
        // The click updates whatever the switch: here, a damaged download.
        std::fs::create_dir_all(b.dir.join("updates")).unwrap();
        let mut bad = std::fs::read(b.dir.join("offer").join("lumen-setup-99.0.0.exe")).unwrap();
        bad[7] ^= 0xFF;
        std::fs::write(
            b.dir.join("updates").join("lumen-setup-99.0.0.exe.part"),
            &bad[..100_000],
        )
        .unwrap();
        write(&b.dir.join("updates"), NOW, &Now {}).unwrap();
        u.online(&agent);
        assert!(!b.dir.join("updates").join(NOW).exists(), "once");
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(b.seen().1, "err_update_sha");
        assert!(!b.dir.join("updates").join(PENDING).exists());
    }
}
