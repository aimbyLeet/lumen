//! What outlives sessions: the monitors' capture, the keys, the registration on
//! the server. It waits for the sessions the server announces and admits
//! their clients.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use protocol::{ControlMessage, PacketHeader};
use signal::{
    CapRule, HostAgent, HostEvent, HostInfo, IncomingSession, LoginFailure, ServerConfig, Trust,
};
use transport::{HostToken, PublicKey, Responder, SOCKET_BUFFER, StaticKey, UdpLink};

use crate::app::{Args, status};
use crate::control::admit;
use crate::files::{
    credentials_path, key_path, load_credentials, load_or_create_token, token_path, write_file,
};
use crate::netinfo;
use crate::streamer::{Screen, Spot, open_screens};
use crate::update::{Updater, Why};

/// How many ports after --port to try when it is taken.
const MORE_PORTS: u16 = 9;
/// How often the host looks at its addresses and its session's activity.
const LOOK_AROUND_EVERY: Duration = Duration::from_secs(2);

/// Why a session ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// The client said bye.
    Bye,
    /// `--seconds` elapsed.
    Deadline,
    /// Nothing from the client for [`CLIENT_SILENCE`].
    Silent,
    /// The server ended it, or the connection to the server is gone.
    Server,
}

/// The console is locked this long after a remote session was lost, unless
/// the same client is back by then (a reconnect: it finds the desktop as it
/// left it). Closed by the client — at once.
const LOCK_GRACE: Duration = Duration::from_secs(10);
/// How long a lock may take to show before another client's session starts.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// After `Incoming`, the client's handshake must arrive within this time.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(15);

/// A client whose handshake message 1 was admitted.
pub(crate) struct Admitted {
    pub(crate) responder: Responder,
    /// Body of message 1 (a repeat gets the same answer).
    pub(crate) hello: Vec<u8>,
    /// For the log.
    pub(crate) peer: String,
    /// The client is on this machine: injecting its input would click into
    /// its own window.
    pub(crate) local: bool,
    pub(crate) psk: [u8; 32],
}

/// What outlives sessions: the monitors' capture, the keys.
pub(crate) struct Station {
    pub(crate) args: Args,
    /// The monitors to stream, monitor 1 first; taken by a session while it runs.
    pub(crate) screens: Vec<Screen>,
    /// The desktop layout the screens were opened for (`None`: synthetic).
    pub(crate) layout: Option<Vec<Spot>>,
    /// Bumped whenever the monitors change (`ControlMessage::Monitors`).
    pub(crate) generation: u32,
    pub(crate) key: StaticKey,
    pub(crate) token: HostToken,
    /// Bitrate caps from the server («auto»).
    pub(crate) caps: Vec<CapRule>,
    /// The socket clients come to; replaced after each session (a session's
    /// socket is connected to its client).
    pub(crate) port: u16,
    /// What the server was last told.
    pub(crate) info: HostInfo,
    pub(crate) looked_around: Instant,
    /// A session announced while another one was running.
    pub(crate) pending: Option<IncomingSession>,
    pub(crate) lock: LockAfter,
    /// File transfers: they outlive a session (a client that reconnects
    /// goes on with them).
    pub(crate) files: desktop::files::FileService,
    /// `--send-file` went to a client already.
    pub(crate) files_sent: bool,
    /// What the service is told about the monitors.
    pub(crate) monitors: crate::headless::Reporter,
    /// The last session was cut off (its client's own monitors stay a while).
    lost_last: bool,
    /// The service's host updates itself from the server.
    updater: Option<Updater>,
}

/// When run by the service, the console is locked after a remote session
///: whoever connects next, or sits at the machine, must
/// not find the last user's desktop open. Another client is let in only
/// after the lock; the same one after a lost session, within [`LOCK_GRACE`] —
/// without it. A client that asked to keep Windows unlocked (`keep_unlocked`
/// in `Hello`) leaves it as it was, whatever ended the session; another client
/// is still let in only after the lock.
#[derive(Default)]
pub(crate) struct LockAfter {
    /// When to lock (never on a tick: the client keeps Windows unlocked), and
    /// the client whose session ended.
    due: Option<(Option<Instant>, [u8; 32])>,
}

impl LockAfter {
    /// A session of `client` ended; `closed`: by the client itself; `keep`:
    /// the client asked not to lock.
    fn session_over(&mut self, client: [u8; 32], closed: bool, keep: bool, now: Instant) {
        let at = match (keep, closed) {
            (true, _) => None,
            (false, true) => Some(now),
            (false, false) => Some(now + LOCK_GRACE),
        };
        self.due = Some((at, client));
    }

