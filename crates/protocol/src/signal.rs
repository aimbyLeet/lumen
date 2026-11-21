//! Signaling: messages between devices and the server, over TLS (TCP).
//!
//! Framing: `u32` little-endian body length, then the body; the body starts
//! with a message tag. [`FrameBuffer`] reassembles frames from a byte stream.
//!
//! Flow:
//! * A device registers once (`Register` → `Registered` with its id and
//!   secret, then `LoginOk`) and later logs in with them (`Login` → `LoginOk` /
//!   `LoginError`). `Register` carries the device's Noise key; the server hands
//!   the host's to clients and the client's to the host.
//! * A host tells who and where it is (`HostUpdate`: machine, Windows account,
//!   the `lookup` derived from its token, its addresses) after every login and
//!   whenever that changes.
//! * A client finds a host by `lookup`: `AddHost` asks the admin for access
//!   (once; a rejected request stays rejected), `Status` only looks; both are
//!   answered with `Cards`. `Forget` drops the client's access to that host.
//! * `Connect` → the server checks the access, that the host is online, active
//!   and free (the same client's older session is ended), then sends `Session`
//!   to the client and `Incoming` to the host (or `ConnectError`).
//! * Either side ends with `SessionEnd`; so does the server (access taken back,
//!   the other side gone). `Ping` / `Pong` keep the connection alive.

use crate::DecodeError;
use crate::wire::{Reader, WriteExt, wire_enum};

/// Signaling protocol version, checked at login. v6: file transfers
/// allowed or not, and their speed cap (`FilePolicy`, `SessionFiles`). v7:
/// `Ping` before login, `Unregister`, `HostRefused`
///. v8: the admin forces the frame rate and the
/// cap each on its own (`Limits`). v9: hosts update
/// themselves (`UpdateWanted`, `UpdateState`, `Presence::Updating`, a card's
/// `update_failed`). v10: the host's update switch
/// (`HostUpdate::auto_update`, a card's `auto_update`, `ConnectFailure::UpdateOff`).
/// v11: a client restarts a host's machine (`Reboot`, `RebootAnswer`, `RebootWanted`,
/// `Presence::Restarting`).
pub const SIGNAL_VERSION: u16 = 11;
/// The version of Lumen (all its programs share it, the workspace's): a
/// client connects only to a host of the very same version.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Upper bound of one frame body (the largest message is a card list).
pub const MAX_FRAME: usize = 64 * 1024;
pub const NOISE_KEY_LEN: usize = 32;
pub const SECRET_LEN: usize = 32;
pub const LOOKUP_LEN: usize = 32;
/// Most addresses a host may announce.
pub const MAX_ADDRS: usize = 16;
/// Longest name (machine, account, client), in bytes.
pub const MAX_NAME: usize = 64;

/// A name people give: a machine's (Lumen Host), a client's own (Lumen Client), the admin's for a client.
/// Trimmed, 1..=`MAX_NAME` characters, Latin letters, digits, spaces and printable signs only — no Cyrillic
/// or other scripts (owner's decision 2026-11-09). The same rule in the UIs: `nameNotLatin` (web/client/src/ui.tsx).
pub fn name_allowed(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.len() <= MAX_NAME && name.chars().all(|c| (' '..='~').contains(&c))
}
/// Frame rates a session may run at.
pub const FPS_CHOICES: [u16; 2] = [30, 60];
/// Bitrate caps offered to clients and to the admin, Mbit/s (besides «auto»).
pub const MBPS_PRESETS: [f32; 7] = [10.0, 20.0, 30.0, 40.0, 60.0, 80.0, 100.0];

wire_enum! {
    pub enum Role {
        Host = 1,
        Client = 2,
    }
}

wire_enum! {
    pub enum LoginFailure {
        /// Unknown device or wrong secret (e.g. the admin removed it).
        BadCredentials = 1,
        /// The device speaks another `SIGNAL_VERSION`.
        Version = 2,
        /// A malformed `Register` (empty or too long name).
        BadRequest = 3,
    }
}

wire_enum! {
    pub enum ConnectFailure {
        /// No host with this `lookup` (wrong token, or the host changed it).
        UnknownHost = 1,
        /// The admin has not approved this client for that host.
        NotApproved = 2,
        Offline = 3,
        /// Another client is connected (one at a time).
        Busy = 4,
        /// Only clients can connect.
        NotAClient = 5,
        /// The host's Windows session is not the one on screen.
        Inactive = 6,
        /// The host runs another version of Lumen ([`APP_VERSION`]).
        Version = 7,
        /// The host runs an older Lumen and does not update itself (its switch is off).
        UpdateOff = 8,
    }
}

wire_enum! {
    /// Why the server did not take a host's `HostUpdate`.
    pub enum HostFailure {
        /// Another host has this token's `lookup` (this machine was removed
        /// here while the server was out of reach: its old record stays).
        TokenTaken = 1,
        /// The machine or account name is empty or too long.
        BadName = 2,
    }
}

