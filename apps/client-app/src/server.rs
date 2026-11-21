//! The link to this client's one server (one server
//! per client, set in the welcome window): this client registered there the
//! first time, access asked, the cards' state polled, the last card taking
//! this client off it.

use std::path::{Path, PathBuf};

use serde::Serialize;
use signal::{
    ClientLink, ConnectFailure, Credentials, Limits, LoginError, LoginFailure, ServerConfig, Trust,
};
use transport::HostToken;

use crate::App;
use crate::card::{Card, CardLive, access_text, presence_text};

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ServerLive {
    /// What the admin sets for this client there, each on its own.
    pub(crate) forced: LimitsJson,
    /// Why the server cannot be reached / refuses this client.
    pub(crate) error: String,
    /// The server does not know this client (its database was reset, the
    /// admin removed it): «Запросить подключение» registers it anew.
    pub(crate) unknown: bool,
}

impl ServerLive {
    /// Linked: what the server forces now, no error.
    fn linked(&mut self, forced: Limits) {
        self.forced = forced.into();
        self.error.clear();
        self.unknown = false;
    }
}

/// Why there is no link to the server.
#[derive(Debug)]
pub(crate) enum LinkError {
    /// It does not know this client.
    Unknown,
    Other(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Unknown => write!(f, "err_unknown_device"),
            LinkError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl From<LinkError> for String {
    fn from(e: LinkError) -> String {
        e.to_string()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct LimitsJson {
    pub(crate) fps: Option<u16>,
    pub(crate) mbps: Option<f32>,
}

impl From<Limits> for LimitsJson {
    fn from(l: Limits) -> Self {
        LimitsJson {
            fps: l.fps,
            mbps: l.mbps,
        }
    }
}

/// The poll thread's link, with the server it is to.
pub(crate) type PollLink = Option<(String, ClientLink)>;

impl App {
    /// The server (`host:port`) and its CA file (development; usually empty).
    pub(crate) fn server(&self) -> (String, String) {
        let s = self.settings.lock().unwrap();
        (s.server.clone(), s.server_ca.clone())
    }

    /// The server and how it is checked; `err_server_required` before the
    /// welcome window.
    pub(crate) fn server_config(&self) -> Result<ServerConfig, String> {
        let (server, ca) = self.server();
        if server.is_empty() {
            return Err("err_server_required".into());
        }
        self.config(&server, &ca)
    }

    /// How `server` is checked: the CA file (development), the key the user
    /// trusted (a server with its own certificate), or the system.
    pub(crate) fn config(&self, server: &str, ca: &str) -> Result<ServerConfig, String> {
        let ca = ca.trim();
        let trust = Trust::choose(
            (!ca.is_empty()).then_some(Path::new(ca)),
            self.pin(server).as_deref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(ServerConfig {
            server: server.to_string(),
            trust,
        })
    }

    /// The key the user trusted for `server` (next to its credentials).
    pub(crate) fn pin_path(&self, server: &str) -> PathBuf {
        self.creds_path(server).with_extension("pin")
    }

    pub(crate) fn pin(&self, server: &str) -> Option<String> {
        std::fs::read_to_string(self.pin_path(server))
            .ok()
            .and_then(|t| signal::pin::normalize_fingerprint(&t))
    }

    /// The user trusts the key with this fingerprint for `server` (they saw it
    /// in `err_cert_untrusted` / `err_cert_changed`).
    pub(crate) fn trust_server(&self, server: &str, fingerprint: &str) -> Result<(), String> {
        let server = signal::server_address(server)?;
        let fp = signal::pin::normalize_fingerprint(fingerprint)
            .ok_or_else(|| "err_cert_fingerprint".to_string())?;
        let path = self.pin_path(&server);
        let _ = std::fs::create_dir_all(path.parent().expect("in servers"));
        std::fs::write(&path, format!("{fp}\n")).map_err(|e| e.to_string())?;
        self.log
            .line(&format!("--- trusted the key of {server}: {fp}"));
        Ok(())
    }

    /// Whether `server` answers (the welcome window and the settings, before
    /// keeping it). A server with its own certificate not trusted yet:
    /// `err_cert_untrusted|<fingerprint>`.
    pub(crate) fn probe(&self, server: &str, ca: &str) -> Result<(), String> {
        let cfg = self.config(server, ca)?;
        // A certificate's question and a bad fingerprint come as their codes (the last arm).
        signal::probe(&cfg).map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                "err_probe_timeout".into()
            }
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset => {
                "err_probe_closed".into()
            }
            _ => e.to_string(),
        })
    }

    /// The card's view of its machine, remembered by the video window
    /// (`lumen-client --view-memory`): mode and windows of several monitors.
    pub(crate) fn view_path(&self, id: u64) -> PathBuf {
        self.data.join("views").join(format!("{id}.json"))
    }

    /// This client's credentials on `server` (one file per server).
    pub(crate) fn creds_path(&self, server: &str) -> PathBuf {
        let safe: String = server
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.data.join("servers").join(format!("{safe}.creds"))
    }

    pub(crate) fn creds(&self, server: &str) -> Option<Credentials> {
        std::fs::read_to_string(self.creds_path(server))
            .ok()
            .and_then(|t| t.parse().ok())
    }

    /// Logs in to the server, registering this client there the first time.
    pub(crate) fn link(&self, name: &str) -> Result<ClientLink, LinkError> {
        let cfg = self.server_config().map_err(LinkError::Other)?;
        let server = &cfg.server;
        let path = self.creds_path(server);
        let creds = self.creds(server);
        let link = ClientLink::open(&cfg, creds.as_ref(), name, self.key.public().0).map_err(
            |e| match e {
                LoginError::Rejected(LoginFailure::BadCredentials) => LinkError::Unknown,
                // As it is: the interface asks the user about the key.
                e if e.cert_problem().is_some() => LinkError::Other(e.to_string()),
                e => LinkError::Other(format!("{server}: {e}")),
            },
        )?;
        if creds.is_none() {
            let c = link.credentials();
            let _ = std::fs::create_dir_all(path.parent().expect("in servers"));
            std::fs::write(&path, format!("{c}\n")).map_err(|e| LinkError::Other(e.to_string()))?;
            self.log.line(&format!(
                "--- registered on {server} as client {}",
                c.device
            ));
        }
        Ok(link)
    }

    pub(crate) fn card(&self, id: u64) -> Result<Card, String> {
        self.settings
            .lock()
            .unwrap()
            .cards
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| "err_no_card".to_string())
    }

    /// Asks the server for access to the card's host; records what it says.
    /// Returns the host's machine and account. A server that does not know
    /// this client (its database was reset): registered anew, and the other
    /// cards ask too.
    pub(crate) fn ask(&self, card: &Card) -> Result<(String, String), String> {
        match self.ask_once(card) {
            Err(LinkError::Unknown) => {
                let (server, _) = self.server();
                self.log.line(&format!(
                    "--- {server} does not know this client: registering anew"
                ));
                let _ = std::fs::remove_file(self.creds_path(&server));
                let r = self.ask_once(card).map_err(String::from);
                let others: Vec<Card> = self
                    .settings
                    .lock()
                    .unwrap()
                    .cards
                    .iter()
                    .filter(|c| c.id != card.id)
                    .cloned()
                    .collect();
                for c in others {
                    let _ = self.ask_once(&c);
                }
                r
            }
            r => r.map_err(String::from),
        }
    }

    pub(crate) fn ask_once(&self, card: &Card) -> Result<(String, String), LinkError> {
        let token: HostToken = card.token.parse().map_err(LinkError::Other)?;
        let name = self.settings.lock().unwrap().name.clone();
        let mut link = self.link(&name)?;
        let (c, forced) = link
            .add_host(token.lookup())
            .map_err(|e| LinkError::Other(e.to_string()))?;
        link.close();
        self.live_server.lock().unwrap().linked(forced);
        self.record(card.id, &c);
        match c.host {
            Some(h) => Ok((h.machine, h.account)),
            None => Err(LinkError::Other("err_host_not_found".into())),
        }
    }

    /// Takes in a card's state from the server.
    pub(crate) fn record(&self, id: u64, c: &signal::Card) {
        let live = match &c.host {
            None => CardLive {
                found: Some(false),
                ..CardLive::default()
            },
            Some(h) => {
                let mut s = self.settings.lock().unwrap();
                if let Some(card) = s.cards.iter_mut().find(|x| x.id == id)
                    && (card.machine != h.machine || card.account != h.account)
                {
                    card.machine = h.machine.clone();
                    card.account = h.account.clone();
                    self.keep_settings(s);
                }
                CardLive {
                    found: Some(true),
                    access: access_text(h.access).into(),
                    presence: presence_text(h.presence).into(),
                    host_version: h.version.clone(),
                    update_failed: h.update_failed.clone(),
                    auto_update: h.auto_update,
                }
            }
        };
        self.cards.lock().unwrap().insert(id, live);
    }

    /// One round: the cards' state from the server.
    pub(crate) fn poll(&self, link: &mut PollLink) {
        let (cards, name, server) = {
            let s = self.settings.lock().unwrap();
            (s.cards.clone(), s.name.clone(), s.server.clone())
        };
        let list: Vec<(u64, [u8; 32])> = cards
            .iter()
            .filter_map(|c| {
                c.token
                    .parse::<HostToken>()
                    .ok()
                    .map(|t| (c.id, t.lookup()))
            })
            .collect();
        // No cards (or another server since): no link.
        let kept = link
            .take()
            .filter(|(s, _)| *s == server && !list.is_empty());
        if list.is_empty() {
            return;
        }
        let lookups: Vec<[u8; 32]> = list.iter().map(|(_, l)| *l).collect();
        let fresh = kept.is_none();
        let result = match kept {
            Some((_, l)) => Ok(l),
            None => self.link(&name),
        }
        .and_then(|mut l| match l.status(&lookups) {
            Ok(r) => Ok((l, r)),
            Err(e) => Err(LinkError::Other(format!("{server}: {e}"))),
        });
        match result {
            Ok((l, (found, forced))) => {
                for ((id, _), c) in list.iter().zip(&found) {
                    self.record(*id, c);
                }
                self.live_server.lock().unwrap().linked(forced);
                *link = Some((server, l));
                // A new link (at start, or the server came back, maybe updated):
                // what version it has now.
                if fresh {
                    self.check_update(crate::update::START_CHECK);
                }
            }
            Err(e) => {
                let mut s = self.live_server.lock().unwrap();
                s.unknown = matches!(e, LinkError::Unknown);
                s.error = e.to_string();
            }
        }
    }

    /// A new card: the host's token, on this client's server. Asks the admin for access.
    pub(crate) fn add_card(&self, token: String) -> Result<(), String> {
        {
            let s = self.settings.lock().unwrap();
            if s.name.is_empty() {
                return Err("err_name_required".into());
            }
            if s.server.is_empty() {
                return Err("err_server_required".into());
            }
        }
        let token: HostToken = token.parse().map_err(|_| "err_token".to_string())?;
        let token = token.to_string();
        let mut card = {
            let mut s = self.settings.lock().unwrap();
            if s.cards.iter().any(|c| c.token == token) {
                return Err("err_card_exists".into());
            }
            let card = Card {
                id: s.next_id,
                token,
                machine: String::new(),
                account: String::new(),
            };
            s.next_id += 1;
            card
        };
        // Kept only if the server knows the token.
        (card.machine, card.account) = self.ask(&card)?;
        let mut s = self.settings.lock().unwrap();
        s.cards.push(card.clone());
        self.keep_settings(s);
        self.log.line(&format!(
            "--- card {} added: {} / {}",
            card.id, card.machine, card.account
        ));
        Ok(())
    }

    /// Restarts the card's machine: the server tells its host (no questions
    /// asked there). Returns what to tell the user.
    pub(crate) fn reboot_card(&self, id: u64) -> Result<String, String> {
        let card = self.card(id)?;
        let token: HostToken = card.token.parse().map_err(|_| "err_token".to_string())?;
        let name = self.settings.lock().unwrap().name.clone();
        let mut link = self.link(&name).map_err(String::from)?;
        let answer = link.reboot(token.lookup());
        link.close();
        self.log
            .line(&format!("--- card {id}: restart asked ({answer:?})"));
        match answer {
            Ok(None) => {
                // At once, not at the next poll: no «Connect» to a machine going down.
                if let Some(live) = self.cards.lock().unwrap().get_mut(&id) {
                    live.presence = "restarting".into();
                }
                Ok(String::new())
            }
            Ok(Some(ConnectFailure::Offline)) => Err("err_reboot_offline".into()),
            Ok(Some(ConnectFailure::NotApproved)) => Err("err_reboot_not_approved".into()),
            Ok(Some(ConnectFailure::UnknownHost)) => Err("err_host_not_found".into()),
            Ok(Some(r)) => Err(format!("err_reboot_failed|{r:?}")),
            Err(e) => Err(format!("err_reboot_failed|{e}")),
        }
    }

    /// Deletes the card; the server forgets the access request. The last card:
    /// this client is removed there too (if the server answers) and forgets
    /// its registration; the server stays (the user may change it now).
    /// Returns what to tell the user (or nothing).
    pub(crate) fn remove_card(&self, id: u64) -> Result<String, String> {
        let card = self.card(id)?;
        let mut s = self.settings.lock().unwrap();
        s.cards.retain(|c| c.id != id);
        let last = s.cards.is_empty();
        let (name, server) = (s.name.clone(), s.server.clone());
        self.keep_settings(s);
        self.cards.lock().unwrap().remove(&id);
        let _ = std::fs::remove_file(self.view_path(id));
        self.log.line(&format!("--- card {id} removed"));
        // Never registers just to say goodbye.
        let link = match self.creds(&server) {
            Some(_) => self.link(&name),
            None => Err(LinkError::Unknown),
        };
        let forget_here = || {
            let _ = std::fs::remove_file(self.creds_path(&server));
            *self.live_server.lock().unwrap() = ServerLive::default();
        };
        let mut link = match link {
            Ok(l) => l,
            Err(e) => {
                if !last {
                    return Ok(String::new());
                }
                forget_here();
                return Ok(match e {
                    LinkError::Unknown => String::new(),
                    LinkError::Other(e) => format!("note_client_unreachable|{e}"),
                });
            }
        };
        // The access request goes too (a rejection stays on the server).
        if let Ok(t) = card.token.parse::<HostToken>() {
            let _ = link.forget(t.lookup());
        }
        if !last {
            link.close();
            return Ok(String::new());
        }
        let gone = link.unregister();
        forget_here();
        self.log.line(&format!(
            "--- the last card of {server}: this client removed there ({gone:?})"
        ));
        Ok(match gone {
            Ok(()) => String::new(),
            Err(e) => format!("note_client_unconfirmed|{e}"),
        })
    }
}

/// The cards against a real server on loopback: the server and the name set
/// first, a card asks, the last card takes this client off the server, a
/// server that forgot this client takes it back on «Запросить подключение».
#[cfg(test)]
mod registration_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use desktop::log::Log;
    use signal::Access;

