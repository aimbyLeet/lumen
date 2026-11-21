//! What all connections share: who is online, which sessions run, and how a
//! session is set up, ended or re-checked against the database.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::Result;
use protocol::signal::{
    Access, ConnectFailure, FilePolicy, NOISE_KEY_LEN, Presence, Quality, Role, SignalMessage,
};

use crate::db::{Db, random_bytes};
use crate::update::{Downloads, UpdateFile};
use crate::{RESTARTING_FOR, UPDATING_FOR};

pub(crate) enum Out {
    Msg(SignalMessage),
    /// The host logged in again elsewhere, or the device was removed.
    Close,
}

struct Conn {
    device: u64,
    role: Role,
    outbox: Sender<Out>,
}

/// A host with a connection.
struct OnlineHost {
    conn: u64,
    addrs: Vec<String>,
    active: bool,
}

struct Session {
    host: u64,
    client: u64,
    /// The client connection that asked: the session ends with it.
    client_conn: u64,
    client_name: String,
    quality: Quality,
    files: FilePolicy,
    started: Instant,
}

/// A running session, for the admin.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: u64,
    pub host: u64,
    pub client: u64,
    pub client_name: String,
    pub quality: Quality,
    pub duration: Duration,
}

#[derive(Default)]
pub(crate) struct State {
    conns: HashMap<u64, Conn>,
    hosts: HashMap<u64, OnlineHost>,
    sessions: HashMap<u64, Session>,
    /// Hosts that said they are updating, and since when.
    updating: HashMap<u64, Instant>,
    /// Hosts told to restart, and since when.
    restarting: HashMap<u64, Instant>,
    next_conn: u64,
}

impl State {
    fn send(&self, conn: u64, msg: SignalMessage) {
        if let Some(c) = self.conns.get(&conn) {
            let _ = c.outbox.send(Out::Msg(msg));
        }
    }

    /// Ends session `id`; tells both sides (the side that ended it ignores it).
    fn end(&mut self, id: u64) {
        let Some(s) = self.sessions.remove(&id) else {
            return;
        };
        if let Some(h) = self.hosts.get(&s.host) {
            self.send(h.conn, SignalMessage::SessionEnd { id });
        }
        self.send(s.client_conn, SignalMessage::SessionEnd { id });
    }

    /// A device removed from the database: its sessions end, its
    /// connections close (but `except`, which closes itself).
    fn drop_device(&mut self, d: u64, except: Option<u64>) {
        self.end_where(|s| s.host == d || s.client == d);
        self.hosts.remove(&d);
        for (id, c) in &self.conns {
            if c.device == d && Some(*id) != except {
                let _ = c.outbox.send(Out::Close);
            }
        }
    }

    fn end_where(&mut self, f: impl Fn(&Session) -> bool) {
        let ids: Vec<u64> = self
            .sessions
            .iter()
            .filter(|(_, s)| f(s))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.end(id);
        }
    }

    fn updating(&self, host: u64) -> bool {
        self.updating
            .get(&host)
            .is_some_and(|since| since.elapsed() < UPDATING_FOR)
    }

    fn presence(&self, host: u64, client: u64) -> Presence {
        let busy = self
            .sessions
            .values()
            .any(|s| s.host == host && s.client != client);
        if self.updating(host) {
            return Presence::Updating;
        }
        if self
            .restarting
            .get(&host)
            .is_some_and(|since| since.elapsed() < RESTARTING_FOR)
        {
            return Presence::Restarting;
        }
        match self.hosts.get(&host) {
            None => Presence::Offline,
            Some(h) if !h.active => Presence::Inactive,
            Some(_) if busy => Presence::Busy,
            Some(_) => Presence::Online,
        }
    }
}

/// What all connections share.
pub struct Hub {
    pub(crate) state: Mutex<State>,
    pub(crate) db_path: PathBuf,
    pub(crate) login_timeout: Duration,
    pub(crate) idle_timeout: Duration,
    pub(crate) update: Option<UpdateFile>,
    pub(crate) downloads: Downloads,
}