    /// Whether to lock now: before a session of `client`, or (`None`) on a tick.
    fn lock_now(&mut self, client: Option<&[u8; 32]>, now: Instant) -> bool {
        let Some((at, last)) = self.due else {
            return false;
        };
        let lock = match client {
            Some(c) => *c != last,
            None => at.is_some_and(|at| now >= at),
        };
        if lock || client.is_some() {
            self.due = None;
        }
        lock
    }
}

pub(crate) fn run(args: Args) -> Result<()> {
    capture::init_process();
    let key_file = key_path(&args.key);
    let (key, created) = StaticKey::load_or_create(&key_file)?;
    if created {
        println!("created host key {}", key_file.display());
    }
    let server = args.server.clone().context(
        "--server is needed (server = ... in host.ini): the host waits for sessions it announces",
    )?;
    let token = load_or_create_token(false)?;
    println!("host key {} ({})", key.public(), key_file.display());
    println!(
        "token {token} ({}): give it to the clients that may connect",
        token_path().display()
    );

    let (screens, layout) = open_screens(&args)?;
    match screens.first() {
        Some(first) => println!(
            "host: {} on {}",
            screens
                .iter()
                .map(|s| format!("{}x{}", s.width, s.height))
                .collect::<Vec<_>>()
                .join(" + "),
            first.gpu().adapter_name
        ),
        None => println!("host: no monitor on the desktop"),
    }

    // A port of our own (the next ones if taken: another Windows user streams).
    let port = (0..=MORE_PORTS)
        .map(|i| args.port.saturating_add(i))
        .find(|p| {
            UdpLink::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, *p)), SOCKET_BUFFER).is_ok()
        })
        .with_context(|| {
            format!(
                "UDP ports {}..={} are all taken",
                args.port,
                args.port + MORE_PORTS
            )
        })?;
    let machine = args
        .machine
        .clone()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "host".into());
    // The service's host is the machine's, not an account's.
    let account = match (&args.account, args.service) {
        (Some(a), _) => a.clone(),
        (None, true) => String::new(),
        (None, false) => std::env::var("USERNAME").unwrap_or_else(|_| "user".into()),
    };
    let mut st = Station {
        monitors: crate::headless::Reporter::new(args.service),
        info: HostInfo {
            machine,
            account,
            lookup: token.lookup(),
            addrs: Vec::new(),
            active: true,
            auto_update: args.service && crate::update::auto(),
        },
        args,
        screens,
        layout,
        generation: 1,
        key,
        token,
        caps: Vec::new(),
        port,
        looked_around: Instant::now(),
        pending: None,
        lock: LockAfter::default(),
        files: desktop::files::FileService::start(),
        files_sent: false,
        lost_last: false,
        updater: None,
    };
    st.look_around();
    st.serve(&server)
}

impl Station {
    /// The desktop's monitors are no longer what the screens were opened for
    /// (Lumen VDD's monitors came, went, moved or changed size).
    pub(crate) fn layout_changed(&self) -> bool {
        self.layout
            .as_ref()
            .is_some_and(|l| *l != crate::streamer::desktop_layout(&self.args))
    }