    use super::*;
    use crate::card::{Button, button};
    use lumen_server::db::Db;
    use lumen_server::{Config, Server, dev_cert, tls_config};
    use signal::{Role, SignalClient, SignalMessage};

    const T: Duration = Duration::from_secs(5);

    /// A server with its own folder; restarted on the same port, with a
    /// fresh database if asked (a server moved to another VPS).
    struct Bench {
        dir: PathBuf,
        cert: lumen_server::DevCert,
        port: u16,
        server: Option<Server>,
    }

    impl Bench {
        fn new(name: &str) -> Bench {
            let dir = std::env::temp_dir()
                .join(format!("lumen-client-app-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("app")).unwrap();
            let cert = dev_cert(&["localhost".into(), "127.0.0.1".into()]).unwrap();
            std::fs::write(dir.join("ca.pem"), &cert.ca_pem).unwrap();
            let mut b = Bench {
                dir,
                cert,
                port: 0,
                server: None,
            };
            b.start(false);
            b
        }

        fn db_path(&self) -> PathBuf {
            self.dir.join("lumen.db")
        }

        fn start(&mut self, fresh_db: bool) {
            self.server = None;
            if fresh_db {
                std::fs::remove_file(self.db_path()).unwrap();
            }
            let server = Server::start(Config::new(
                format!("127.0.0.1:{}", self.port).parse().unwrap(),
                self.db_path(),
                tls_config(self.cert.cert_pem.as_bytes(), self.cert.key_pem.as_bytes()).unwrap(),
            ))
            .unwrap();
            self.port = server.addr().port();
            self.server = Some(server);
        }

        fn addr(&self) -> String {
            format!("127.0.0.1:{}", self.port)
        }

        fn ca(&self) -> String {
            self.dir.join("ca.pem").display().to_string()
        }

        fn db(&self) -> Db {
            Db::open(&self.db_path()).unwrap()
        }

        /// A host announced with `token`; its connection keeps it online.
        fn host(&self, token: &HostToken, key: u8) -> (SignalClient, u64) {
            let trust = Trust::from_pem_file(Path::new(&self.ca())).unwrap();
            let mut c = SignalClient::connect(&self.addr(), &trust, T).unwrap();
            let (creds, _) = c.register(Role::Host, "", [key; 32], T).unwrap();
            c.send(&SignalMessage::HostUpdate {
                machine: format!("WPC-{key}"),
                account: String::new(),
                lookup: token.lookup(),
                addrs: vec!["127.0.0.1:9000".into()],
                active: true,
                auto_update: true,
            })
            .unwrap();
            c.send(&SignalMessage::Ping { id: 1 }).unwrap();
            assert_eq!(c.recv(T).unwrap(), Some(SignalMessage::Pong { id: 1 }));
            (c, creds.device)
        }

        /// The app with this server's CA (development; the welcome window has no field for it).
        fn app(&self) -> App {
            let data = self.dir.join("app");
            let app = App::open(data.clone(), Arc::new(Log::new(data.join("client.log"))));
            app.settings.lock().unwrap().server_ca = self.ca();
            app
        }
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            self.server = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn client_id(app: &App, server: &str) -> Option<u64> {
        app.creds(server).map(|c| c.device)
    }

    #[test]
    fn cards_requests_and_a_server_that_forgot_this_client() {
        let mut b = Bench::new("cards");
        let (one, two) = (HostToken::generate(), HostToken::generate());
        let (_h1, host1) = b.host(&one, 1);
        let (_h2, host2) = b.host(&two, 2);
        let app = b.app();
        let srv = b.addr();

        // No name and server yet: no machines.
        assert_eq!(
            app.add_card(one.to_string()).unwrap_err(),
            "err_name_required"
        );
        assert_eq!(
            app.set_identity(String::new(), srv.clone()).unwrap_err(),
            "err_name_required"
        );
        assert!(
            app.set_identity("x".repeat(65), srv.clone()).is_err(),
            "over 64 characters"
        );
        assert_eq!(
            app.set_identity("Иван".into(), srv.clone()).unwrap_err(),
            "err_name_invalid",
            "Latin only"
        );
        assert_eq!(
            app.set_identity("Ivan".into(), "a b".into()).unwrap_err(),
            "err_server_address"
        );
        // Nobody there: not kept.
        let nobody = app.set_identity("Ivan".into(), "127.0.0.1:1".into());
        assert!(nobody.is_err());
        assert_eq!(app.server().0, "");
        app.set_identity("  Ivan  ".into(), srv.clone()).unwrap();
        assert_eq!(app.settings.lock().unwrap().name, "Ivan");
        assert_eq!(app.server().0, srv);
        assert_eq!(client_id(&app, &srv), None, "not registered yet");

        // A card asks for access at once.
        app.add_card(one.to_string()).unwrap();
        app.add_card(two.to_string()).unwrap();
        assert_eq!(
            app.add_card(two.to_string()).unwrap_err(),
            "err_card_exists"
        );
        let me = client_id(&app, &srv).expect("registered");
        assert_eq!(b.db().access(me, host1).unwrap(), Access::Pending);
        assert_eq!(b.db().access(me, host2).unwrap(), Access::Pending);

        // With machines the name and the server stay; the same is no change.
        assert_eq!(
            app.set_identity("Petr".into(), srv.clone()).unwrap_err(),
            "err_name_locked"
        );
        assert_eq!(
            app.set_identity("Ivan".into(), "127.0.0.1:1".into())
                .unwrap_err(),
            "err_server_has_machines"
        );
        app.set_identity("Ivan".into(), srv.clone()).unwrap();

        // Not the last card: the request goes, this client stays.
        let first = app.settings.lock().unwrap().cards[0].id;
        assert_eq!(app.remove_card(first).unwrap(), "");
        assert_eq!(b.db().access(me, host1).unwrap(), Access::None);
        assert_eq!(client_id(&app, &srv), Some(me));
        assert!(b.db().device_exists(me).unwrap());

        // The server moved with a fresh database: it does not know this client.
        b.start(true);
        let (_h2, host2) = b.host(&two, 2);
        app.poll(&mut None);
        let card = app.settings.lock().unwrap().cards[0].clone();
        let live = app.cards.lock().unwrap()[&card.id].clone();
        let server = app.live_server.lock().unwrap().clone();
        assert!(server.unknown, "{server:?}");
        assert_eq!(
            button(&live, &server),
            Button {
                op: "request",
                enabled: true
            }
        );
        // «Запросить подключение»: registered anew, asked again.
        app.ask(&card).unwrap();
        let again = client_id(&app, &srv).expect("registered anew");
        assert_eq!(b.db().access(again, host2).unwrap(), Access::Pending);
        let mut link = None;
        app.poll(&mut link);
        assert!(!app.live_server.lock().unwrap().unknown);
        assert!(link.is_some());

        // The last card: this client is gone there and here; the server stays.
        assert_eq!(app.remove_card(card.id).unwrap(), "");
        assert!(!b.db().device_exists(again).unwrap());
        assert_eq!(client_id(&app, &srv), None);
        assert_eq!(app.server().0, srv);
        app.poll(&mut link);
        assert!(link.is_none(), "no cards, no link");

        // No machines left: the name can change.
        app.set_identity("Petr".into(), srv.clone()).unwrap();

        // The server out of reach: gone here, the admin cleans up there.
        app.add_card(two.to_string()).unwrap();
        let last = client_id(&app, &srv).unwrap();
        b.server = None;
        let id = app.settings.lock().unwrap().cards[0].id;
        let note = app.remove_card(id).unwrap();
        assert!(note.starts_with("note_client_unreachable|"), "{note}");
        assert_eq!(client_id(&app, &srv), None);
        assert!(b.db().device_exists(last).unwrap(), "left for the admin");
    }
}
