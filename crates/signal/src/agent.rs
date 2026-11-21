//! Long-running signaling for the host and the client.
//!
//! * [`HostAgent`]: a thread that keeps the host logged in (registering it
//!   first if it has no credentials; reconnecting with backoff), tells the
//!   server where the host is, answers keepalives and turns server messages
//!   into [`HostEvent`]s.
//! * [`ClientLink`]: the client app's connection — cards (access requests,
//!   host status), its name.
//! * [`connect_to_host`]: the client asks for a session; the returned
//!   [`ClientSession`] keeps its own signaling connection alive in a thread
//!   and tells when the session is over (the host left, access was taken
//!   back, the server went away).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{
    APP_VERSION, CapRule, Card, ConnectFailure, Credentials, FilePolicy, HostFailure, LOOKUP_LEN,
    Limits, LoginError, LoginFailure, Quality, Role, SignalClient, SignalMessage, Trust,
    UpdateStep,
};

/// Devices ping this often; the server drops connections silent for 30 s.
pub const PING_EVERY: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(200);
/// Longest wait between reconnection attempts after a network error.
const MAX_RETRY: Duration = Duration::from_secs(5);
/// Wait after the server refused the login (it will not change soon).
const REFUSED_RETRY: Duration = Duration::from_secs(30);

/// Which server, and how to check its certificate.
#[derive(Clone)]
pub struct ServerConfig {
    /// `host:port` of the signaling server.
    pub server: String,
    pub trust: Trust,
}

/// Connects and logs in with `creds`, or registers as `role` (with `name`,
/// for clients) when there are none. Returns the connection, the credentials
/// (new ones if it registered: keep them) and the bitrate caps (hosts).
pub fn sign_in(
    cfg: &ServerConfig,
    role: Role,
    name: &str,
    noise_key: [u8; 32],
    creds: Option<&Credentials>,
) -> Result<(SignalClient, Credentials, Vec<CapRule>), LoginError> {
    let mut c = SignalClient::connect(&cfg.server, &cfg.trust, IO_TIMEOUT)?;
    match creds {
        Some(creds) => {
            let (got, caps) = c.login(creds, IO_TIMEOUT)?;
            if got != role {
                return Err(LoginError::Rejected(LoginFailure::BadCredentials));
            }
            Ok((c, *creds, caps))
        }
        None => {
            let (creds, caps) = c.register(role, name, noise_key, IO_TIMEOUT)?;
            Ok((c, creds, caps))
        }
    }
}

/// Whether the server is there and speaks to us, before registering: a
/// `Ping` answered before login (a server of another signaling version
/// closes the connection instead).
pub fn probe(cfg: &ServerConfig) -> io::Result<()> {
    let mut c = SignalClient::connect(&cfg.server, &cfg.trust, IO_TIMEOUT)?;
    c.send(&SignalMessage::Ping { id: 1 })?;
    match c.recv(IO_TIMEOUT)? {
        Some(SignalMessage::Pong { .. }) => {
            c.close();
            Ok(())
        }
        Some(m) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected answer {m:?}"),
        )),
        None => Err(io::ErrorKind::TimedOut.into()),
    }
}

/// Removes this device from the server (its access, requests and sessions
/// go with it). `Rejected(BadCredentials)`: the server does not know it
/// (already removed).
pub fn unregister(cfg: &ServerConfig, role: Role, creds: &Credentials) -> Result<(), LoginError> {
    let (mut c, _, _) = sign_in(cfg, role, "", [0; 32], Some(creds))?;
    unregister_on(&mut c)
}

fn unregister_on(c: &mut SignalClient) -> Result<(), LoginError> {
    c.send(&SignalMessage::Unregister)?;
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match c.expect(left)? {
            SignalMessage::Unregistered => return Ok(()),
            _ => continue,
        }
    }
}

/// Who and where the host is, as the server shows it to clients.
#[derive(Debug, Clone, PartialEq)]
pub struct HostInfo {
    pub machine: String,
    pub account: String,
    /// Derived from the host's token (the server never sees the token).
    pub lookup: [u8; LOOKUP_LEN],
    /// `ip:port` the host listens on.
    pub addrs: Vec<String>,
    /// Its Windows session is the one on screen.
    pub active: bool,
    /// It updates itself when a newer client connects (the machine's switch;
    /// false for a host run by hand).
    pub auto_update: bool,
}