    /// Opens the screens anew for the desktop as it is now; a new generation.
    pub(crate) fn reopen_screens(&mut self) -> Result<()> {
        self.screens.clear();
        let (screens, layout) = open_screens(&self.args)?;
        self.screens = screens;
        self.layout = layout;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    /// Refreshes the addresses, the activity and the update switch the server
    /// knows; true if they changed (the switch: the service writes it when the
    /// user flips it; a host run by hand does not update itself).
    fn look_around(&mut self) -> bool {
        self.looked_around = Instant::now();
        let addrs: Vec<String> = if self.args.addr.is_empty() {
            desktop::net::local_ipv4s()
                .into_iter()
                .map(|ip| SocketAddr::from((ip, self.port)).to_string())
                .collect()
        } else {
            self.args.addr.iter().map(ToString::to_string).collect()
        };
        // The service's host follows the console session: always on screen.
        let active = self.args.service || netinfo::session_active();
        let auto_update = self.args.service && crate::update::auto();
        if auto_update != self.info.auto_update {
            println!("updates itself when a newer client connects: {auto_update}");
        }
        let changed = addrs != self.info.addrs
            || active != self.info.active
            || auto_update != self.info.auto_update;
        if changed {
            status(serde_json::json!({
                "event": "listening", "port": self.port, "addrs": addrs, "active": active,
            }));
            println!(
                "listening on UDP {}: {addrs:?}{}",
                self.port,
                if active {
                    ""
                } else {
                    " (session not on screen)"
                }
            );
        }
        self.info.addrs = addrs;
        self.info.active = active;
        self.info.auto_update = auto_update;
        changed
    }

    /// Whether `ip` is this machine (a local client: no input injection).
    fn is_local(&self, ip: IpAddr) -> bool {
        ip.is_loopback()
            || match ip {
                IpAddr::V4(v4) => desktop::net::local_ipv4s().contains(&v4),
                IpAddr::V6(_) => false,
            }
    }

    /// Stays registered on the server and serves each session it announces.
    fn serve(&mut self, server: &str) -> Result<()> {
        let agent = self.start_agent(server)?;
        // Only the service's host: one run by hand is no installation.
        if self.args.service {
            self.updater = Some(Updater::new(server, self.trust()?));
        }
        loop {
            if self.lock.lock_now(None, Instant::now()) {
                lock_console();
            }
            if self.looked_around.elapsed() >= LOOK_AROUND_EVERY && self.look_around() {
                agent.update(self.info.clone());
            }
            // A physical monitor came or went: the machine's virtual ones follow.
            self.monitors.look(None, self.lost_last);
            let ev = match self.pending.take() {
                Some(inc) => HostEvent::Incoming(inc),
                None => match agent.events.recv_timeout(Duration::from_millis(500)) {
                    Ok(ev) => ev,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        bail!("the signaling thread stopped")
                    }
                },
            };
            match ev {
                HostEvent::Incoming(inc) => {
                    self.session(&agent, &inc);
                    if self.args.seconds > 0.0 {
                        return Ok(());
                    }
                }
                ev => self.on_server(&agent, ev)?,
            }
        }
    }

    /// The signaling agent, logged in with the host's credentials (or
    /// registering it).
    fn trust(&self) -> Result<Trust> {
        Ok(Trust::choose(
            self.args.server_ca.as_deref(),
            self.args.server_pin.as_deref(),
        )?)
    }

    fn start_agent(&self, server: &str) -> Result<HostAgent> {
        let trust = self.trust()?;
        let creds = load_credentials()?;
        println!(
            "signaling: {server} as {} / {}{}",
            self.info.machine,
            self.info.account,
            match &creds {
                Some(c) => format!(" (host {})", c.device),
                None => " (registering)".into(),
            }
        );
        Ok(HostAgent::start(
            ServerConfig {
                server: server.into(),
                trust,
            },
            creds,
            self.key.public().0,
            self.info.clone(),
        ))
    }

    /// What the server says between sessions.
    fn on_server(&mut self, agent: &HostAgent, ev: HostEvent) -> Result<()> {
        match ev {
            HostEvent::Registered(c) => {
                write_file(&credentials_path(), &format!("{c}\n"))?;
                status(serde_json::json!({ "event": "registered", "device": c.device }));
                println!(
                    "registered as host {}; credentials in {}",
                    c.device,
                    credentials_path().display()
                );
            }
            HostEvent::Online { caps } => {
                println!("online on the server; waiting for a client");
                status(serde_json::json!({ "event": "online" }));
                self.caps = caps;
                if let Some(u) = &mut self.updater {
                    u.online(agent);
                }
            }
            HostEvent::UpdateWanted { version } => {
                println!("a client of Lumen {version} wants this host");
                if let Some(u) = &mut self.updater {
                    u.update(agent, Why::Wanted, None);
                }
            }
            HostEvent::RebootWanted { client_name } => restart(&client_name),
            HostEvent::Offline { reason } => {
                println!("offline: {reason}; retrying");
                status(serde_json::json!({ "event": "offline", "reason": reason }));
            }
            HostEvent::Refused(LoginFailure::Version) => {
                println!("the server speaks another signaling version: updating to its Lumen");
                refused(format!("{:?}", LoginFailure::Version));
                if let Some(u) = &mut self.updater {
                    u.update(agent, Why::Refused, None);
                }
            }
            HostEvent::Refused(r) => {
                println!(
                    "the server refused this host ({r:?}): removed by the admin? Delete {} to register again",
                    credentials_path().display()
                );
                refused(format!("{r:?}"));
            }
            HostEvent::HostRefused(r) => {
                println!("the server did not take this host's announcement: {r:?}");
                refused(format!("{r:?}"));
            }
            HostEvent::SessionEnd { .. }
            | HostEvent::SessionFiles { .. }
            | HostEvent::Incoming(_) => {}
        }
        Ok(())
    }