wire_enum! {
    /// A client's access to a host.
    pub enum Access {
        /// Never asked, or the admin took it back.
        None = 1,
        Pending = 2,
        Approved = 3,
        Rejected = 4,
    }
}

wire_enum! {
    pub enum Presence {
        Offline = 1,
        Online = 2,
        /// Another client is connected.
        Busy = 3,
        /// Online, but its Windows session is not on screen.
        Inactive = 4,
        /// Installing a newer Lumen (it said `UpdateState::Updating`): back soon.
        Updating = 5,
        /// A client restarted the machine (`Reboot`): back after it logs in again.
        Restarting = 6,
    }
}

wire_enum! {
    /// What a host does about an update (`UpdateState`).
    pub enum UpdateStep {
        /// Installing now: it goes offline and comes back newer.
        Updating = 1,
        /// It tried and could not (`detail`: an `err_update_…` code).
        Failed = 2,
        /// It does not update by itself (its owner turned that off).
        Off = 3,
    }
}

/// Frame rate and bitrate cap of a session; `mbps == 0` is «auto» (the
/// server's cap table by picture size and frame rate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    pub fps: u16,
    pub mbps: f32,
}

impl Quality {
    pub fn is_auto(&self) -> bool {
        self.mbps <= 0.0
    }

    /// One of [`FPS_CHOICES`], and «auto» or a positive cap of at most 1 Gbit/s.
    pub fn is_valid(&self) -> bool {
        FPS_CHOICES.contains(&self.fps)
            && self.mbps.is_finite()
            && (0.0..=1000.0).contains(&self.mbps)
    }
}

/// What the admin forces on a client, each on its own; `None`: the client
/// chooses. `mbps == Some(0.0)` forces «auto».
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Limits {
    pub fps: Option<u16>,
    pub mbps: Option<f32>,
}

impl Limits {
    pub fn is_empty(&self) -> bool {
        self.fps.is_none() && self.mbps.is_none()
    }

    /// What is set is one of [`FPS_CHOICES`], and «auto» or a positive cap of
    /// at most 1 Gbit/s.
    pub fn is_valid(&self) -> bool {
        Quality {
            fps: self.fps.unwrap_or(FPS_CHOICES[0]),
            mbps: self.mbps.unwrap_or(0.0),
        }
        .is_valid()
    }

    /// These limits, with `other` filling in what they leave open: a client's
    /// own over everyone's.
    pub fn over(self, other: Limits) -> Limits {
        Limits {
            fps: self.fps.or(other.fps),
            mbps: self.mbps.or(other.mbps),
        }
    }

    /// What the client asked for, with these limits in force.
    pub fn apply(self, asked: Quality) -> Quality {
        Quality {
            fps: self.fps.unwrap_or(asked.fps),
            mbps: self.mbps.unwrap_or(asked.mbps),
        }
    }
}

/// File transfers in a session: allowed or not (the
/// admin's switch, the client's own over the global one), and the most
/// either side's file data may take, Mbit/s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePolicy {
    pub allowed: bool,
    pub mbps: u16,
}

impl FilePolicy {
    /// The cap of our file data, bit/s: `--file-mbps` (`own`), else the server's.
    pub fn cap_bps(&self, own: Option<f64>) -> u64 {
        (own.unwrap_or(f64::from(self.mbps)) * 1e6) as u64
    }
}

impl Default for FilePolicy {
    fn default() -> Self {
        FilePolicy {
            allowed: true,
            mbps: 100,
        }
    }
}

fn put_files(out: &mut Vec<u8>, f: &FilePolicy) {
    out.put_bool(f.allowed);
    out.put_u16(f.mbps);
}

fn files(r: &mut Reader<'_>) -> Result<FilePolicy, DecodeError> {
    Ok(FilePolicy {
        allowed: r.bool()?,
        mbps: r.u16()?,
    })
}

/// Bitrate cap for pictures up to `max_pixels` at up to `fps` frames/s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapRule {
    pub max_pixels: u32,
    pub fps: u16,
    pub mbps: f32,
}

/// The cap that applies to `width`×`height` at `fps`: the rule with the
/// smallest `max_pixels` that fits the picture, among the rules with the
/// smallest `fps` that is at least `fps`.
pub fn cap_for(rules: &[CapRule], width: u32, height: u32, fps: u32) -> Option<f32> {
    let pixels = u64::from(width) * u64::from(height);
    let rate = rules
        .iter()
        .filter(|r| u32::from(r.fps) >= fps)
        .map(|r| r.fps)
        .min()?;
    rules
        .iter()
        .filter(|r| r.fps == rate && u64::from(r.max_pixels) >= pixels)
        .min_by_key(|r| r.max_pixels)
        .map(|r| r.mbps)
}