impl HostInfo {
    fn message(&self) -> SignalMessage {
        SignalMessage::HostUpdate {
            machine: self.machine.clone(),
            account: self.account.clone(),
            lookup: self.lookup,
            addrs: self.addrs.clone(),
            active: self.active,
            auto_update: self.auto_update,
        }
    }
}

/// A session the server set up for the host.
#[derive(Debug, Clone)]
pub struct IncomingSession {
    pub id: u64,
    /// Only this client key may open the session.
    pub client_key: [u8; 32],
    pub client_name: String,
    pub quality: Quality,
    /// What it may do with files (the admin's).
    pub files: FilePolicy,
}

#[derive(Debug)]
pub enum HostEvent {
    /// The host was created on the server: keep these credentials.
    Registered(Credentials),
    /// Logged in; the server's bitrate caps.
    Online {
        caps: Vec<CapRule>,
    },
    /// Not connected (any running session is over); retrying.
    Offline {
        reason: String,
    },
    /// The server refused the login (e.g. the admin removed this host);
    /// retrying slowly. No [`HostEvent::Offline`] follows: this is the state
    /// until the next one.
    Refused(LoginFailure),
    /// The server did not take the host's announcement (its token is another
    /// host's there); retrying slowly. No [`HostEvent::Offline`] follows.
    HostRefused(HostFailure),
    Incoming(IncomingSession),
    /// The client or the server ended session `id`.
    SessionEnd {
        id: u64,
    },
    /// The admin changed what session `id` may do with files.
    SessionFiles {
        id: u64,
        files: FilePolicy,
    },
    /// A client of this newer Lumen wants to connect.
    UpdateWanted {
        version: String,
    },
    /// A client (`client_name`) asked to restart this machine now.
    RebootWanted {
        client_name: String,
    },
}

enum Command {
    End(u64),
    Update(HostInfo),
    UpdateState(UpdateStep, String),
}

pub struct HostAgent {
    pub events: Receiver<HostEvent>,
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HostAgent {
    /// Starts the agent. Without `creds` it registers the host first
    /// ([`HostEvent::Registered`]).
    pub fn start(
        cfg: ServerConfig,
        creds: Option<Credentials>,
        noise_key: [u8; 32],
        info: HostInfo,
    ) -> HostAgent {
        let (tx, events) = mpsc::channel();
        let (commands, commands_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            thread::Builder::new()
                .name("signal-host".into())
                .spawn(move || {
                    HostLoop {
                        cfg,
                        creds,
                        noise_key,
                        info,
                        events: tx,
                        commands: commands_rx,
                        pending_state: None,
                    }
                    .run(&stop)
                })
                .expect("spawn signal-host")
        };
        HostAgent {
            events,
            commands,
            stop,
            thread: Some(thread),
        }
    }

    /// Tells the server the host ended session `id`.
    pub fn end_session(&self, id: u64) {
        let _ = self.commands.send(Command::End(id));
    }

    /// New names, token, addresses or activity: tells the server (now, and
    /// after every reconnection).
    pub fn update(&self, info: HostInfo) {
        let _ = self.commands.send(Command::Update(info));
    }

    /// Tells the server what the host does about an update (`detail`: with
    /// `Failed`, the code). Sent when connected (queued while not).
    pub fn update_state(&self, step: UpdateStep, detail: &str) {
        let _ = self
            .commands
            .send(Command::UpdateState(step, detail.to_string()));
    }
}

impl Drop for HostAgent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct HostLoop {
    cfg: ServerConfig,
    creds: Option<Credentials>,
    noise_key: [u8; 32],
    info: HostInfo,
    events: Sender<HostEvent>,
    commands: Receiver<Command>,
    /// An update state said while offline: sent after the next login.
    pending_state: Option<(UpdateStep, String)>,
}

impl HostLoop {
    fn run(&mut self, stop: &AtomicBool) {
        let mut backoff = Duration::from_secs(1);
        while !stop.load(Ordering::Relaxed) {
            // Updates queued while offline.
            self.drain_commands(None).ok();
            let reason: Option<String> = match sign_in(
                &self.cfg,
                Role::Host,
                "",
                self.noise_key,
                self.creds.as_ref(),
            ) {
                Ok((mut c, creds, caps)) => {
                    if self.creds.is_none() {
                        self.creds = Some(creds);
                        let _ = self.events.send(HostEvent::Registered(creds));
                    }
                    backoff = Duration::from_secs(1);
                    let _ = self.events.send(HostEvent::Online { caps });
                    let why = self.connected(&mut c, stop);
                    if stop.load(Ordering::Relaxed) {
                        c.close();
                        return;
                    }
                    match why {
                        Ended::Lost(why) => Some(why),
                        Ended::Refused(r) => {
                            let _ = self.events.send(HostEvent::HostRefused(r));
                            backoff = REFUSED_RETRY;
                            None
                        }
                    }
                }
                Err(LoginError::Rejected(r)) => {
                    let _ = self.events.send(HostEvent::Refused(r));
                    backoff = REFUSED_RETRY;
                    None
                }
                Err(LoginError::Io(e)) => Some(e.to_string()),
            };
            if let Some(reason) = reason {
                let _ = self.events.send(HostEvent::Offline { reason });
            }
            let until = Instant::now() + backoff;
            while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                thread::sleep(POLL);
            }
            // A refused login keeps its long wait; network trouble retries soon.
            if backoff < REFUSED_RETRY {
                backoff = (backoff * 2).min(MAX_RETRY);
            }
        }
    }