    /// A session the server announced, from the handshake to its end; the
    /// server is told unless it ended it.
    fn session(&mut self, agent: &HostAgent, inc: &IncomingSession) {
        if self.lock.lock_now(Some(&inc.client_key), Instant::now()) {
            lock_console();
        }
        println!(
            "incoming session {:016x} from {} ({} fps, cap {})",
            inc.id,
            inc.client_name,
            inc.quality.fps,
            if inc.quality.is_auto() {
                "auto".to_string()
            } else {
                format!("{} Mbit/s", inc.quality.mbps)
            }
        );
        status(serde_json::json!({
            "event": "session", "client": inc.client_name,
            "fps": inc.quality.fps, "mbps": inc.quality.mbps,
        }));
        let result = self.accept(agent, inc);
        status(serde_json::json!({
            "event": "session_end",
            "reason": match &result {
                Ok(end) => format!("{end:?}"),
                Err(e) => format!("{e:#}"),
            },
        }));
        match result {
            Ok(end) => {
                println!("session over: {end:?}");
                if end != End::Server {
                    agent.end_session(inc.id);
                }
            }
            Err(e) => {
                println!("session failed: {e:#}");
                agent.end_session(inc.id);
            }
        }
    }

    /// Our port on each of our addresses: Windows sends from a socket bound to
    /// an address only through that address's interface (strong host model),
    /// so the reply leaves the way the client's datagram came in. A socket on
    /// 0.0.0.0 would follow the default route, which a VPN with a TUN adapter
    /// (Happ…) takes over: the client would never hear back. With --addr (a
    /// mapping, not our addresses) — one socket on 0.0.0.0.
    fn listen(&self) -> Result<Vec<UdpLink>> {
        let mut links = Vec::new();
        if self.args.addr.is_empty() {
            for ip in desktop::net::local_ipv4s() {
                match UdpLink::bind(SocketAddr::from((ip, self.port)), SOCKET_BUFFER) {
                    Ok(l) => links.push(l),
                    Err(e) => println!("UDP {ip}:{}: {e}", self.port),
                }
            }
        }
        if links.is_empty() {
            links.push(
                UdpLink::bind(
                    SocketAddr::from((Ipv4Addr::UNSPECIFIED, self.port)),
                    SOCKET_BUFFER,
                )
                .with_context(|| format!("UDP port {}", self.port))?,
            );
        }
        Ok(links)
    }

    /// Waits for the announced client's handshake on our port, then runs the session.
    fn accept(&mut self, agent: &HostAgent, inc: &IncomingSession) -> Result<End> {
        let mut links = self.listen()?;
        // Each socket in turn; what arrives meanwhile waits in its buffer.
        let slice = Duration::from_millis(200 / links.len() as u64).max(Duration::from_millis(10));
        let expected = PublicKey(inc.client_key);
        let mut buf = vec![0u8; 2048];
        let deadline = Instant::now() + HANDSHAKE_WAIT;
        let (link, peer, responder, n) = loop {
            if Instant::now() > deadline {
                bail!(
                    "no handshake from {} within {HANDSHAKE_WAIT:?}",
                    inc.client_name
                );
            }
            if let Some(end) = self.server_says(agent, inc) {
                return Ok(end);
            }
            let mut got = None;
            for (i, link) in links.iter().enumerate() {
                let mut admitted = None;
                let found = link.wait_for_peer(&mut buf, slice, |d| {
                    admitted = admit(&self.key, &expected, d);
                    admitted.is_some()
                })?;
                if let (Some((peer, n)), Some(r)) = (found, admitted) {
                    got = Some((i, peer, r, n));
                    break;
                }
            }
            if let Some((i, peer, r, n)) = got {
                break (links.swap_remove(i), peer, r, n);
            }
        };
        drop(links);
        let hello = PacketHeader::decode(&buf[..n])?.1.to_vec();
        let adm = Admitted {
            responder,
            hello,
            peer: format!("{} at {peer}", inc.client_name),
            local: self.is_local(peer.ip()),
            psk: self.token.psk(),
        };
        let lock = self.args.service && !adm.local;
        let keep = matches!(
            ControlMessage::decode(adm.responder.payload()),
            Ok(ControlMessage::Hello {
                keep_unlocked: true,
                ..
            })
        );
        if keep {
            println!("the client asks not to lock Windows after the session");
        }
        let end = crate::session::run(self, link, adm, agent, inc);
        // Closed by the client, or cut off (it may come back): its own
        // monitors go now or after a while, Windows is locked likewise.
        let closed = matches!(end, Ok(End::Bye | End::Deadline));
        self.lost_last = !closed;
        self.monitors.tell(None, !closed);
        if lock {
            self.lock
                .session_over(inc.client_key, closed, keep, Instant::now());
        }
        end
    }