/// What a client sees of a host it has a card for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardHost {
    pub machine: String,
    pub account: String,
    /// The Lumen version the host last logged in with (empty: never).
    pub version: String,
    pub access: Access,
    pub presence: Presence,
    /// The host's last update failed: its `err_update_…` code (empty: no failure).
    pub update_failed: String,
    /// The host updates itself when a newer client connects (its switch, as it last said).
    pub auto_update: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub lookup: [u8; LOOKUP_LEN],
    /// `None`: no host has this lookup.
    pub host: Option<CardHost>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalMessage {
    /// Device → server, once: creates the device.
    Register {
        version: u16,
        role: Role,
        /// Clients: the client's name. Hosts: ignored (see `HostUpdate`).
        name: String,
        noise_key: [u8; NOISE_KEY_LEN],
        /// [`APP_VERSION`] (not read from a device of another `version`).
        app_version: String,
    },
    /// Server → device: keep these to log in later; `LoginOk` follows.
    Registered {
        device: u64,
        secret: [u8; SECRET_LEN],
    },
    Login {
        version: u16,
        device: u64,
        secret: [u8; SECRET_LEN],
        /// [`APP_VERSION`] (not read from a device of another `version`).
        app_version: String,
    },
    /// Server → device. Hosts get the bitrate caps (empty for clients).
    LoginOk {
        role: Role,
        caps: Vec<CapRule>,
    },
    LoginError {
        reason: LoginFailure,
    },
    /// Device → server: remove this device (its access, requests and
    /// sessions go with it). Answered with `Unregistered`, then the server
    /// closes the connection.
    Unregister,
    Unregistered,
    /// Server → host: its `HostUpdate` was not taken; the server closes the
    /// connection.
    HostRefused {
        reason: HostFailure,
    },
    /// Host → server, after login and on every change.
    HostUpdate {
        machine: String,
        account: String,
        lookup: [u8; LOOKUP_LEN],
        /// `ip:port` the host listens on (all its interfaces).
        addrs: Vec<String>,
        /// Its Windows session is the one on screen.
        active: bool,
        /// It updates itself when a newer client connects (the machine's switch).
        auto_update: bool,
    },
    /// Client → server: its own name changed.
    ClientUpdate {
        name: String,
    },
    /// Client → server: ask the admin for access to this host (no-op if
    /// already asked, approved or rejected). Answered with `Cards`.
    AddHost {
        lookup: [u8; LOOKUP_LEN],
    },
    /// Client → server: how these hosts are doing. Answered with `Cards`.
    Status {
        lookups: Vec<[u8; LOOKUP_LEN]>,
    },
    /// Server → client. `forced`: what the admin sets for this client (the
    /// client cannot change it).
    Cards {
        cards: Vec<Card>,
        forced: Limits,
    },
    /// Client → server: drop the access to this host (the card is deleted).
    Forget {
        lookup: [u8; LOOKUP_LEN],
    },
    /// Client → server: start a session with the host, at this quality
    /// (unless the admin forces another one).
    Connect {
        lookup: [u8; LOOKUP_LEN],
        quality: Quality,
    },
    ConnectError {
        reason: ConnectFailure,
        /// With [`ConnectFailure::Version`] and [`ConnectFailure::UpdateOff`]: the host's version; else empty.
        host_version: String,
    },
    /// Server → client: the session is set up.
    Session {
        id: u64,
        host_key: [u8; NOISE_KEY_LEN],
        addrs: Vec<String>,
        quality: Quality,
        files: FilePolicy,
    },
    /// Server → host: a client is coming. Only this key may open session `id`.
    Incoming {
        id: u64,
        client_key: [u8; NOISE_KEY_LEN],
        client_name: String,
        quality: Quality,
        files: FilePolicy,
    },
    /// Server → both: the admin changed what session `id` may do with files.
    SessionFiles {
        id: u64,
        files: FilePolicy,
    },
    /// Either way: session `id` is over.
    SessionEnd {
        id: u64,
    },
    /// Either way, also before login (is the server there at all?).
    Ping {
        id: u32,
    },
    Pong {
        id: u32,
    },
    /// Server → host: a client of this newer `version` wants to connect.
    UpdateWanted {
        version: String,
    },
    /// Host → server: what it does about an update (an `UpdateWanted`, its
    /// owner's click, or after it tried). `detail`: with `Failed`, the code.
    UpdateState {
        state: UpdateStep,
        detail: String,
    },
    /// Client → server: restart the machine of this host (no questions asked:
    /// sessions on it end). Needs approved access and the host online.
    /// Answered with `RebootAnswer`.
    Reboot {
        lookup: [u8; LOOKUP_LEN],
    },
    /// Server → client: `None` — the host was told to restart.
    RebootAnswer {
        refused: Option<ConnectFailure>,
    },
    /// Server → host: restart the machine now; `client_name` asked for it.
    RebootWanted {
        client_name: String,
    },
}

