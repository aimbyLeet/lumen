//! Shared setup for the server's integration tests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use lumen_server::db::Db;
use lumen_server::{Config, Server, dev_cert, tls_config};
use protocol::signal::{Card, Limits, Quality, Role, SignalMessage};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use signal::{Credentials, SignalClient, Trust};

pub const T: Duration = Duration::from_secs(3);
/// What a test client asks for unless it says otherwise.
pub const Q: Quality = Quality { fps: 60, mbps: 0.0 };

pub struct Env {
    server: Option<Server>,
    admin: Option<lumen_server::admin::AdminConfig>,
    tls: std::sync::Arc<rustls::ServerConfig>,
    login_timeout: Duration,
    idle_timeout: Duration,
    port: u16,
    pub db: PathBuf,
    pub trust: Trust,
    /// The installer the update door offers (takes effect at [`Env::restart`]).
    pub update: Option<lumen_server::update::UpdateFile>,
    pub dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.server = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn env() -> Env {
    env_with(lumen_server::LOGIN_TIMEOUT, lumen_server::IDLE_TIMEOUT)
}

/// The server with its web admin on a free port (login `admin`, password [`ADMIN_PASSWORD`]).
pub fn env_admin(web: Option<PathBuf>) -> Env {
    env_admin_with(web, false)
}

/// [`env_admin`], over plain HTTP for a reverse proxy if `behind_proxy`.
pub fn env_admin_with(web: Option<PathBuf>, behind_proxy: bool) -> Env {
    env_full(
        lumen_server::LOGIN_TIMEOUT,
        lumen_server::IDLE_TIMEOUT,
        Some(lumen_server::admin::AdminConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            user: "admin".into(),
            password: ADMIN_PASSWORD.into(),
            web,
            behind_proxy,
            site: None,
            redirect: None,
        }),
    )
}

/// [`env_admin`] as a server in a local network: the landing page from `site`
/// at the root, plain HTTP on another free port redirecting to the admin.
pub fn env_admin_lan(web: Option<PathBuf>, site: PathBuf) -> Env {
    env_full(
        lumen_server::LOGIN_TIMEOUT,
        lumen_server::IDLE_TIMEOUT,
        Some(lumen_server::admin::AdminConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            user: "admin".into(),
            password: ADMIN_PASSWORD.into(),
            web,
            behind_proxy: false,
            site: Some(site),
            redirect: Some("127.0.0.1:0".parse().unwrap()),
        }),
    )
}

pub const ADMIN_PASSWORD: &str = "correct horse battery";

pub fn env_with(login_timeout: Duration, idle_timeout: Duration) -> Env {
    env_full(login_timeout, idle_timeout, None)
}