    /// What the server said meanwhile about the session `inc`: why it ends, if it does.
    pub(crate) fn server_says(&mut self, agent: &HostAgent, inc: &IncomingSession) -> Option<End> {
        for ev in agent.events.try_iter() {
            match ev {
                HostEvent::SessionEnd { id } if id == inc.id => {
                    println!("the server ended the session");
                    return Some(End::Server);
                }
                HostEvent::RebootWanted { client_name } => {
                    restart(&client_name);
                    return Some(End::Server);
                }
                HostEvent::Offline { reason } => {
                    println!("offline: {reason}");
                    status(serde_json::json!({ "event": "offline", "reason": reason }));
                    return Some(End::Server);
                }
                // Off the server (it forgot this host, say): the session too.
                HostEvent::Refused(r) => {
                    println!("the server refused this host: {r:?}");
                    refused(format!("{r:?}"));
                    return Some(End::Server);
                }
                HostEvent::HostRefused(r) => {
                    println!("the server did not take this host's announcement: {r:?}");
                    refused(format!("{r:?}"));
                    return Some(End::Server);
                }
                HostEvent::Online { caps } => self.caps = caps,
                HostEvent::SessionFiles { id, files } if id == inc.id => {
                    let _ = self.files.commands().send(desktop::files::Cmd::Policy {
                        allowed: files.allowed,
                        cap_bps: files.cap_bps(self.args.file_mbps),
                    });
                }
                // One client at a time: the server announces another only
                // after ending this one (e.g. the same client came back).
                HostEvent::Incoming(other) => {
                    self.pending = Some(other);
                    return Some(End::Server);
                }
                // The server asks only a free host; one asked just now waits for the next.
                HostEvent::SessionEnd { .. }
                | HostEvent::SessionFiles { .. }
                | HostEvent::UpdateWanted { .. }
                | HostEvent::Registered(_) => {}
            }
        }
        None
    }
}

/// A client asked to restart the machine: no questions, sessions end with it.
fn restart(client_name: &str) {
    println!("{client_name} restarts this machine");
    if let Err(e) = crate::reboot::restart_machine() {
        eprintln!("restart failed: {e:#}");
    }
}

/// Lumen Host's word: the server does not take this host (`reason`, as
/// `desktop::hostsvc::HostStatus::detail` reads it).
fn refused(reason: String) {
    status(serde_json::json!({ "event": "refused", "reason": reason }));
}

/// Locks the console and waits for the lock screen (the next session must
/// not show a frame of the desktop).
fn lock_console() {
    match netinfo::lock_console() {
        Ok(true) => {
            let start = Instant::now();
            while !netinfo::console_locked() && start.elapsed() < LOCK_WAIT {
                std::thread::sleep(Duration::from_millis(50));
            }
            println!(
                "console locked after the session ({} ms)",
                start.elapsed().as_millis()
            );
        }
        Ok(false) => {}
        Err(e) => println!("could not lock the console: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_grace_unless_the_same_client_returns() {
        let t0 = Instant::now();
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut l = LockAfter::default();
        assert!(!l.lock_now(None, t0), "no session yet");

        l.session_over(a, true, false, t0);
        assert!(l.lock_now(None, t0), "closed by the client: at once");

        l.session_over(a, false, false, t0);
        assert!(!l.lock_now(None, t0 + LOCK_GRACE / 2));
        assert!(l.lock_now(None, t0 + LOCK_GRACE));
        assert!(!l.lock_now(None, t0 + LOCK_GRACE * 2), "once");

        l.session_over(a, false, false, t0);
        assert!(!l.lock_now(Some(&a), t0), "the same client: no lock");
        assert!(!l.lock_now(None, t0 + LOCK_GRACE), "and none later");

        l.session_over(a, false, false, t0);
        assert!(l.lock_now(Some(&b), t0), "another client: lock first");
        assert!(!l.lock_now(Some(&b), t0));
    }

    #[test]
    fn a_client_that_keeps_windows_unlocked_is_not_locked_after() {
        let t0 = Instant::now();
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut l = LockAfter::default();
        for closed in [true, false] {
            l.session_over(a, closed, true, t0);
            assert!(!l.lock_now(None, t0), "closed {closed}: not at once");
            assert!(!l.lock_now(None, t0 + LOCK_GRACE * 100), "nor ever");
            assert!(!l.lock_now(Some(&a), t0), "nor before its own session");
        }
        l.session_over(a, false, true, t0);
        assert!(!l.lock_now(None, t0 + LOCK_GRACE * 100));
        assert!(l.lock_now(Some(&b), t0), "another client: lock first");
        assert!(!l.lock_now(Some(&b), t0), "once");
    }
}