mod tag {
    pub const REGISTER: u8 = 1;
    pub const REGISTERED: u8 = 2;
    pub const LOGIN: u8 = 3;
    pub const LOGIN_OK: u8 = 4;
    pub const LOGIN_ERROR: u8 = 5;
    pub const HOST_UPDATE: u8 = 6;
    pub const CLIENT_UPDATE: u8 = 7;
    pub const ADD_HOST: u8 = 8;
    pub const STATUS: u8 = 9;
    pub const CARDS: u8 = 10;
    pub const FORGET: u8 = 11;
    pub const CONNECT: u8 = 12;
    pub const CONNECT_ERROR: u8 = 13;
    pub const SESSION: u8 = 14;
    pub const INCOMING: u8 = 15;
    pub const SESSION_END: u8 = 16;
    pub const PING: u8 = 17;
    pub const PONG: u8 = 18;
    pub const SESSION_FILES: u8 = 19;
    pub const UNREGISTER: u8 = 20;
    pub const UNREGISTERED: u8 = 21;
    pub const HOST_REFUSED: u8 = 22;
    pub const UPDATE_WANTED: u8 = 23;
    pub const UPDATE_STATE: u8 = 24;
    pub const REBOOT: u8 = 25;
    pub const REBOOT_ANSWER: u8 = 26;
    pub const REBOOT_WANTED: u8 = 27;
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.put_u16(b.len() as u16);
    out.extend_from_slice(b);
}

fn str(r: &mut Reader<'_>) -> Result<String, DecodeError> {
    let n = r.u16()?;
    String::from_utf8(r.take(usize::from(n))?.to_vec())
        .map_err(|_| DecodeError::InvalidValue("utf-8 string"))
}

fn put_strs(out: &mut Vec<u8>, v: &[String]) {
    out.put_u16(v.len() as u16);
    for s in v {
        put_str(out, s);
    }
}

fn strs(r: &mut Reader<'_>) -> Result<Vec<String>, DecodeError> {
    let n = r.u16()?;
    (0..n).map(|_| str(r)).collect()
}

/// `Register` / `Login`: the app version follows only in this
/// [`SIGNAL_VERSION`]. A device of another one (whose message ends earlier)
/// still decodes, and gets `LoginFailure::Version` rather than a broken
/// connection.
fn app_version(r: &mut Reader<'_>, version: u16) -> Result<String, DecodeError> {
    if version == SIGNAL_VERSION {
        str(r)
    } else {
        r.rest();
        Ok(String::new())
    }
}

fn put_quality(out: &mut Vec<u8>, q: &Quality) {
    out.put_u16(q.fps);
    out.put_u32(q.mbps.to_bits());
}

fn quality(r: &mut Reader<'_>) -> Result<Quality, DecodeError> {
    Ok(Quality {
        fps: r.u16()?,
        mbps: f32::from_bits(r.u32()?),
    })
}