    /// Applies queued commands; sends them when connected.
    fn drain_commands(&mut self, mut c: Option<&mut SignalClient>) -> io::Result<()> {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                Command::End(id) => {
                    if let Some(c) = c.as_deref_mut() {
                        c.send(&SignalMessage::SessionEnd { id })?;
                    }
                }
                Command::Update(info) => {
                    self.info = info;
                    if let Some(c) = c.as_deref_mut() {
                        c.send(&self.info.message())?;
                    }
                }
                Command::UpdateState(state, detail) => match c.as_deref_mut() {
                    Some(c) => c.send(&SignalMessage::UpdateState { state, detail })?,
                    // Not connected: it waits for the next connection.
                    None => self.pending_state = Some((state, detail)),
                },
            }
        }
        Ok(())
    }

    /// Runs a logged-in connection until it fails; returns why.
    fn connected(&mut self, c: &mut SignalClient, stop: &AtomicBool) -> Ended {
        let mut last_ping = Instant::now();
        let mut ping_id = 0u32;
        let mut refused = None;
        let result: io::Result<()> = (|| {
            c.send(&self.info.message())?;
            if let Some((state, detail)) = self.pending_state.take() {
                c.send(&SignalMessage::UpdateState { state, detail })?;
            }
            while !stop.load(Ordering::Relaxed) {
                self.drain_commands(Some(c))?;
                if last_ping.elapsed() >= PING_EVERY {
                    last_ping = Instant::now();
                    ping_id = ping_id.wrapping_add(1);
                    c.send(&SignalMessage::Ping { id: ping_id })?;
                }
                match c.recv(POLL)? {
                    Some(SignalMessage::Incoming {
                        id,
                        client_key,
                        client_name,
                        quality,
                        files,
                    }) => {
                        let _ = self.events.send(HostEvent::Incoming(IncomingSession {
                            id,
                            client_key,
                            client_name,
                            quality,
                            files,
                        }));
                    }
                    Some(SignalMessage::SessionFiles { id, files }) => {
                        let _ = self.events.send(HostEvent::SessionFiles { id, files });
                    }
                    Some(SignalMessage::SessionEnd { id }) => {
                        let _ = self.events.send(HostEvent::SessionEnd { id });
                    }
                    Some(SignalMessage::HostRefused { reason }) => {
                        refused = Some(reason);
                        return Ok(());
                    }
                    Some(SignalMessage::UpdateWanted { version }) => {
                        let _ = self.events.send(HostEvent::UpdateWanted { version });
                    }
                    Some(SignalMessage::RebootWanted { client_name }) => {
                        let _ = self.events.send(HostEvent::RebootWanted { client_name });
                    }
                    _ => {}
                }
            }
            Ok(())
        })();
        match (refused, result) {
            (Some(r), _) => Ended::Refused(r),
            (None, Ok(())) => Ended::Lost("stopped".into()),
            (None, Err(e)) => Ended::Lost(format!("connection to the server lost: {e}")),
        }
    }
}