fn env_full(
    login_timeout: Duration,
    idle_timeout: Duration,
    admin: Option<lumen_server::admin::AdminConfig>,
) -> Env {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lumen-server-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("lumen.db");
    let cert = dev_cert(&["localhost".into(), "127.0.0.1".into()]).unwrap();
    let ca = CertificateDer::from_pem_slice(cert.ca_pem.as_bytes()).unwrap();
    let tls = tls_config(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap();
    let mut e = Env {
        server: None,
        admin,
        tls,
        login_timeout,
        idle_timeout,
        port: 0,
        db,
        trust: Trust::Roots(vec![ca]),
        update: None,
        dir,
    };
    e.restart();
    e
}

/// A host's announcement: machine, account, lookup, active.
pub fn update(machine: &str, account: &str, lookup: [u8; 32], active: bool) -> SignalMessage {
    SignalMessage::HostUpdate {
        machine: machine.into(),
        account: account.into(),
        lookup,
        addrs: vec!["127.0.0.1:9000".into(), "10.9.9.9:9001".into()],
        active,
        auto_update: true,
    }
}

impl Env {
    pub fn server(&self) -> &Server {
        self.server.as_ref().expect("server running")
    }

    /// `host:port` of signaling.
    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn stop(&mut self) {
        self.server = None;
    }

    /// A new server on the same port (a free one the first time) and database.
    pub fn restart(&mut self) {
        self.server = None;
        let server = Server::start(Config {
            listen: format!("127.0.0.1:{}", self.port).parse().unwrap(),
            db: self.db.clone(),
            tls: self.tls.clone(),
            login_timeout: self.login_timeout,
            idle_timeout: self.idle_timeout,
            admin: self.admin.clone(),
            update: self.update.clone(),
        })
        .unwrap();
        self.port = server.addr().port();
        self.server = Some(server);
    }

    /// The admin's view of the database (another connection, like the CLI).
    pub fn admin(&self) -> Db {
        Db::open(&self.db).unwrap()
    }

    pub fn connect(&self) -> SignalClient {
        SignalClient::connect(&self.addr(), &self.trust, T).unwrap()
    }

    /// A newly registered host that announced itself (and the server has
    /// handled the announcement).
    pub fn host(&self, machine: &str, account: &str, lookup: [u8; 32], key: u8) -> Device {
        let mut c = self.connect();
        let (creds, _) = c.register(Role::Host, "", [key; 32], T).unwrap();
        c.send(&update(machine, account, lookup, true)).unwrap();
        sync(&mut c);
        Device { c, creds }
    }

    /// A newly registered client.
    pub fn client(&self, name: &str, key: u8) -> Device {
        let mut c = self.connect();
        let (creds, _) = c.register(Role::Client, name, [key; 32], T).unwrap();
        Device { c, creds }
    }

    /// Another connection of a registered device.
    pub fn login(&self, creds: &Credentials) -> SignalClient {
        let mut c = self.connect();
        c.login(creds, T).unwrap();
        c
    }
}

pub struct Device {
    pub c: SignalClient,
    pub creds: Credentials,
}

impl Device {
    pub fn id(&self) -> u64 {
        self.creds.device
    }
}

/// Waits until the server has handled everything sent so far (it answers in
/// order). Messages that arrive meanwhile are lost: call it when none are due.
pub fn sync(c: &mut SignalClient) {
    c.send(&SignalMessage::Ping { id: 0xFEED }).unwrap();
    loop {
        match c.recv(T).unwrap() {
            Some(SignalMessage::Pong { id: 0xFEED }) => return,
            Some(_) => continue,
            None => panic!("no pong"),
        }
    }
}

/// The next message, which must come within `T`.
pub fn next(c: &mut SignalClient) -> SignalMessage {
    c.expect(T).unwrap()
}

/// Nothing arrives for a while.
pub fn quiet(c: &mut SignalClient) {
    assert_eq!(c.recv(Duration::from_millis(300)).unwrap(), None);
}

/// The next message that is not a `SessionEnd` (an ended session's notice
/// may come after the answer that replaced it).
pub fn reply(c: &mut SignalClient) -> SignalMessage {
    loop {
        match next(c) {
            SignalMessage::SessionEnd { .. } => continue,
            m => return m,
        }
    }
}

pub fn add_host(c: &mut SignalClient, lookup: [u8; 32]) -> Card {
    c.send(&SignalMessage::AddHost { lookup }).unwrap();
    match reply(c) {
        SignalMessage::Cards { mut cards, .. } => cards.pop().unwrap(),
        m => panic!("{m:?}"),
    }
}

pub fn status(c: &mut SignalClient, lookups: &[[u8; 32]]) -> (Vec<Card>, Limits) {
    c.send(&SignalMessage::Status {
        lookups: lookups.to_vec(),
    })
    .unwrap();
    match reply(c) {
        SignalMessage::Cards { cards, forced } => (cards, forced),
        m => panic!("{m:?}"),
    }
}

pub fn connect(c: &mut SignalClient, lookup: [u8; 32], quality: Quality) -> SignalMessage {
    c.send(&SignalMessage::Connect { lookup, quality }).unwrap();
    reply(c)
}