impl SignalMessage {
    /// Appends the message body (tag + fields), without the length prefix.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            SignalMessage::Register {
                version,
                role,
                name,
                noise_key,
                app_version,
            } => {
                out.put_u8(tag::REGISTER);
                out.put_u16(*version);
                out.put_u8(*role as u8);
                put_str(out, name);
                out.extend_from_slice(noise_key);
                put_str(out, app_version);
            }
            SignalMessage::Registered { device, secret } => {
                out.put_u8(tag::REGISTERED);
                out.put_u64(*device);
                out.extend_from_slice(secret);
            }
            SignalMessage::Login {
                version,
                device,
                secret,
                app_version,
            } => {
                out.put_u8(tag::LOGIN);
                out.put_u16(*version);
                out.put_u64(*device);
                out.extend_from_slice(secret);
                put_str(out, app_version);
            }
            SignalMessage::LoginOk { role, caps } => {
                out.put_u8(tag::LOGIN_OK);
                out.put_u8(*role as u8);
                out.put_u16(caps.len() as u16);
                for c in caps {
                    out.put_u32(c.max_pixels);
                    out.put_u16(c.fps);
                    out.put_u32(c.mbps.to_bits());
                }
            }
            SignalMessage::LoginError { reason } => {
                out.put_u8(tag::LOGIN_ERROR);
                out.put_u8(*reason as u8);
            }
            SignalMessage::Unregister => out.put_u8(tag::UNREGISTER),
            SignalMessage::Unregistered => out.put_u8(tag::UNREGISTERED),
            SignalMessage::HostRefused { reason } => {
                out.put_u8(tag::HOST_REFUSED);
                out.put_u8(*reason as u8);
            }
            SignalMessage::HostUpdate {
                machine,
                account,
                lookup,
                addrs,
                active,
                auto_update,
            } => {
                out.put_u8(tag::HOST_UPDATE);
                put_str(out, machine);
                put_str(out, account);
                out.extend_from_slice(lookup);
                put_strs(out, addrs);
                out.put_bool(*active);
                out.put_bool(*auto_update);
            }
            SignalMessage::ClientUpdate { name } => {
                out.put_u8(tag::CLIENT_UPDATE);
                put_str(out, name);
            }
            SignalMessage::AddHost { lookup } => {
                out.put_u8(tag::ADD_HOST);
                out.extend_from_slice(lookup);
            }
            SignalMessage::Status { lookups } => {
                out.put_u8(tag::STATUS);
                out.put_u16(lookups.len() as u16);
                for l in lookups {
                    out.extend_from_slice(l);
                }
            }
            SignalMessage::Cards { cards, forced } => {
                out.put_u8(tag::CARDS);
                out.put_u16(cards.len() as u16);
                for c in cards {
                    out.extend_from_slice(&c.lookup);
                    out.put_bool(c.host.is_some());
                    if let Some(h) = &c.host {
                        put_str(out, &h.machine);
                        put_str(out, &h.account);
                        put_str(out, &h.version);
                        out.put_u8(h.access as u8);
                        out.put_u8(h.presence as u8);
                        put_str(out, &h.update_failed);
                        out.put_bool(h.auto_update);
                    }
                }
                out.put_bool(forced.fps.is_some());
                if let Some(fps) = forced.fps {
                    out.put_u16(fps);
                }
                out.put_bool(forced.mbps.is_some());
                if let Some(mbps) = forced.mbps {
                    out.put_u32(mbps.to_bits());
                }
            }
            SignalMessage::Forget { lookup } => {
                out.put_u8(tag::FORGET);
                out.extend_from_slice(lookup);
            }
            SignalMessage::Connect { lookup, quality } => {
                out.put_u8(tag::CONNECT);
                out.extend_from_slice(lookup);
                put_quality(out, quality);
            }
            SignalMessage::ConnectError {
                reason,
                host_version,
            } => {
                out.put_u8(tag::CONNECT_ERROR);
                out.put_u8(*reason as u8);
                put_str(out, host_version);
            }
            SignalMessage::Session {
                id,
                host_key,
                addrs,
                quality,
                files,
            } => {
                out.put_u8(tag::SESSION);
                out.put_u64(*id);
                out.extend_from_slice(host_key);
                put_strs(out, addrs);
                put_quality(out, quality);
                put_files(out, files);
            }
            SignalMessage::Incoming {
                id,
                client_key,
                client_name,
                quality,
                files,
            } => {
                out.put_u8(tag::INCOMING);
                out.put_u64(*id);
                out.extend_from_slice(client_key);
                put_str(out, client_name);
                put_quality(out, quality);
                put_files(out, files);
            }
            SignalMessage::SessionFiles { id, files } => {
                out.put_u8(tag::SESSION_FILES);
                out.put_u64(*id);
                put_files(out, files);
            }
            SignalMessage::SessionEnd { id } => {
                out.put_u8(tag::SESSION_END);
                out.put_u64(*id);
            }
            SignalMessage::Ping { id } => {
                out.put_u8(tag::PING);
                out.put_u32(*id);
            }
            SignalMessage::Pong { id } => {
                out.put_u8(tag::PONG);
                out.put_u32(*id);
            }
            SignalMessage::UpdateWanted { version } => {
                out.put_u8(tag::UPDATE_WANTED);
                put_str(out, version);
            }
            SignalMessage::UpdateState { state, detail } => {
                out.put_u8(tag::UPDATE_STATE);
                out.put_u8(*state as u8);
                put_str(out, detail);
            }
            SignalMessage::Reboot { lookup } => {
                out.put_u8(tag::REBOOT);
                out.extend_from_slice(lookup);
            }
            SignalMessage::RebootAnswer { refused } => {
                out.put_u8(tag::REBOOT_ANSWER);
                out.put_u8(refused.map_or(0, |r| r as u8));
            }
            SignalMessage::RebootWanted { client_name } => {
                out.put_u8(tag::REBOOT_WANTED);
                put_str(out, client_name);
            }
        }
    }

    /// The whole frame: length prefix + body.
    pub fn to_frame(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        self.encode(&mut out);
        let len = (out.len() - 4) as u32;
        out[..4].copy_from_slice(&len.to_le_bytes());
        out
    }

    /// Decodes one frame body. Trailing bytes are an error.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        let msg = match r.u8()? {
            tag::REGISTER => {
                let version = r.u16()?;
                SignalMessage::Register {
                    version,
                    role: Role::try_from(r.u8()?)?,
                    name: str(&mut r)?,
                    noise_key: r.array()?,
                    app_version: app_version(&mut r, version)?,
                }
            }
            tag::REGISTERED => SignalMessage::Registered {
                device: r.u64()?,
                secret: r.array()?,
            },
            tag::LOGIN => {
                let version = r.u16()?;
                SignalMessage::Login {
                    version,
                    device: r.u64()?,
                    secret: r.array()?,
                    app_version: app_version(&mut r, version)?,
                }
            }
            tag::LOGIN_OK => {
                let role = Role::try_from(r.u8()?)?;
                let n = r.u16()?;
                let caps = (0..n)
                    .map(|_| {
                        Ok(CapRule {
                            max_pixels: r.u32()?,
                            fps: r.u16()?,
                            mbps: f32::from_bits(r.u32()?),
                        })
                    })
                    .collect::<Result<_, DecodeError>>()?;
                SignalMessage::LoginOk { role, caps }
            }
            tag::LOGIN_ERROR => SignalMessage::LoginError {
                reason: LoginFailure::try_from(r.u8()?)?,
            },
            tag::UNREGISTER => SignalMessage::Unregister,
            tag::UNREGISTERED => SignalMessage::Unregistered,
            tag::HOST_REFUSED => SignalMessage::HostRefused {
                reason: HostFailure::try_from(r.u8()?)?,
            },
            tag::HOST_UPDATE => SignalMessage::HostUpdate {
                machine: str(&mut r)?,
                account: str(&mut r)?,
                lookup: r.array()?,
                addrs: strs(&mut r)?,
                active: r.bool()?,
                auto_update: r.bool()?,
            },
            tag::CLIENT_UPDATE => SignalMessage::ClientUpdate { name: str(&mut r)? },
            tag::ADD_HOST => SignalMessage::AddHost { lookup: r.array()? },
            tag::STATUS => {
                let n = r.u16()?;
                let lookups = (0..n).map(|_| r.array()).collect::<Result<_, _>>()?;
                SignalMessage::Status { lookups }
            }
            tag::CARDS => {
                let n = r.u16()?;
                let cards = (0..n)
                    .map(|_| {
                        let lookup = r.array()?;
                        let host = if r.bool()? {
                            Some(CardHost {
                                machine: str(&mut r)?,
                                account: str(&mut r)?,
                                version: str(&mut r)?,
                                access: Access::try_from(r.u8()?)?,
                                presence: Presence::try_from(r.u8()?)?,
                                update_failed: str(&mut r)?,
                                auto_update: r.bool()?,
                            })
                        } else {
                            None
                        };
                        Ok(Card { lookup, host })
                    })
                    .collect::<Result<_, DecodeError>>()?;
                let fps = if r.bool()? { Some(r.u16()?) } else { None };
                let mbps = if r.bool()? {
                    Some(f32::from_bits(r.u32()?))
                } else {
                    None
                };
                SignalMessage::Cards {
                    cards,
                    forced: Limits { fps, mbps },
                }
            }
            tag::FORGET => SignalMessage::Forget { lookup: r.array()? },
            tag::CONNECT => SignalMessage::Connect {
                lookup: r.array()?,
                quality: quality(&mut r)?,
            },
            tag::CONNECT_ERROR => SignalMessage::ConnectError {
                reason: ConnectFailure::try_from(r.u8()?)?,
                host_version: str(&mut r)?,
            },
            tag::SESSION => SignalMessage::Session {
                id: r.u64()?,
                host_key: r.array()?,
                addrs: strs(&mut r)?,
                quality: quality(&mut r)?,
                files: files(&mut r)?,
            },
            tag::INCOMING => SignalMessage::Incoming {
                id: r.u64()?,
                client_key: r.array()?,
                client_name: str(&mut r)?,
                quality: quality(&mut r)?,
                files: files(&mut r)?,
            },
            tag::SESSION_FILES => SignalMessage::SessionFiles {
                id: r.u64()?,
                files: files(&mut r)?,
            },
            tag::SESSION_END => SignalMessage::SessionEnd { id: r.u64()? },
            tag::PING => SignalMessage::Ping { id: r.u32()? },
            tag::PONG => SignalMessage::Pong { id: r.u32()? },
            tag::UPDATE_WANTED => SignalMessage::UpdateWanted {
                version: str(&mut r)?,
            },
            tag::UPDATE_STATE => SignalMessage::UpdateState {
                state: UpdateStep::try_from(r.u8()?)?,
                detail: str(&mut r)?,
            },
            tag::REBOOT => SignalMessage::Reboot { lookup: r.array()? },
            tag::REBOOT_ANSWER => SignalMessage::RebootAnswer {
                refused: match r.u8()? {
                    0 => None,
                    v => Some(ConnectFailure::try_from(v)?),
                },
            },
            tag::REBOOT_WANTED => SignalMessage::RebootWanted {
                client_name: str(&mut r)?,
            },
            t => return Err(DecodeError::UnknownMessage(t)),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// Collects stream bytes and yields complete messages.
