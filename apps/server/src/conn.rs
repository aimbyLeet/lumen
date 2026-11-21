//! One device connection: TLS, login, the signaling messages and the update
//! door (`protocol::update`).

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use protocol::signal::{
    Access, Card, CardHost, ConnectFailure, FPS_CHOICES, FrameBuffer, HostFailure, LOOKUP_LEN,
    LoginFailure, MAX_ADDRS, NOISE_KEY_LEN, Quality, Role, SIGNAL_VERSION, SignalMessage,
    UpdateStep,
};
use protocol::update::{DoorMessage, is_door, newer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::db::{self, Db, clean_version, given_name};
use crate::hub::{Hub, Out, SessionClient};
use crate::{MAX_CARDS, POLL};

/// Who a connection logged in as.
#[derive(Clone, Copy)]
struct Me {
    device: u64,
    role: Role,
    noise_key: [u8; NOISE_KEY_LEN],
    conn: u64,
}

/// One device connection.
pub(crate) struct Connection<'a> {
    hub: &'a Hub,
    db: Db,
    stream: StreamOwned<ServerConnection, TcpStream>,
    outbox: (Sender<Out>, Receiver<Out>),
    me: Option<Me>,
}

impl Connection<'_> {
    pub(crate) fn serve(
        tcp: TcpStream,
        hub: &Hub,
        tls: Arc<ServerConfig>,
        stop: &AtomicBool,
    ) -> Result<()> {
        tcp.set_nonblocking(false)?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(POLL))?;
        tcp.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut c = Connection {
            hub,
            db: Db::open(&hub.db_path)?,
            stream: StreamOwned::new(ServerConnection::new(tls)?, tcp),
            outbox: mpsc::channel(),
            me: None,
        };
        let result = c.run(stop);
        if let Some(me) = &c.me {
            hub.detach(me.conn);
            println!("{} {} offline", db::role_name(me.role), me.device);
        }
        result
    }

    fn run(&mut self, stop: &AtomicBool) -> Result<()> {
        let started = Instant::now();
        let mut last_rx = Instant::now();
        let mut frames = FrameBuffer::default();
        while !stop.load(Ordering::Relaxed) {
            while let Ok(out) = self.outbox.1.try_recv() {
                match out {
                    Out::Msg(m) => self.send(&m)?,
                    Out::Close => return Ok(()),
                }
            }
            match self.read_into(&mut frames)? {
                None => return Ok(()),
                Some(0) => {}
                Some(_) => last_rx = Instant::now(),
            }
            while let Some(body) = frames.next_frame()? {
                if self.me.is_none() && is_door(&body) {
                    return self.door(&body, &mut frames, stop);
                }
                if !self.handle(SignalMessage::decode(&body)?)? {
                    return Ok(());
                }
            }
            if self.me.is_none() && started.elapsed() > self.hub.login_timeout {
                return Ok(());
            }
            if last_rx.elapsed() > self.hub.idle_timeout {
                return Ok(());
            }
        }
        Ok(())
    }

    /// What has arrived (at most one read's worth, waiting at most [`POLL`]),
    /// into `frames`: how many bytes (0: nothing yet). None: the device closed.
    fn read_into(&mut self, frames: &mut FrameBuffer) -> io::Result<Option<usize>> {
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => {
                frames.extend(&buf[..n]);
                Ok(Some(n))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(Some(0))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        self.stream.write_all(frame)?;
        self.stream.flush()
    }

    fn send(&mut self, msg: &SignalMessage) -> io::Result<()> {
        self.write_frame(&msg.to_frame())
    }

    fn send_door(&mut self, msg: &DoorMessage) -> io::Result<()> {
        self.write_frame(&msg.to_frame())
    }

    /// The update door (`protocol::update`): the offer, then, if the device
    /// asks within the login time, the installer from its offset. The
    /// connection closes after.
    fn door(&mut self, first: &[u8], frames: &mut FrameBuffer, stop: &AtomicBool) -> Result<()> {
        let DoorMessage::Query { app_version, .. } = DoorMessage::decode(first)? else {
            return Ok(());
        };
        let app_version = clean_version(&app_version);
        let hub = self.hub;
        let Some(file) = &hub.update else {
            return Ok(self.send_door(&DoorMessage::NoUpdate)?);
        };
        self.send_door(&DoorMessage::Offer {
            version: file.version.clone(),
            size: file.size,
            sha256: file.sha256,
        })?;
        let started = Instant::now();
        let offset = loop {
            if let Some(body) = frames.next_frame()? {
                match DoorMessage::decode(&body)? {
                    DoorMessage::Fetch { offset } => break offset,
                    _ => return Ok(()),
                }
            }
            if stop.load(Ordering::Relaxed) || started.elapsed() > hub.login_timeout {
                return Ok(());
            }
            if self.read_into(frames)?.is_none() {
                return Ok(());
            }
        };
        let Some(_slot) = hub.downloads.take() else {
            println!("update: Lumen {app_version} turned away, too many downloads");
            return Ok(self.send_door(&DoorMessage::Busy)?);
        };
        println!(
            "update: Lumen {app_version} downloads {} from byte {offset}",
            file.version
        );
        for chunk in file.chunks(offset)? {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            self.send_door(&DoorMessage::Chunk(chunk?))?;
        }
        Ok(())
    }

    /// Handles one message; false closes the connection.
    fn handle(&mut self, msg: SignalMessage) -> Result<bool> {
        let Some(me) = self.me else {
            return self.login(msg);
        };
        match (me.role, msg) {
            (_, SignalMessage::Ping { id }) => self.send(&SignalMessage::Pong { id })?,
            (_, SignalMessage::SessionEnd { id }) => self.hub.end_session(id, me.device),
            (_, SignalMessage::Unregister) => {
                self.db.remove_device(me.device)?;
                self.hub.unregistered(me.device, me.conn);
                println!("{} {} removed itself", db::role_name(me.role), me.device);
                self.send(&SignalMessage::Unregistered)?;
                return Ok(false);
            }
            (
                Role::Host,
                SignalMessage::HostUpdate {
                    machine,
                    account,
                    lookup,
                    addrs,
                    active,
                    auto_update,
                },
            ) => {
                self.db.set_auto_update(me.device, auto_update)?;
                let addrs: Vec<String> = addrs
                    .into_iter()
                    .filter(|a| a.parse::<SocketAddr>().is_ok())
                    .take(MAX_ADDRS)
                    .collect();
                if let Err(e) = self.db.update_host(me.device, &machine, &account, &lookup) {
                    eprintln!("host {}: {e:#}", me.device);
                    let taken = self
                        .db
                        .host_by_lookup(&lookup)?
                        .is_some_and(|h| h.id != me.device);
                    let reason = if taken {
                        HostFailure::TokenTaken
                    } else {
                        HostFailure::BadName
                    };
                    self.send(&SignalMessage::HostRefused { reason })?;
                    return Ok(false);
                }
                println!(
                    "host {} is {}{} at {addrs:?}",
                    me.device,
                    db::host_label(&machine, &account),
                    if active { "" } else { " (inactive)" }
                );
                self.hub.host_update(me.device, me.conn, addrs, active);
            }
            (Role::Host, SignalMessage::UpdateState { state, detail }) => {
                let detail = clean_version(&detail);
                match state {
                    UpdateStep::Updating => {
                        println!("host {} is updating", me.device);
                        self.hub.set_updating(me.device, true);
                        self.db.set_update_failed(me.device, "")?;
                    }
                    UpdateStep::Failed => {
                        println!("host {}: update failed ({detail})", me.device);
                        self.hub.set_updating(me.device, false);
                        self.db.set_update_failed(me.device, &detail)?;
                    }
                    UpdateStep::Off => {
                        println!("host {} does not update by itself", me.device);
                        self.db.set_auto_update(me.device, false)?;
                    }
                }
            }
            (Role::Client, SignalMessage::ClientUpdate { name }) => {
                if let Err(e) = self.db.set_client_name(me.device, &name) {
                    eprintln!("client {}: {e:#}", me.device);
                }
            }
            (Role::Client, SignalMessage::AddHost { lookup }) => {
                if let Some(h) = self.db.host_by_lookup(&lookup)?
                    && self.db.access(me.device, h.id)? == Access::None
                    && self.db.request_access(me.device, h.id)? == Access::Pending
                {
                    println!("client {} asks for {}", me.device, h.label());
                }
                let cards = self.cards(me.device, &[lookup])?;
                self.send(&cards)?;
            }
            (Role::Client, SignalMessage::Status { lookups }) => {
                let cards = self.cards(me.device, &lookups)?;
                self.send(&cards)?;
            }
            (Role::Client, SignalMessage::Forget { lookup }) => {
                if let Some(h) = self.db.host_by_lookup(&lookup)? {
                    self.db.forget(me.device, h.id)?;
                }
            }
            (Role::Client, SignalMessage::Connect { lookup, quality }) => {
                let reply = match self.connect(me, &lookup, quality)? {
                    Ok(m) => m,
                    Err((reason, host_version)) => SignalMessage::ConnectError {
                        reason,
                        host_version,
                    },
                };
                self.send(&reply)?;
            }
            (Role::Client, SignalMessage::Reboot { lookup }) => {
                let refused = self.reboot(me, &lookup)?.err();
                self.send(&SignalMessage::RebootAnswer { refused })?;
            }
            (Role::Host, SignalMessage::Connect { .. }) => {
                self.send(&SignalMessage::ConnectError {
                    reason: ConnectFailure::NotAClient,
                    host_version: String::new(),
                })?
            }
            _ => {}
        }
        Ok(true)
    }

    /// `Reboot`: approved access and the host online — any version, busy or not.
    fn reboot(
        &self,
        me: Me,
        lookup: &[u8; LOOKUP_LEN],
    ) -> Result<std::result::Result<(), ConnectFailure>> {
        let Some(host) = self.db.host_by_lookup(lookup)? else {
            return Ok(Err(ConnectFailure::UnknownHost));
        };
        if self.db.access(me.device, host.id)? != Access::Approved {
            return Ok(Err(ConnectFailure::NotApproved));
        }
        let Some(row) = self.db.client(me.device)? else {
            return Ok(Err(ConnectFailure::NotAClient));
        };
        let r = self.hub.want_reboot(host.id, &row.name);
        if r.is_ok() {
            println!("{} restarts {}", row.name, host.label());
        }
        Ok(r)
    }

    fn cards(&self, client: u64, lookups: &[[u8; LOOKUP_LEN]]) -> Result<SignalMessage> {
        let mut cards = Vec::with_capacity(lookups.len());
        for lookup in lookups.iter().take(MAX_CARDS) {
            let host = match self.db.host_by_lookup(lookup)? {
                Some(h) => Some(CardHost {
                    access: self.db.access(client, h.id)?,
                    presence: self.hub.presence(h.id, client),
                    machine: h.machine,
                    account: h.account,
                    version: h.version,
                    update_failed: h.update_failed,
                    auto_update: h.auto_update,
                }),
                None => None,
            };
            cards.push(Card {
                lookup: *lookup,
                host,
            });
        }
        Ok(SignalMessage::Cards {
            cards,
            forced: self.db.forced_limits(client)?,
        })
    }

    fn connect(
        &self,
        me: Me,
        lookup: &[u8; LOOKUP_LEN],
        asked: Quality,
    ) -> Result<std::result::Result<SignalMessage, (ConnectFailure, String)>> {
        let refused = |reason| Ok(Err((reason, String::new())));
        let Some(host) = self.db.host_by_lookup(lookup)? else {
            return refused(ConnectFailure::UnknownHost);
        };
        if self.db.access(me.device, host.id)? != Access::Approved {
            return refused(ConnectFailure::NotApproved);
        }
        let Some(row) = self.db.client(me.device)? else {
            return refused(ConnectFailure::NotAClient);
        };
        // Only the very same version of Lumen (as both last logged in). A
        // newer client: the host may update itself.
        if row.version != host.version {
            // Older and its switch off: no use waiting (the client's window closes).
            if newer(&row.version, &host.version) && !host.auto_update {
                return Ok(Err((ConnectFailure::UpdateOff, host.version)));
            }
            if newer(&row.version, &host.version) && self.hub.want_update(host.id, &row.version) {
                println!(
                    "{} (Lumen {}) wants {} (Lumen {}): asked it to update",
                    row.name,
                    row.version,
                    host.label(),
                    host.version
                );
            }
            return Ok(Err((ConnectFailure::Version, host.version)));
        }
        let asked = if asked.is_valid() {
            asked
        } else {
            Quality {
                fps: FPS_CHOICES[FPS_CHOICES.len() - 1],
                mbps: 0.0,
            }
        };
        let quality = self.db.forced_limits(me.device)?.apply(asked);
        let client = SessionClient {
            device: me.device,
            conn: me.conn,
            name: &row.name,
            key: me.noise_key,
        };
        let files = self.db.files_policy(me.device)?;
        let r = self
            .hub
            .connect(&client, (host.id, host.noise_key), quality, files)
            .map_err(|reason| (reason, String::new()));
        if r.is_ok() {
            let cap = if quality.is_auto() {
                "auto".to_string()
            } else {
                format!("{} Mbit/s", quality.mbps)
            };
            println!(
                "session {} -> {} at {} fps, cap {cap}",
                row.name,
                host.label(),
                quality.fps
            );
        }
        Ok(r)
    }

    fn login(&mut self, msg: SignalMessage) -> Result<bool> {
        let (device, role, noise_key, app_version) = match msg {
            SignalMessage::Register {
                version,
                role,
                name,
                noise_key,
                app_version,
            } => {
                if version != SIGNAL_VERSION {
                    return self.refuse(LoginFailure::Version);
                }
                if role == Role::Client && given_name(&name).is_none() {
                    return self.refuse(LoginFailure::BadRequest);
                }
                let (device, secret) = self.db.register(role, &name, noise_key)?;
                println!("{} {device} registered", db::role_name(role));
                self.send(&SignalMessage::Registered { device, secret })?;
                (device, role, noise_key, app_version)
            }
            SignalMessage::Login {
                version,
                device,
                secret,
                app_version,
            } => {
                if version != SIGNAL_VERSION {
                    return self.refuse(LoginFailure::Version);
                }
                match self.db.check_login(device, &secret)? {
                    Some((role, key)) => (device, role, key, app_version),
                    None => return self.refuse(LoginFailure::BadCredentials),
                }
            }
            // Is the server there at all (a device not registered yet)?
            SignalMessage::Ping { id } => {
                self.send(&SignalMessage::Pong { id })?;
                return Ok(true);
            }
            _ => return Ok(false),
        };
        let app_version = clean_version(&app_version);
        self.db.touch(device, &app_version)?;
        let caps = if role == Role::Host {
            self.db.caps()?
        } else {
            Vec::new()
        };
        let conn = self.hub.attach(device, role, self.outbox.0.clone());
        println!(
            "{} {device} online, Lumen {app_version}",
            db::role_name(role)
        );
        self.me = Some(Me {
            device,
            role,
            noise_key,
            conn,
        });
        self.send(&SignalMessage::LoginOk { role, caps })?;
        Ok(true)
    }

    fn refuse(&mut self, reason: LoginFailure) -> Result<bool> {
        self.send(&SignalMessage::LoginError { reason })?;
        Ok(false)
    }
}
