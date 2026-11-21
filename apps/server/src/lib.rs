//! Lumen server: devices, access and sessions over TLS signaling
//!.
//!
//! One thread accepts TCP connections, one thread per connection runs TLS and
//! the signaling protocol (`protocol::signal`). Connections share a [`Hub`]
//! with who is online and which sessions exist; messages for another
//! connection (e.g. `Incoming` for a host) go through its outbox, which its
//! thread drains between reads. A watcher thread re-checks running sessions
//! and connected devices against the database, so the admin's changes (access
//! taken back, a device removed) end them within [`RECHECK_EVERY`].
//!
//! Video does not go through the server: hosts announce their addresses in
//! the user's network (WireGuard, Tailscale) and clients connect to them.

pub mod admin;
pub mod db;
pub mod update;

mod cert;
mod conn;
mod hub;

pub use cert::{
    CERT_CHECK_EVERY, DevCert, SelfCert, dev_cert, key_fingerprint, self_cert, spki_fingerprint,
    tls_config, tls_config_from_files,
};
pub use hub::{Hub, SessionInfo};

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustls::ServerConfig;

use crate::conn::Connection;
use crate::db::Db;
use crate::hub::State;
use crate::update::{Downloads, UpdateFile};

/// A connection must log in within this time.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
/// A connection that sends nothing for this long is dead (devices ping every
/// 10 s): its sessions end, a host becomes free. Covers drops without a FIN.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// How often running sessions and connected devices are checked against the
/// database (access taken back, device removed).
pub const RECHECK_EVERY: Duration = Duration::from_millis(500);
/// How often a connection thread looks at its outbox while waiting for data.
const POLL: Duration = Duration::from_millis(20);
/// Most cards one `Status` is answered for (keeps `Cards` under `MAX_FRAME`).
const MAX_CARDS: usize = 256;
/// A host that said it is updating shows «updating» this long at most (it
/// logs in again sooner, newer or with a failure).
pub const UPDATING_FOR: Duration = Duration::from_secs(5 * 60);
/// A host told to restart shows «restarting» until it logs in again, this
/// long at most (if the restart did not happen).
pub const RESTARTING_FOR: Duration = Duration::from_secs(3 * 60);

pub struct Config {
    pub listen: SocketAddr,
    pub db: PathBuf,
    pub tls: Arc<ServerConfig>,
    /// [`LOGIN_TIMEOUT`] / [`IDLE_TIMEOUT`] (shorter in tests).
    pub login_timeout: Duration,
    pub idle_timeout: Duration,
    /// The web admin (same certificate), if any.
    pub admin: Option<admin::AdminConfig>,
    /// The installer the update door offers, if any.
    pub update: Option<UpdateFile>,
}

impl Config {
    /// Signaling only, with the usual timeouts: no web admin, no installer.
    pub fn new(listen: SocketAddr, db: PathBuf, tls: Arc<ServerConfig>) -> Config {
        Config {
            listen,
            db,
            tls,
            login_timeout: LOGIN_TIMEOUT,
            idle_timeout: IDLE_TIMEOUT,
            admin: None,
            update: None,
        }
    }
}

/// A running server; stops when dropped.
pub struct Server {
    addr: SocketAddr,
    admin_addr: Option<SocketAddr>,
    redirect_addr: Option<SocketAddr>,
    hub: Arc<Hub>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    pub fn start(cfg: Config) -> Result<Server> {
        // Create the schema up front so a bad path fails here, not per connection.
        Db::open(&cfg.db)?;
        let listener =
            TcpListener::bind(cfg.listen).with_context(|| format!("listen on {}", cfg.listen))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let hub = Arc::new(Hub {
            state: Mutex::new(State::default()),
            db_path: cfg.db,
            login_timeout: cfg.login_timeout,
            idle_timeout: cfg.idle_timeout,
            update: cfg.update,
            downloads: Downloads::default(),
        });
        let mut threads = Vec::new();
        let (admin_addr, redirect_addr) = match cfg.admin {
            Some(a) => {
                let started = admin::start(a, hub.clone(), cfg.tls.clone(), stop.clone())?;
                threads.extend(started.threads);
                (Some(started.addr), started.redirect)
            }
            None => (None, None),
        };
        let accept = {
            let (hub, stop) = (hub.clone(), stop.clone());
            let tls = cfg.tls;
            thread::Builder::new()
                .name("accept".into())
                .spawn(move || accept_loop(listener, hub, tls, stop))?
        };
        let watch = {
            let (hub, stop) = (hub.clone(), stop.clone());
            thread::Builder::new()
                .name("recheck".into())
                .spawn(move || recheck_loop(&hub, &stop))?
        };
        threads.insert(0, accept);
        threads.push(watch);
        Ok(Server {
            addr,
            admin_addr,
            redirect_addr,
            hub,
            stop,
            threads,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The web admin's address, if it runs.
    pub fn admin_addr(&self) -> Option<SocketAddr> {
        self.admin_addr
    }

    /// The plain HTTP port that sends browsers to the admin's HTTPS, if any.
    pub fn redirect_addr(&self) -> Option<SocketAddr> {
        self.redirect_addr
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// Blocks until the server stops (never, unless the accept loop fails).
    pub fn wait(mut self) {
        let threads = std::mem::take(&mut self.threads);
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn recheck_loop(hub: &Hub, stop: &AtomicBool) {
    let mut db: Option<Db> = None;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        if db.is_none() {
            db = Db::open(&hub.db_path)
                .map_err(|e| eprintln!("recheck: {e:#}"))
                .ok();
        }
        if let Some(d) = &db
            && let Err(e) = hub.recheck(d)
        {
            eprintln!("recheck: {e:#}");
            db = None;
        }
        while started.elapsed() < RECHECK_EVERY && !stop.load(Ordering::Relaxed) {
            thread::sleep(POLL);
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    hub: Arc<Hub>,
    tls: Arc<ServerConfig>,
    stop: Arc<AtomicBool>,
) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((tcp, peer)) => {
                let (hub, tls, stop) = (hub.clone(), tls.clone(), stop.clone());
                let spawned =
                    thread::Builder::new()
                        .name(format!("conn {peer}"))
                        .spawn(move || {
                            if let Err(e) = Connection::serve(tcp, &hub, tls, &stop) {
                                eprintln!("{peer}: {e:#}");
                            }
                        });
                match spawned {
                    Ok(t) => workers.push(t),
                    Err(e) => eprintln!("{peer}: {e}"),
                }
                workers.retain(|t| !t.is_finished());
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                eprintln!("accept: {e}");
                thread::sleep(POLL);
            }
        }
    }
    for t in workers {
        let _ = t.join();
    }
}