/// How a logged-in host connection ended.
enum Ended {
    Lost(String),
    Refused(HostFailure),
}

/// The client app's connection to the server: cards and its name. Requests
/// block until the answer; call [`ClientLink::keepalive`] when idle.
pub struct ClientLink {
    c: SignalClient,
    creds: Credentials,
    last_tx: Instant,
    ping_id: u32,
}

impl ClientLink {
    /// Logs in (or registers as `name` when there are no credentials).
    pub fn open(
        cfg: &ServerConfig,
        creds: Option<&Credentials>,
        name: &str,
        noise_key: [u8; 32],
    ) -> Result<ClientLink, LoginError> {
        let (c, creds, _) = sign_in(cfg, Role::Client, name, noise_key, creds)?;
        Ok(ClientLink {
            c,
            creds,
            last_tx: Instant::now(),
            ping_id: 0,
        })
    }

    /// The credentials it logged in with (new ones after registering).
    pub fn credentials(&self) -> Credentials {
        self.creds
    }

    fn send(&mut self, msg: &SignalMessage) -> io::Result<()> {
        self.last_tx = Instant::now();
        self.c.send(msg)
    }

    fn cards_reply(&mut self) -> io::Result<(Vec<Card>, Limits)> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.c.expect(left)? {
                SignalMessage::Cards { cards, forced } => return Ok((cards, forced)),
                _ => continue,
            }
        }
    }

    /// Asks the admin for access to the host with this lookup (once); its
    /// card, and what the admin sets for this client.
    pub fn add_host(&mut self, lookup: [u8; LOOKUP_LEN]) -> io::Result<(Card, Limits)> {
        self.send(&SignalMessage::AddHost { lookup })?;
        let (mut cards, forced) = self.cards_reply()?;
        let card = cards
            .pop()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no card"))?;
        Ok((card, forced))
    }

    /// The cards of these hosts, and what the admin sets for this client.
    pub fn status(&mut self, lookups: &[[u8; LOOKUP_LEN]]) -> io::Result<(Vec<Card>, Limits)> {
        self.send(&SignalMessage::Status {
            lookups: lookups.to_vec(),
        })?;
        self.cards_reply()
    }

    /// The card was deleted: drop the access to that host.
    pub fn forget(&mut self, lookup: [u8; LOOKUP_LEN]) -> io::Result<()> {
        self.send(&SignalMessage::Forget { lookup })
    }

    /// Asks the server to restart the machine of the host with this lookup.
    /// `Some`: why it was refused.
    pub fn reboot(&mut self, lookup: [u8; LOOKUP_LEN]) -> io::Result<Option<ConnectFailure>> {
        self.send(&SignalMessage::Reboot { lookup })?;
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let SignalMessage::RebootAnswer { refused } = self.c.expect(left)? {
                return Ok(refused);
            }
        }
    }

    /// Removes this client from the server (its last card was deleted).
    pub fn unregister(mut self) -> Result<(), LoginError> {
        unregister_on(&mut self.c)
    }

    pub fn rename(&mut self, name: &str) -> io::Result<()> {
        self.send(&SignalMessage::ClientUpdate { name: name.into() })
    }

    /// Pings if nothing was sent for [`PING_EVERY`].
    pub fn keepalive(&mut self) -> io::Result<()> {
        if self.last_tx.elapsed() >= PING_EVERY {
            self.ping_id = self.ping_id.wrapping_add(1);
            let id = self.ping_id;
            self.send(&SignalMessage::Ping { id })?;
        }
        // Drain pongs.
        while self.c.recv(Duration::ZERO)?.is_some() {}
        Ok(())
    }

    pub fn close(self) {
        self.c.close();
    }
}