impl Hub {
    /// Adds a logged-in connection. A host has one: a newer one closes the
    /// older (and its session). A client may have several (its app and its
    /// video window). Returns the connection id.
    pub(crate) fn attach(&self, device: u64, role: Role, outbox: Sender<Out>) -> u64 {
        let mut st = self.state.lock().unwrap();
        st.next_conn += 1;
        let conn = st.next_conn;
        if role == Role::Host {
            // Back, newer or not: no longer «updating» nor «restarting».
            st.updating.remove(&device);
            st.restarting.remove(&device);
            if let Some(old) = st.hosts.remove(&device) {
                st.end_where(|s| s.host == device);
                if let Some(c) = st.conns.remove(&old.conn) {
                    let _ = c.outbox.send(Out::Close);
                }
            }
            // A host connection that has not sent HostUpdate yet.
            let older: Vec<u64> = st
                .conns
                .iter()
                .filter(|(_, c)| c.device == device)
                .map(|(id, _)| *id)
                .collect();
            for id in older {
                if let Some(c) = st.conns.remove(&id) {
                    let _ = c.outbox.send(Out::Close);
                }
            }
        }
        st.conns.insert(
            conn,
            Conn {
                device,
                role,
                outbox,
            },
        );
        conn
    }

    pub(crate) fn detach(&self, conn: u64) {
        let mut st = self.state.lock().unwrap();
        let Some(c) = st.conns.remove(&conn) else {
            return;
        };
        if c.role == Role::Host && st.hosts.get(&c.device).is_some_and(|h| h.conn == conn) {
            st.end_where(|s| s.host == c.device);
            st.hosts.remove(&c.device);
        }
        st.end_where(|s| s.client_conn == conn);
    }

    pub(crate) fn host_update(&self, device: u64, conn: u64, addrs: Vec<String>, active: bool) {
        let mut st = self.state.lock().unwrap();
        if !st.conns.contains_key(&conn) {
            return; // replaced meanwhile
        }
        if !active {
            st.end_where(|s| s.host == device);
        }
        st.hosts.insert(
            device,
            OnlineHost {
                conn,
                addrs,
                active,
            },
        );
    }

    pub(crate) fn presence(&self, host: u64, client: u64) -> Presence {
        self.state.lock().unwrap().presence(host, client)
    }

    /// Sets up a session (access already checked): ends the client's older
    /// session, sends `Incoming` to the host, returns the client's `Session`.
    pub(crate) fn connect(
        &self,
        client: &SessionClient<'_>,
        host: (u64, [u8; NOISE_KEY_LEN]),
        quality: Quality,
        files: FilePolicy,
    ) -> std::result::Result<SignalMessage, ConnectFailure> {
        let (host, host_key) = host;
        let mut st = self.state.lock().unwrap();
        match st.presence(host, client.device) {
            Presence::Offline | Presence::Updating | Presence::Restarting => {
                return Err(ConnectFailure::Offline);
            }
            Presence::Inactive => return Err(ConnectFailure::Inactive),
            Presence::Busy => return Err(ConnectFailure::Busy),
            Presence::Online => {}
        }
        // One session per client: a new Connect ends the previous one.
        st.end_where(|s| s.client == client.device);
        let id = loop {
            let id = u64::from_le_bytes(random_bytes());
            if id != 0 && !st.sessions.contains_key(&id) {
                break id;
            }
        };
        st.sessions.insert(
            id,
            Session {
                host,
                client: client.device,
                client_conn: client.conn,
                client_name: client.name.to_string(),
                quality,
                files,
                started: Instant::now(),
            },
        );
        let h = &st.hosts[&host];
        let (host_conn, addrs) = (h.conn, h.addrs.clone());
        st.send(
            host_conn,
            SignalMessage::Incoming {
                id,
                client_key: client.key,
                client_name: client.name.to_string(),
                quality,
                files,
            },
        );
        Ok(SignalMessage::Session {
            id,
            host_key,
            addrs,
            quality,
            files,
        })
    }

    /// A client of a newer `version` wants `host`: tell the host, if it is
    /// connected, free and not updating already (busy: no update, the client
    /// is turned away as usual). True if told.
    pub(crate) fn want_update(&self, host: u64, version: &str) -> bool {
        let st = self.state.lock().unwrap();
        let busy = st.sessions.values().any(|s| s.host == host);
        if busy || st.updating(host) {
            return false;
        }
        let Some(h) = st.hosts.get(&host) else {
            return false;
        };
        st.send(
            h.conn,
            SignalMessage::UpdateWanted {
                version: version.to_string(),
            },
        );
        true
    }

    /// A client asked to restart `host`'s machine: tell the host, if it is
    /// connected and not updating (a restart mid-update could break it).
    pub(crate) fn want_reboot(
        &self,
        host: u64,
        client_name: &str,
    ) -> std::result::Result<(), ConnectFailure> {
        let mut st = self.state.lock().unwrap();
        if st.updating(host) {
            return Err(ConnectFailure::Offline);
        }
        let Some(h) = st.hosts.get(&host) else {
            return Err(ConnectFailure::Offline);
        };
        st.send(
            h.conn,
            SignalMessage::RebootWanted {
                client_name: client_name.to_string(),
            },
        );
        // «Restarting» at once: no connecting to a machine about to go down.
        st.restarting.insert(host, Instant::now());
        Ok(())
    }