#[derive(Default)]
pub struct FrameBuffer {
    buf: Vec<u8>,
}

impl FrameBuffer {
    pub fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, if one has arrived. An error (oversized or
    /// malformed frame) means the stream is unusable.
    pub fn next_message(&mut self) -> Result<Option<SignalMessage>, DecodeError> {
        match self.next_frame()? {
            Some(body) => SignalMessage::decode(&body).map(Some),
            None => Ok(None),
        }
    }

    /// The next complete frame's body, not decoded (the update door's
    /// messages share the framing, see `update`).
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, DecodeError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes")) as usize;
        if len > MAX_FRAME {
            return Err(DecodeError::InvalidValue("signal frame length"));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_cap_own_or_server() {
        let p = FilePolicy {
            allowed: true,
            mbps: 40,
        };
        assert_eq!(p.cap_bps(None), 40_000_000);
        assert_eq!(p.cap_bps(Some(2.5)), 2_500_000);
    }

    #[test]
    fn names_latin_only() {
        for ok in [
            "HEADLESS-TEST",
            "Ivan Petrov",
            "ws_07 (studio #2)",
            "  trimmed  ",
            &"x".repeat(MAX_NAME),
        ] {
            assert!(name_allowed(ok), "{ok:?}");
        }
        for bad in [
            "",
            "   ",
            "Иван",
            "PC-Студия",
            "café",
            "tab\there",
            "li\nne",
            &"x".repeat(MAX_NAME + 1),
        ] {
            assert!(!name_allowed(bad), "{bad:?}");
        }
    }

    fn all() -> Vec<SignalMessage> {
        let q = Quality {
            fps: 60,
            mbps: 40.0,
        };
        vec![
            SignalMessage::Register {
                version: SIGNAL_VERSION,
                role: Role::Client,
                name: "Анна".into(),
                noise_key: [7; 32],
                app_version: "0.1.0".into(),
            },
            SignalMessage::Registered {
                device: 42,
                secret: [8; 32],
            },
            SignalMessage::Login {
                version: SIGNAL_VERSION,
                device: u64::MAX,
                secret: [9; 32],
                app_version: APP_VERSION.into(),
            },
            SignalMessage::LoginOk {
                role: Role::Client,
                caps: vec![],
            },
            SignalMessage::LoginOk {
                role: Role::Host,
                caps: vec![
                    CapRule {
                        max_pixels: 1920 * 1080,
                        fps: 60,
                        mbps: 10.0,
                    },
                    CapRule {
                        max_pixels: u32::MAX,
                        fps: 30,
                        mbps: 24.5,
                    },
                ],
            },
            SignalMessage::LoginError {
                reason: LoginFailure::Version,
            },
            SignalMessage::Unregister,
            SignalMessage::Unregistered,
            SignalMessage::HostRefused {
                reason: HostFailure::TokenTaken,
            },
            SignalMessage::HostUpdate {
                machine: "WPC-10".into(),
                account: "A.Ivanov".into(),
                lookup: [1; 32],
                addrs: vec!["192.168.100.41:9000".into(), "10.0.0.2:9000".into()],
                active: true,
                auto_update: true,
            },
            SignalMessage::ClientUpdate {
                name: "R3tr0".into(),
            },
            SignalMessage::AddHost { lookup: [2; 32] },
            SignalMessage::Status {
                lookups: vec![[2; 32], [3; 32]],
            },
            SignalMessage::Cards {
                cards: vec![
                    Card {
                        lookup: [2; 32],
                        host: Some(CardHost {
                            machine: "WPC-10".into(),
                            account: "A.Ivanov".into(),
                            version: "0.1.0".into(),
                            access: Access::Pending,
                            presence: Presence::Inactive,
                            update_failed: "err_update_download".into(),
                            auto_update: false,
                        }),
                    },
                    Card {
                        lookup: [3; 32],
                        host: None,
                    },
                ],
                forced: Limits {
                    fps: Some(30),
                    mbps: Some(0.0),
                },
            },
            SignalMessage::Cards {
                cards: vec![],
                forced: Limits {
                    fps: None,
                    mbps: Some(40.0),
                },
            },
            SignalMessage::Cards {
                cards: vec![],
                forced: Limits::default(),
            },
            SignalMessage::Forget { lookup: [2; 32] },
            SignalMessage::Connect {
                lookup: [2; 32],
                quality: q,
            },
            SignalMessage::ConnectError {
                reason: ConnectFailure::Inactive,
                host_version: String::new(),
            },
            SignalMessage::ConnectError {
                reason: ConnectFailure::Version,
                host_version: "0.1.1".into(),
            },
            SignalMessage::Session {
                id: u64::MAX - 3,
                host_key: [1; 32],
                addrs: vec!["192.168.100.41:9000".into()],
                quality: q,
                files: FilePolicy::default(),
            },
            SignalMessage::Incoming {
                id: 9,
                client_key: [3; 32],
                client_name: "R3tr0".into(),
                quality: q,
                files: FilePolicy {
                    allowed: false,
                    mbps: 20,
                },
            },
            SignalMessage::SessionFiles {
                id: 9,
                files: FilePolicy {
                    allowed: false,
                    mbps: 100,
                },
            },
            SignalMessage::SessionEnd { id: 9 },
            SignalMessage::Ping { id: 5 },
            SignalMessage::Pong { id: 5 },
            SignalMessage::UpdateWanted {
                version: "0.7.1".into(),
            },
            SignalMessage::UpdateState {
                state: UpdateStep::Failed,
                detail: "err_update_sha".into(),
            },
            SignalMessage::Reboot { lookup: [4; 32] },
            SignalMessage::RebootAnswer { refused: None },
            SignalMessage::RebootAnswer {
                refused: Some(ConnectFailure::Offline),
            },
            SignalMessage::RebootWanted {
                client_name: "R3tr0".into(),
            },
        ]
    }

    #[test]
    fn roundtrip_through_a_byte_stream() {
        let msgs = all();
        let stream: Vec<u8> = msgs.iter().flat_map(|m| m.to_frame()).collect();
        // Fed in awkward pieces.
        let mut fb = FrameBuffer::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(3) {
            fb.extend(chunk);
            while let Some(m) = fb.next_message().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, msgs);
    }

    /// A device of the previous signaling version (no app version yet) gets
    /// its `Login` read, so the server can answer `LoginFailure::Version`.
    #[test]
    fn an_older_login_still_decodes() {
        let mut body = vec![tag::LOGIN];
        body.extend_from_slice(&(SIGNAL_VERSION - 1).to_le_bytes());
        body.extend_from_slice(&7u64.to_le_bytes());
        body.extend_from_slice(&[9; SECRET_LEN]);
        assert_eq!(
            SignalMessage::decode(&body),
            Ok(SignalMessage::Login {
                version: SIGNAL_VERSION - 1,
                device: 7,
                secret: [9; SECRET_LEN],
                app_version: String::new(),
            })
        );
    }

    #[test]
    fn rejects_garbage() {
        let mut body = Vec::new();
        SignalMessage::Ping { id: 1 }.encode(&mut body);
        body.push(0);
        assert_eq!(
            SignalMessage::decode(&body),
            Err(DecodeError::TrailingBytes)
        );
        assert!(SignalMessage::decode(&[99]).is_err());
        let mut fb = FrameBuffer::default();
        fb.extend(&(MAX_FRAME as u32 + 1).to_le_bytes());
        assert!(fb.next_message().is_err());
    }

    #[test]
    fn caps_by_size_and_rate() {
        let rules: Vec<CapRule> = [
            (1920 * 1080, 60, 10.0),
            (2560 * 1440, 60, 18.0),
            (u32::MAX, 60, 40.0),
            (1920 * 1080, 30, 6.0),
            (2560 * 1440, 30, 11.0),
            (u32::MAX, 30, 24.0),
        ]
        .into_iter()
        .map(|(max_pixels, fps, mbps)| CapRule {
            max_pixels,
            fps,
            mbps,
        })
        .collect();
        assert_eq!(cap_for(&rules, 3840, 2160, 60), Some(40.0));
        assert_eq!(cap_for(&rules, 3840, 2160, 30), Some(24.0));
        assert_eq!(cap_for(&rules, 2560, 1440, 60), Some(18.0));
        assert_eq!(cap_for(&rules, 1920, 1080, 30), Some(6.0));
        assert_eq!(cap_for(&rules, 1280, 720, 24), Some(6.0));
        assert_eq!(cap_for(&rules, 3840, 2160, 120), None);
        assert_eq!(cap_for(&[], 1920, 1080, 60), None);
    }

    #[test]
    fn limits_each_on_its_own() {
        let asked = Quality {
            fps: 60,
            mbps: 80.0,
        };
        let fps = Limits {
            fps: Some(30),
            mbps: None,
        };
        let cap = Limits {
            fps: None,
            mbps: Some(20.0),
        };
        assert_eq!(Limits::default().apply(asked), asked);
        assert_eq!(fps.apply(asked).fps, 30);
        assert_eq!(fps.apply(asked).mbps, 80.0);
        assert_eq!(cap.apply(asked).fps, 60);
        assert_eq!(cap.apply(asked).mbps, 20.0);
        // A client's own wins, field by field; everyone's fills the rest.
        let both = cap.over(fps);
        assert_eq!((both.fps, both.mbps), (Some(30), Some(20.0)));
        let own = Limits {
            fps: Some(60),
            mbps: None,
        };
        assert_eq!(own.over(fps).fps, Some(60));
        assert!(Limits::default().is_empty() && !fps.is_empty());
        assert!(fps.is_valid() && cap.is_valid() && Limits::default().is_valid());
        let bad_fps = Limits {
            fps: Some(45),
            mbps: None,
        };
        let bad_cap = Limits {
            fps: None,
            mbps: Some(-1.0),
        };
        assert!(!bad_fps.is_valid() && !bad_cap.is_valid());
    }
}