#[derive(Debug)]
pub enum ConnectError {
    Io(io::Error),
    Login(LoginFailure),
    Refused(ConnectFailure),
    /// The host runs this version of Lumen, not ours ([`APP_VERSION`]).
    Version(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Io(e) => write!(f, "{e}"),
            ConnectError::Login(r) => write!(f, "login refused: {r:?}"),
            ConnectError::Refused(ConnectFailure::UpdateOff) => write!(
                f,
                "the host runs an older Lumen and does not update itself (its switch is off): update it by hand"
            ),
            ConnectError::Refused(r) => write!(f, "the server refused the session: {r:?}"),
            ConnectError::Version(host) => write!(
                f,
                "the host runs Lumen {host}, this client {APP_VERSION}: install the same version on both"
            ),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        ConnectError::Io(e)
    }
}

/// A session the server set up for this client. The signaling connection is
/// kept alive until this is dropped (which ends the session on the server).
pub struct ClientSession {
    pub id: u64,
    pub host_key: [u8; 32],
    /// Where the host listens (`ip:port`): try them all.
    pub addrs: Vec<String>,
    /// The session's frame rate and cap (the admin's, if forced).
    pub quality: Quality,
    /// What it may do with files, as the server says now.
    files: Arc<std::sync::Mutex<FilePolicy>>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClientSession {
    /// The host or the server ended the session, or the server is gone.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// What the session may do with files, kept up to date (the admin may
    /// change it while it runs).
    pub fn files(&self) -> Arc<std::sync::Mutex<FilePolicy>> {
        self.files.clone()
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Logs in as a client (own connection) and asks for a session with the host
/// whose token gives `lookup`.
pub fn connect_to_host(
    cfg: &ServerConfig,
    creds: &Credentials,
    lookup: [u8; LOOKUP_LEN],
    quality: Quality,
) -> Result<ClientSession, ConnectError> {
    let (mut c, _, _) =
        sign_in(cfg, Role::Client, "", [0; 32], Some(creds)).map_err(|e| match e {
            LoginError::Rejected(r) => ConnectError::Login(r),
            LoginError::Io(e) => ConnectError::Io(e),
        })?;
    c.send(&SignalMessage::Connect { lookup, quality })?;
    let (id, host_key, addrs, quality, files) = match c.expect(IO_TIMEOUT)? {
        SignalMessage::Session {
            id,
            host_key,
            addrs,
            quality,
            files,
        } => (id, host_key, addrs, quality, files),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Version,
            host_version,
        } => return Err(ConnectError::Version(host_version)),
        SignalMessage::ConnectError { reason, .. } => return Err(ConnectError::Refused(reason)),
        other => {
            return Err(ConnectError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected answer {other:?}"),
            )));
        }
    };
    let ended = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let files = Arc::new(std::sync::Mutex::new(files));
    let thread = {
        let (ended, stop, policy) = (ended.clone(), stop.clone(), files.clone());
        thread::Builder::new()
            .name("signal-client".into())
            .spawn(move || {
                let mut last_ping = Instant::now();
                let mut ping_id = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    if last_ping.elapsed() >= PING_EVERY {
                        last_ping = Instant::now();
                        ping_id = ping_id.wrapping_add(1);
                        if c.send(&SignalMessage::Ping { id: ping_id }).is_err() {
                            break;
                        }
                    }
                    match c.recv(POLL) {
                        Ok(Some(SignalMessage::SessionEnd { id: ended_id })) if ended_id == id => {
                            break;
                        }
                        Ok(Some(SignalMessage::SessionFiles { id: of, files })) if of == id => {
                            *policy.lock().unwrap() = files;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                ended.store(true, Ordering::Relaxed);
                if stop.load(Ordering::Relaxed) {
                    let _ = c.send(&SignalMessage::SessionEnd { id });
                    c.close();
                }
            })
            .map_err(ConnectError::Io)?
    };
    Ok(ClientSession {
        id,
        host_key,
        addrs,
        quality,
        files,
        ended,
        stop,
        thread: Some(thread),
    })
}