    /// The host says it is updating (`on`) or not any more.
    pub(crate) fn set_updating(&self, host: u64, on: bool) {
        let mut st = self.state.lock().unwrap();
        if on {
            st.updating.insert(host, Instant::now());
        } else {
            st.updating.remove(&host);
        }
    }

    /// Hosts updating now, for the admin.
    pub fn updating_hosts(&self) -> Vec<u64> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<u64> = st
            .updating
            .keys()
            .copied()
            .filter(|h| st.updating(*h))
            .collect();
        v.sort_unstable();
        v
    }

    /// The device removed itself: what it had going ends now, not at the
    /// next recheck.
    pub(crate) fn unregistered(&self, device: u64, conn: u64) {
        self.state.lock().unwrap().drop_device(device, Some(conn));
    }

    /// Ends session `id` if `device` takes part in it.
    pub(crate) fn end_session(&self, id: u64, device: u64) {
        let mut st = self.state.lock().unwrap();
        if st
            .sessions
            .get(&id)
            .is_some_and(|s| s.host == device || s.client == device)
        {
            st.end(id);
        }
    }

    /// Running sessions, for the admin (newest first).
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<SessionInfo> = st
            .sessions
            .iter()
            .map(|(id, s)| SessionInfo {
                id: *id,
                host: s.host,
                client: s.client,
                client_name: s.client_name.clone(),
                quality: s.quality,
                duration: s.started.elapsed(),
            })
            .collect();
        v.sort_by_key(|s| s.duration);
        v
    }

    /// Clients with at least one connection.
    pub fn online_clients(&self) -> Vec<u64> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<u64> = st
            .conns
            .values()
            .filter(|c| c.role == Role::Client)
            .map(|c| c.device)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Hosts with a connection: (device, active).
    pub fn online_hosts(&self) -> Vec<(u64, bool)> {
        let st = self.state.lock().unwrap();
        st.hosts.iter().map(|(d, h)| (*d, h.active)).collect()
    }

    /// Ends what the database no longer allows: sessions whose access is gone,
    /// connections of removed devices; tells both sides of a session when its
    /// file switch or speed cap changed (a transfer stops at once).
    pub(crate) fn recheck(&self, db: &Db) -> Result<()> {
        let files: Vec<(u64, u64, FilePolicy)> = {
            let st = self.state.lock().unwrap();
            st.sessions
                .iter()
                .map(|(id, s)| (*id, s.client, s.files))
                .collect()
        };
        for (id, client, was) in files {
            let now = db.files_policy(client)?;
            if now == was {
                continue;
            }
            {
                let mut st = self.state.lock().unwrap();
                let Some(s) = st.sessions.get_mut(&id) else {
                    continue;
                };
                s.files = now;
                let (host, client_conn) = (s.host, s.client_conn);
                let msg = SignalMessage::SessionFiles { id, files: now };
                if let Some(h) = st.hosts.get(&host) {
                    st.send(h.conn, msg.clone());
                }
                st.send(client_conn, msg);
            }
            println!(
                "session {id:016x}: files {} at {} Mbit/s",
                if now.allowed { "allowed" } else { "off" },
                now.mbps
            );
        }
        let (sessions, devices): (Vec<(u64, u64, u64)>, Vec<u64>) = {
            let st = self.state.lock().unwrap();
            let mut devices: Vec<u64> = st.conns.values().map(|c| c.device).collect();
            devices.sort_unstable();
            devices.dedup();
            (
                st.sessions
                    .iter()
                    .map(|(id, s)| (*id, s.client, s.host))
                    .collect(),
                devices,
            )
        };
        let mut gone_sessions = Vec::new();
        for (id, client, host) in sessions {
            if db.access(client, host)? != Access::Approved {
                gone_sessions.push(id);
            }
        }
        let mut gone_devices = Vec::new();
        for d in devices {
            if !db.device_exists(d)? {
                gone_devices.push(d);
            }
        }
        if gone_sessions.is_empty() && gone_devices.is_empty() {
            return Ok(());
        }
        let mut st = self.state.lock().unwrap();
        for id in gone_sessions {
            println!("session {id:016x}: access taken back");
            st.end(id);
        }
        for d in gone_devices {
            println!("device {d} removed: closing its connections");
            st.drop_device(d, None);
        }
        Ok(())
    }
}

/// The client side of a new session.
pub(crate) struct SessionClient<'a> {
    pub(crate) device: u64,
    pub(crate) conn: u64,
    pub(crate) name: &'a str,
    pub(crate) key: [u8; NOISE_KEY_LEN],
}
