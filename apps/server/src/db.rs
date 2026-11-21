//! Devices, access of clients to hosts, quality overrides and bitrate caps in
//! SQLite.
//!
//! The admin (CLI, web) and the running server open the same file; the server
//! reads it on every request and re-checks running sessions, so admin changes
//! apply without a restart. Device secrets are 32 random bytes; only their
//! BLAKE2s hash is stored. The server never sees a host's token, only the
//! `lookup` derived from it.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use blake2::{Blake2s256, Digest};
use protocol::signal::{
    Access, CapRule, FilePolicy, LOOKUP_LEN, Limits, NOISE_KEY_LEN, Role, SECRET_LEN, name_allowed,
};
use rusqlite::{Connection, OptionalExtension, params};

/// Picture size classes of the cap table.
pub const TIERS: [(&str, u32); 3] = [("fhd", 1920 * 1080), ("2k", 2560 * 1440), ("4k", u32::MAX)];

/// Default caps, Mbit/s of video (the office worst case
/// fits in 1 Gbit/s).
const DEFAULT_CAPS: [(&str, u16, f32); 6] = [
    ("fhd", 60, 10.0),
    ("2k", 60, 18.0),
    ("4k", 60, 40.0),
    ("fhd", 30, 6.0),
    ("2k", 30, 11.0),
    ("4k", 30, 24.0),
];

/// `PRAGMA user_version` of this schema. Older databases (stage 3: users and
/// links) lose those tables; the cap table is kept. 5: `devices.version`.
/// 6: file transfers (`settings`, `clients.files`). 7: the frame rate and the
/// cap each on its own. 8: the admin's name for a client again (`clients.admin_name`).
/// 9: a host's last failed update (`hosts.update_failed`).
/// 10: a host's update switch (`hosts.auto_update`).
const SCHEMA: i64 = 10;

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Host => "host",
        Role::Client => "client",
    }
}

fn parse_role(s: &str) -> rusqlite::Result<Role> {
    match s {
        "host" => Ok(Role::Host),
        "client" => Ok(Role::Client),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub fn access_name(a: Access) -> &'static str {
    match a {
        Access::None => "none",
        Access::Pending => "pending",
        Access::Approved => "approved",
        Access::Rejected => "rejected",
    }
}

fn parse_access(s: &str) -> Access {
    match s {
        "pending" => Access::Pending,
        "approved" => Access::Approved,
        "rejected" => Access::Rejected,
        _ => Access::None,
    }
}

fn hash_secret(secret: &[u8]) -> Vec<u8> {
    Blake2s256::digest(secret).to_vec()
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .expect("system random");
    b
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A name people give (a machine's, a client's, the admin's for a client): trimmed, Latin only
/// (`protocol::signal::name_allowed`, owner's decision 2026-11-09).
pub fn given_name(s: &str) -> Option<String> {
    name_allowed(s).then(|| s.trim().to_string())
}

/// The Lumen version a device sent: printable ASCII, at most 32 bytes
/// (anything else is cut off).
pub fn clean_version(s: &str) -> String {
    s.trim()
        .chars()
        .take_while(|c| c.is_ascii_graphic())
        .take(32)
        .collect()
}

/// How a host is shown: `machine`, or `machine / account` for a host with an
/// account (rows of per-account hosts from before the service may remain; new
/// hosts are the machine's and send an empty account).
pub fn host_label(machine: &str, account: &str) -> String {
    if account.is_empty() {
        machine.to_string()
    } else {
        format!("{machine} / {account}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostRow {
    pub id: u64,
    pub machine: String,
    /// Empty for the machine's host (the service's); old per-account rows keep theirs.
    pub account: String,
    pub noise_key: [u8; NOISE_KEY_LEN],
    /// Unix seconds; 0 if never.
    pub last_seen: i64,
    /// The Lumen version it last logged in with (empty: never).
    pub version: String,
    /// Its last update failed: the host's `err_update_…` code (empty: none).
    pub update_failed: String,
    /// It updates itself when a newer client connects (as it last said; on until it says).
    pub auto_update: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientRow {
    pub id: u64,
    /// As the client calls itself in Lumen Client.
    pub name: String,
    /// What the admin calls it, seen only in the admin panel; `None`: not set.
    pub admin_name: Option<String>,
    pub noise_key: [u8; NOISE_KEY_LEN],
    /// What the admin sets for this client alone.
    pub limits: Limits,
    /// The admin's file transfer switch for this client: `None` — as everyone.
    pub files: Option<bool>,
    pub last_seen: i64,
    /// The Lumen version it last logged in with (empty: never).
    pub version: String,
}

impl HostRow {
    pub fn label(&self) -> String {
        host_label(&self.machine, &self.account)
    }
}

impl AccessRow {
    /// The host of this access, as [`HostRow::label`] shows it.
    pub fn label(&self) -> String {
        host_label(&self.machine, &self.account)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessRow {
    pub client: u64,
    pub client_name: String,
    pub host: u64,
    pub machine: String,
    pub account: String,
    pub access: Access,
    /// When asked (Unix seconds).
    pub requested: i64,
    /// When the admin decided (Unix seconds), if they did.
    pub decided: Option<i64>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Opens (creating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        Self::create_tables(&conn, version)?;
        Self::migrate(&conn)?;
        conn.pragma_update(None, "user_version", SCHEMA)?;
        let db = Db { conn };
        Self::seed_caps(&db)?;
        Ok(db)
    }

    /// Drops the tables of old schemas, creates the current ones.
    fn create_tables(conn: &Connection, version: i64) -> Result<()> {
        if version < SCHEMA {
            conn.execute_batch(
                "DROP TABLE IF EXISTS links;
                 DROP TABLE IF EXISTS users;",
            )?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS devices (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 role TEXT NOT NULL CHECK (role IN ('host', 'client')),
                 secret_hash BLOB NOT NULL,
                 noise_key BLOB NOT NULL,
                 created INTEGER NOT NULL,
                 last_seen INTEGER NOT NULL DEFAULT 0,
                 version TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS hosts (
                 device INTEGER PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
                 machine TEXT NOT NULL DEFAULT '',
                 account TEXT NOT NULL DEFAULT '',
                 lookup BLOB UNIQUE,
                 update_failed TEXT NOT NULL DEFAULT '',
                 auto_update INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE IF NOT EXISTS clients (
                 device INTEGER PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 limit_fps INTEGER,
                 limit_mbps REAL,
                 admin_name TEXT
             );
             CREATE TABLE IF NOT EXISTS access (
                 client INTEGER NOT NULL REFERENCES clients(device) ON DELETE CASCADE,
                 host INTEGER NOT NULL REFERENCES hosts(device) ON DELETE CASCADE,
                 status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected')),
                 requested INTEGER NOT NULL,
                 decided INTEGER,
                 PRIMARY KEY (client, host)
             );
             CREATE TABLE IF NOT EXISTS global_override (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 limit_fps INTEGER,
                 limit_mbps REAL
             );
             INSERT OR IGNORE INTO global_override (id) VALUES (1);
             CREATE TABLE IF NOT EXISTS settings (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 files_on INTEGER NOT NULL,
                 files_mbps INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO settings VALUES (1, 1, 100);
             CREATE TABLE IF NOT EXISTS caps (
                 tier TEXT NOT NULL,
                 fps INTEGER NOT NULL,
                 mbps REAL NOT NULL,
                 PRIMARY KEY (tier, fps)
             );",
        )?;
        Ok(())
    }

    /// Brings a database of an older schema up to date, column by column.
    fn migrate(conn: &Connection) -> Result<()> {
        // A schema 4 database: its devices get the version column.
        if !has_column(conn, "devices", "version")? {
            conn.execute_batch("ALTER TABLE devices ADD COLUMN version TEXT NOT NULL DEFAULT ''")?;
        }
        // Before schema 6: a client's file switch.
        if !has_column(conn, "clients", "files")? {
            conn.execute_batch("ALTER TABLE clients ADD COLUMN files INTEGER")?;
        }
        // Before schema 7: one override for the frame rate and the cap together.
        for table in ["clients", "global_override"] {
            if !has_column(conn, table, "limit_fps")? {
                conn.execute_batch(&format!(
                    "BEGIN;
                     ALTER TABLE {table} ADD COLUMN limit_fps INTEGER;
                     ALTER TABLE {table} ADD COLUMN limit_mbps REAL;
                     UPDATE {table} SET limit_fps = override_fps, limit_mbps = override_mbps
                         WHERE override_on;
                     ALTER TABLE {table} DROP COLUMN override_on;
                     ALTER TABLE {table} DROP COLUMN override_fps;
                     ALTER TABLE {table} DROP COLUMN override_mbps;
                     COMMIT;"
                ))?;
            }
        }
        // Schema 7 had no admin's name (a schema 6 database keeps its own).
        if !has_column(conn, "clients", "admin_name")? {
            conn.execute_batch("ALTER TABLE clients ADD COLUMN admin_name TEXT")?;
        }
        // Before schema 9: no update failures.
        if !has_column(conn, "hosts", "update_failed")? {
            conn.execute_batch(
                "ALTER TABLE hosts ADD COLUMN update_failed TEXT NOT NULL DEFAULT ''",
            )?;
        }
        // Before schema 10: the switch as by default, on.
        if !has_column(conn, "hosts", "auto_update")? {
            conn.execute_batch(
                "ALTER TABLE hosts ADD COLUMN auto_update INTEGER NOT NULL DEFAULT 1",
            )?;
        }
        Ok(())
    }

    /// The default caps of a database that has none.
    fn seed_caps(db: &Db) -> Result<()> {
        let caps: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM caps", [], |r| r.get(0))?;
        if caps == 0 {
            for (tier, fps, mbps) in DEFAULT_CAPS {
                db.set_cap(tier, fps, mbps)?;
            }
        }
        Ok(())
    }

    // ---- devices -------------------------------------------------------

    /// Creates a device; returns its id and secret (the device keeps them).
    /// Clients need a valid `name`; a host's names come with `HostUpdate`.
    pub fn register(
        &self,
        role: Role,
        name: &str,
        noise_key: [u8; NOISE_KEY_LEN],
    ) -> Result<(u64, [u8; SECRET_LEN])> {
        let client_name = match role {
            Role::Client => Some(given_name(name).context("bad client name")?),
            Role::Host => None,
        };
        let secret: [u8; SECRET_LEN] = random_bytes();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO devices (role, secret_hash, noise_key, created) VALUES (?1, ?2, ?3, ?4)",
            params![role_name(role), hash_secret(&secret), &noise_key[..], now()],
        )?;
        let id = tx.last_insert_rowid();
        match client_name {
            Some(name) => tx.execute(
                "INSERT INTO clients (device, name) VALUES (?1, ?2)",
                params![id, name],
            )?,
            None => tx.execute("INSERT INTO hosts (device) VALUES (?1)", [id])?,
        };
        tx.commit()?;
        Ok((id as u64, secret))
    }

    /// The device's role and Noise key if `secret` is right.
    pub fn check_login(
        &self,
        device: u64,
        secret: &[u8; SECRET_LEN],
    ) -> Result<Option<(Role, [u8; NOISE_KEY_LEN])>> {
        let row: Option<(String, Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT role, secret_hash, noise_key FROM devices WHERE id = ?1",
                [device as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            Some((role, hash, key)) if hash == hash_secret(secret) => {
                Some((parse_role(&role)?, key_array(&key)?))
            }
            _ => None,
        })
    }

    pub fn device_exists(&self, device: u64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM devices WHERE id = ?1",
                [device as i64],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The device logged in now, with this version of Lumen.
    pub fn touch(&self, device: u64, version: &str) -> Result<()> {
        // A host back with another version: its failed update is over.
        self.conn.execute(
            "UPDATE hosts SET update_failed = '' WHERE device = ?1
                 AND (SELECT version FROM devices WHERE id = ?1) != ?2",
            params![device as i64, version],
        )?;
        self.conn.execute(
            "UPDATE devices SET last_seen = ?2, version = ?3 WHERE id = ?1",
            params![device as i64, now(), version],
        )?;
        Ok(())
    }

    /// Removes a device with its access rows. False if there was none.
    pub fn remove_device(&self, device: u64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM devices WHERE id = ?1", [device as i64])?
            > 0)
    }

    // ---- hosts ---------------------------------------------------------

    /// A host's names and the `lookup` of its current token.
    pub fn update_host(
        &self,
        host: u64,
        machine: &str,
        account: &str,
        lookup: &[u8; LOOKUP_LEN],
    ) -> Result<()> {
        let machine = given_name(machine).context("bad machine name")?;
        // None: the machine's host.
        let account = match account.trim() {
            "" => String::new(),
            a => given_name(a).context("bad account name")?,
        };
        let n = self
            .conn
            .execute(
                "UPDATE hosts SET machine = ?2, account = ?3, lookup = ?4 WHERE device = ?1",
                params![host as i64, machine, account, &lookup[..]],
            )
            .context("this token belongs to another host")?;
        if n == 0 {
            bail!("no host {host}");
        }
        Ok(())
    }

    const HOST_COLUMNS: &str = "h.device, h.machine, h.account, d.noise_key, d.last_seen, d.version, h.update_failed, h.auto_update";

    fn host_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostRow> {
        Ok(HostRow {
            id: r.get::<_, i64>(0)? as u64,
            machine: r.get(1)?,
            account: r.get(2)?,
            noise_key: key_array(&r.get::<_, Vec<u8>>(3)?)?,
            last_seen: r.get(4)?,
            version: r.get(5)?,
            update_failed: r.get(6)?,
            auto_update: r.get(7)?,
        })
    }

    /// A host's update failed with this code; empty: it did not (or no more).
    pub fn set_update_failed(&self, host: u64, code: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE hosts SET update_failed = ?2 WHERE device = ?1",
            params![host as i64, code],
        )?;
        Ok(())
    }

    /// The host's update switch, as it says.
    pub fn set_auto_update(&self, host: u64, on: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE hosts SET auto_update = ?2 WHERE device = ?1",
            params![host as i64, on],
        )?;
        Ok(())
    }

    pub fn host_by_lookup(&self, lookup: &[u8; LOOKUP_LEN]) -> Result<Option<HostRow>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM hosts h JOIN devices d ON d.id = h.device WHERE h.lookup = ?1",
                    Self::HOST_COLUMNS
                ),
                [&lookup[..]],
                Self::host_row,
            )
            .optional()?)
    }

    pub fn host(&self, id: u64) -> Result<Option<HostRow>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM hosts h JOIN devices d ON d.id = h.device WHERE h.device = ?1",
                    Self::HOST_COLUMNS
                ),
                [id as i64],
                Self::host_row,
            )
            .optional()?)
    }

    pub fn hosts(&self) -> Result<Vec<HostRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM hosts h JOIN devices d ON d.id = h.device
             ORDER BY h.machine, h.account, h.device",
            Self::HOST_COLUMNS
        ))?;
        let rows = st.query_map([], Self::host_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- clients -------------------------------------------------------

    const CLIENT_COLUMNS: &str = "c.device, c.name, d.noise_key, c.limit_fps, c.limit_mbps, \
                                  d.last_seen, d.version, c.files, c.admin_name";

    fn client_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClientRow> {
        Ok(ClientRow {
            id: r.get::<_, i64>(0)? as u64,
            name: r.get(1)?,
            noise_key: key_array(&r.get::<_, Vec<u8>>(2)?)?,
            limits: Limits {
                fps: r.get(3)?,
                mbps: r.get(4)?,
            },
            last_seen: r.get(5)?,
            version: r.get(6)?,
            files: r.get(7)?,
            admin_name: r.get(8)?,
        })
    }

    pub fn client(&self, id: u64) -> Result<Option<ClientRow>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM clients c JOIN devices d ON d.id = c.device WHERE c.device = ?1",
                    Self::CLIENT_COLUMNS
                ),
                [id as i64],
                Self::client_row,
            )
            .optional()?)
    }

    pub fn clients(&self) -> Result<Vec<ClientRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM clients c JOIN devices d ON d.id = c.device
             ORDER BY c.name, c.device",
            Self::CLIENT_COLUMNS
        ))?;
        let rows = st.query_map([], Self::client_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The client's own name for itself.
    pub fn set_client_name(&self, client: u64, name: &str) -> Result<()> {
        let name = given_name(name).context("bad client name")?;
        self.conn.execute(
            "UPDATE clients SET name = ?2 WHERE device = ?1",
            params![client as i64, name],
        )?;
        Ok(())
    }

    /// The admin's name for a client; `None` (or blank) clears it.
    pub fn set_admin_name(&self, client: u64, name: Option<&str>) -> Result<()> {
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => Some(given_name(n).context("bad client name")?),
            None => None,
        };
        let n = self.conn.execute(
            "UPDATE clients SET admin_name = ?2 WHERE device = ?1",
            params![client as i64, name],
        )?;
        if n == 0 {
            bail!("no client {client}");
        }
        Ok(())
    }

    // ---- access --------------------------------------------------------

    pub fn access(&self, client: u64, host: u64) -> Result<Access> {
        Ok(self
            .conn
            .query_row(
                "SELECT status FROM access WHERE client = ?1 AND host = ?2",
                [client as i64, host as i64],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map_or(Access::None, |s| parse_access(&s)))
    }

    /// Files a request for the admin unless the client already asked (or was
    /// approved, or rejected: a rejected client cannot ask again). Returns the
    /// access after that.
    pub fn request_access(&self, client: u64, host: u64) -> Result<Access> {
        self.conn.execute(
            "INSERT OR IGNORE INTO access (client, host, status, requested)
             VALUES (?1, ?2, 'pending', ?3)",
            params![client as i64, host as i64, now()],
        )?;
        self.access(client, host)
    }

    /// The client deleted its card. A rejection stays (so it cannot re-ask).
    pub fn forget(&self, client: u64, host: u64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM access WHERE client = ?1 AND host = ?2 AND status != 'rejected'",
            [client as i64, host as i64],
        )?;
        Ok(())
    }

    fn decide(&self, client: u64, host: u64, status: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE access SET status = ?3, decided = ?4 WHERE client = ?1 AND host = ?2",
            params![client as i64, host as i64, status, now()],
        )?;
        if n == 0 {
            bail!("client {client} has not asked for host {host}");
        }
        Ok(())
    }

    pub fn approve(&self, client: u64, host: u64) -> Result<()> {
        self.decide(client, host, "approved")
    }

    pub fn reject(&self, client: u64, host: u64) -> Result<()> {
        self.decide(client, host, "rejected")
    }

    /// Takes access back (or lifts a rejection): the client may ask again.
    pub fn unlink(&self, client: u64, host: u64) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM access WHERE client = ?1 AND host = ?2",
            [client as i64, host as i64],
        )? > 0)
    }

    /// Takes back every access of a client that was approved or pending.
    /// Returns how many.
    pub fn unlink_all(&self, client: u64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM access WHERE client = ?1 AND status != 'rejected'",
            [client as i64],
        )?)
    }

    /// Access rows, newest requests first; only those with `status` if given.
    pub fn access_list(&self, status: Option<Access>) -> Result<Vec<AccessRow>> {
        let mut st = self.conn.prepare(
            "SELECT a.client, c.name, a.host, h.machine, h.account,
                    a.status, a.requested, a.decided
             FROM access a
             JOIN clients c ON c.device = a.client
             JOIN hosts h ON h.device = a.host
             WHERE ?1 IS NULL OR a.status = ?1
             ORDER BY a.requested DESC, a.client, a.host",
        )?;
        let rows = st.query_map([status.map(access_name)], |r| {
            Ok(AccessRow {
                client: r.get::<_, i64>(0)? as u64,
                client_name: r.get(1)?,
                host: r.get::<_, i64>(2)? as u64,
                machine: r.get(3)?,
                account: r.get(4)?,
                access: parse_access(&r.get::<_, String>(5)?),
                requested: r.get(6)?,
                decided: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- quality -------------------------------------------------------

    pub fn global_limits(&self) -> Result<Limits> {
        Ok(self.conn.query_row(
            "SELECT limit_fps, limit_mbps FROM global_override",
            [],
            |r| {
                Ok(Limits {
                    fps: r.get(0)?,
                    mbps: r.get(1)?,
                })
            },
        )?)
    }

    pub fn set_global_limits(&self, l: Limits) -> Result<()> {
        check_limits(l)?;
        self.conn.execute(
            "UPDATE global_override SET limit_fps = ?1, limit_mbps = ?2",
            params![l.fps, l.mbps],
        )?;
        Ok(())
    }

    pub fn set_client_limits(&self, client: u64, l: Limits) -> Result<()> {
        check_limits(l)?;
        let n = self.conn.execute(
            "UPDATE clients SET limit_fps = ?2, limit_mbps = ?3 WHERE device = ?1",
            params![client as i64, l.fps, l.mbps],
        )?;
        if n == 0 {
            bail!("no client {client}");
        }
        Ok(())
    }

    /// What the admin sets for this client: its own limits, each over
    /// everyone's; what neither sets, the client chooses.
    pub fn forced_limits(&self, client: u64) -> Result<Limits> {
        let own = self.client(client)?.map_or(Limits::default(), |c| c.limits);
        Ok(own.over(self.global_limits()?))
    }

    // ---- files ---------------------------------------------------------

    /// File transfers for everyone (unless a client has its own switch), and
    /// their speed cap, Mbit/s.
    pub fn global_files(&self) -> Result<FilePolicy> {
        let (allowed, mbps): (bool, u16) =
            self.conn
                .query_row("SELECT files_on, files_mbps FROM settings", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok(FilePolicy { allowed, mbps })
    }

    pub fn set_global_files(&self, allowed: bool) -> Result<()> {
        self.conn
            .execute("UPDATE settings SET files_on = ?1", params![allowed])?;
        Ok(())
    }

    pub fn set_files_mbps(&self, mbps: u16) -> Result<()> {
        if !(1..=1000).contains(&mbps) {
            bail!("the file speed cap must be 1 .. 1000 Mbit/s");
        }
        self.conn
            .execute("UPDATE settings SET files_mbps = ?1", params![mbps])?;
        Ok(())
    }

    /// `None`: the client follows the global switch.
    pub fn set_client_files(&self, client: u64, allowed: Option<bool>) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE clients SET files = ?2 WHERE device = ?1",
            params![client as i64, allowed],
        )?;
        if n == 0 {
            bail!("no client {client}");
        }
        Ok(())
    }

    /// What a session of this client may do with files: its own switch, else
    /// the global one; the server's speed cap.
    pub fn files_policy(&self, client: u64) -> Result<FilePolicy> {
        let global = self.global_files()?;
        let own = self.client(client)?.and_then(|c| c.files);
        Ok(FilePolicy {
            allowed: own.unwrap_or(global.allowed),
            mbps: global.mbps,
        })
    }

    // ---- caps ----------------------------------------------------------

    pub fn set_cap(&self, tier: &str, fps: u16, mbps: f32) -> Result<()> {
        if !TIERS.iter().any(|(t, _)| *t == tier) {
            bail!("tier must be one of fhd, 2k, 4k");
        }
        if !(mbps > 0.0 && mbps.is_finite()) {
            bail!("the cap must be a positive number of Mbit/s");
        }
        self.conn.execute(
            "INSERT INTO caps (tier, fps, mbps) VALUES (?1, ?2, ?3)
             ON CONFLICT (tier, fps) DO UPDATE SET mbps = excluded.mbps",
            params![tier, fps, mbps],
        )?;
        Ok(())
    }

    /// (tier, fps, Mbit/s), as stored.
    pub fn cap_table(&self) -> Result<Vec<(String, u16, f32)>> {
        let mut st = self
            .conn
            .prepare("SELECT tier, fps, mbps FROM caps ORDER BY fps DESC, tier")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The cap table as sent to hosts.
    pub fn caps(&self) -> Result<Vec<CapRule>> {
        Ok(self
            .cap_table()?
            .into_iter()
            .filter_map(|(tier, fps, mbps)| {
                let max_pixels = TIERS.iter().find(|(t, _)| *t == tier)?.1;
                Some(CapRule {
                    max_pixels,
                    fps,
                    mbps,
                })
            })
            .collect())
    }
}

fn check_limits(l: Limits) -> Result<()> {
    if !l.is_valid() {
        bail!("fps must be 30 or 60, the cap 0 (auto) .. 1000 Mbit/s");
    }
    Ok(())
}

/// Whether `table` has `column` (an older schema may not).
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |r| r.get(0),
    )?)
}

fn key_array(v: &[u8]) -> rusqlite::Result<[u8; NOISE_KEY_LEN]> {
    v.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new empty folder for one test's database: its `lumen.db` path.
    fn temp_db_path() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lumen-db-test-{}-{}",
            std::process::id(),
            u64::from_le_bytes(random_bytes())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("lumen.db")
    }

    /// A fresh database file (left in the temp directory).
    fn db() -> Db {
        Db::open(&temp_db_path()).unwrap()
    }

    #[test]
    fn devices_log_in_with_their_secret() {
        let db = db();
        let (c, secret) = db.register(Role::Client, " R3tr0 ", [1; 32]).unwrap();
        assert_eq!(
            db.check_login(c, &secret).unwrap(),
            Some((Role::Client, [1; 32]))
        );
        assert_eq!(db.check_login(c, &[0; 32]).unwrap(), None);
        assert_eq!(db.check_login(c + 1, &secret).unwrap(), None);
        assert_eq!(db.client(c).unwrap().unwrap().name, "R3tr0");
        assert!(db.register(Role::Client, "  ", [1; 32]).is_err());
        let (h, _) = db.register(Role::Host, "", [2; 32]).unwrap();
        assert!(db.host(h).unwrap().is_some());
        assert!(db.remove_device(h).unwrap());
        assert!(!db.device_exists(h).unwrap());
    }

    #[test]
    fn the_admin_names_a_client() {
        let db = db();
        let (c, _) = db.register(Role::Client, "R3tr0", [1; 32]).unwrap();
        assert_eq!(db.client(c).unwrap().unwrap().admin_name, None);
        db.set_admin_name(c, Some("  Ivan, design  ")).unwrap();
        let row = db.client(c).unwrap().unwrap();
        assert_eq!(row.admin_name.as_deref(), Some("Ivan, design"));
        // The client's own name stays, and changes on its own.
        db.set_client_name(c, "R3tr0-laptop").unwrap();
        let row = db.client(c).unwrap().unwrap();
        assert_eq!(
            (row.name.as_str(), row.admin_name.as_deref()),
            ("R3tr0-laptop", Some("Ivan, design"))
        );
        assert_eq!(
            db.clients().unwrap()[0].admin_name.as_deref(),
            Some("Ivan, design")
        );
        // Bad names are refused and change nothing.
        assert!(
            db.set_admin_name(c, Some(&"x".repeat(protocol::signal::MAX_NAME + 1)))
                .is_err()
        );
        assert!(db.set_admin_name(c, Some("a\nb")).is_err());
        assert!(db.set_admin_name(c + 100, Some("x")).is_err());
        assert_eq!(
            db.client(c).unwrap().unwrap().admin_name.as_deref(),
            Some("Ivan, design")
        );
        // Blank or none: cleared.
        db.set_admin_name(c, Some("  ")).unwrap();
        assert_eq!(db.client(c).unwrap().unwrap().admin_name, None);
        db.set_admin_name(c, Some("x")).unwrap();
        db.set_admin_name(c, None).unwrap();
        assert_eq!(db.client(c).unwrap().unwrap().admin_name, None);
    }

    /// A schema 7 database (no `clients.admin_name`) gets the column, empty.
    #[test]
    fn a_schema_7_database_gets_admin_names() {
        let path = temp_db_path();
        let c = {
            let db = Db::open(&path).unwrap();
            let (c, _) = db.register(Role::Client, "anna", [1; 32]).unwrap();
            db.conn
                .execute_batch(
                    "ALTER TABLE clients DROP COLUMN admin_name;
                     PRAGMA user_version = 7;",
                )
                .unwrap();
            c
        };
        // Opened again, the name set stays.
        for expected in [None, Some("Anna".to_string())] {
            let db = Db::open(&path).unwrap();
            let anna = db.client(c).unwrap().unwrap();
            assert_eq!((anna.name.as_str(), anna.admin_name), ("anna", expected));
            db.set_admin_name(c, Some("Anna")).unwrap();
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// A schema 4 database (no `devices.version`) opens; its devices keep
    /// what they had, the version comes with the next login.
    #[test]
    fn a_schema_4_database_gets_versions() {
        let path = temp_db_path();
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE devices (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     role TEXT NOT NULL CHECK (role IN ('host', 'client')),
                     secret_hash BLOB NOT NULL,
                     noise_key BLOB NOT NULL,
                     created INTEGER NOT NULL,
                     last_seen INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE clients (
                     device INTEGER PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
                     name TEXT NOT NULL,
                     admin_name TEXT,
                     override_on INTEGER NOT NULL DEFAULT 0,
                     override_fps INTEGER NOT NULL DEFAULT 60,
                     override_mbps REAL NOT NULL DEFAULT 0
                 );
                 INSERT INTO devices VALUES (1, 'client', x'00', zeroblob(32), 0, 5);
                 INSERT INTO clients (device, name) VALUES (1, 'anna');
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let anna = db.client(1).unwrap().unwrap();
        assert_eq!((anna.name.as_str(), anna.version.as_str()), ("anna", ""));
        // Schema 6: files allowed for everyone, 100 Mbit/s, no own switch.
        assert_eq!(anna.files, None);
        assert_eq!(db.files_policy(1).unwrap(), FilePolicy::default());
        db.touch(1, "0.1.0").unwrap();
        assert_eq!(db.client(1).unwrap().unwrap().version, "0.1.0");
        // Opening again changes nothing.
        drop(db);
        assert_eq!(
            Db::open(&path).unwrap().client(1).unwrap().unwrap().version,
            "0.1.0"
        );
    }

    #[test]
    fn versions_are_cleaned() {
        assert_eq!(clean_version(" 0.1.0 "), "0.1.0");
        assert_eq!(clean_version("0.1.0 evil\nline"), "0.1.0");
        assert_eq!(clean_version(&"9".repeat(100)).len(), 32);
    }

    #[test]
    fn access_needs_the_admin() {
        let db = db();
        let (c, _) = db.register(Role::Client, "anna", [1; 32]).unwrap();
        let (h, _) = db.register(Role::Host, "", [2; 32]).unwrap();
        db.update_host(h, "WPC-10", "A.Ivanov", &[7; 32]).unwrap();
        assert_eq!(db.host_by_lookup(&[7; 32]).unwrap().unwrap().id, h);
        assert_eq!(db.host_by_lookup(&[8; 32]).unwrap(), None);

        assert_eq!(db.access(c, h).unwrap(), Access::None);
        assert_eq!(db.request_access(c, h).unwrap(), Access::Pending);
        assert_eq!(db.access_list(Some(Access::Pending)).unwrap().len(), 1);
        db.approve(c, h).unwrap();
        assert_eq!(db.request_access(c, h).unwrap(), Access::Approved);
        assert!(db.unlink(c, h).unwrap());
        assert_eq!(db.access(c, h).unwrap(), Access::None);

        // Rejected stays rejected, even after the client deletes its card.
        db.request_access(c, h).unwrap();
        db.reject(c, h).unwrap();
        assert_eq!(db.request_access(c, h).unwrap(), Access::Rejected);
        db.forget(c, h).unwrap();
        assert_eq!(db.access(c, h).unwrap(), Access::Rejected);
        assert_eq!(db.unlink_all(c).unwrap(), 0);
        // The admin lifts it.
        assert!(db.unlink(c, h).unwrap());
        assert_eq!(db.request_access(c, h).unwrap(), Access::Pending);

        // A second host with the same token is refused.
        let (h2, _) = db.register(Role::Host, "", [3; 32]).unwrap();
        assert!(db.update_host(h2, "WPC-11", "B", &[7; 32]).is_err());
        // Removing the host removes the access.
        db.remove_device(h).unwrap();
        assert!(db.access_list(None).unwrap().is_empty());
    }

    #[test]
    fn the_machine_host_has_no_account() {
        let db = db();
        let (h, _) = db.register(Role::Host, "", [2; 32]).unwrap();
        db.update_host(h, "WPC-10", "", &[7; 32]).unwrap();
        let row = db.host_by_lookup(&[7; 32]).unwrap().unwrap();
        assert_eq!((row.account.as_str(), row.label()), ("", "WPC-10".into()));
        db.update_host(h, "WPC-10", "A.Ivanov", &[7; 32]).unwrap();
        assert_eq!(db.hosts().unwrap()[0].label(), "WPC-10 / A.Ivanov");
        assert!(
            db.update_host(h, "", "", &[7; 32]).is_err(),
            "a machine needs a name"
        );
    }

    #[test]
    fn machine_names_latin_only() {
        let db = db();
        let (h, _) = db.register(Role::Host, "", [2; 32]).unwrap();
        assert!(db.update_host(h, "ДИЗАЙН-01", "", &[7; 32]).is_err());
        db.update_host(h, "DESIGN-01", "", &[7; 32]).unwrap();
        assert_eq!(db.hosts().unwrap()[0].label(), "DESIGN-01");
    }

    #[test]
    fn file_switches() {
        let db = db();
        let (c, _) = db.register(Role::Client, "anna", [1; 32]).unwrap();
        assert_eq!(db.files_policy(c).unwrap(), FilePolicy::default());
        db.set_global_files(false).unwrap();
        db.set_files_mbps(40).unwrap();
        let off = FilePolicy {
            allowed: false,
            mbps: 40,
        };
        assert_eq!(db.files_policy(c).unwrap(), off);
        // Her own switch wins over the global one, both ways.
        db.set_client_files(c, Some(true)).unwrap();
        assert!(db.files_policy(c).unwrap().allowed);
        db.set_global_files(true).unwrap();
        db.set_client_files(c, Some(false)).unwrap();
        assert!(!db.files_policy(c).unwrap().allowed);
        db.set_client_files(c, None).unwrap();
        assert!(db.files_policy(c).unwrap().allowed);
        assert!(db.set_files_mbps(0).is_err());
        assert!(db.set_client_files(c + 100, None).is_err());
    }

    #[test]
    fn limits() {
        let db = db();
        let (c, _) = db.register(Role::Client, "anna", [1; 32]).unwrap();
        let fps = |fps| Limits {
            fps: Some(fps),
            mbps: None,
        };
        let cap = |mbps| Limits {
            fps: None,
            mbps: Some(mbps),
        };
        assert_eq!(db.forced_limits(c).unwrap(), Limits::default());
        // Everyone's frame rate alone, then this client's cap alone: both hold.
        db.set_global_limits(fps(30)).unwrap();
        assert_eq!(db.forced_limits(c).unwrap(), fps(30));
        db.set_client_limits(c, cap(20.0)).unwrap();
        assert_eq!(
            db.forced_limits(c).unwrap(),
            Limits {
                fps: Some(30),
                mbps: Some(20.0)
            }
        );
        // Its own wins field by field.
        db.set_client_limits(c, fps(60)).unwrap();
        assert_eq!(db.forced_limits(c).unwrap(), fps(60));
        db.set_client_limits(c, Limits::default()).unwrap();
        db.set_global_limits(Limits::default()).unwrap();
        assert_eq!(db.forced_limits(c).unwrap(), Limits::default());
        assert!(db.set_global_limits(fps(45)).is_err());
        assert!(db.set_client_limits(c, cap(-1.0)).is_err());
        assert!(db.set_client_limits(999, fps(30)).is_err());
    }

    /// A schema 6 database: the override and the admin's name carry over.
    #[test]
    fn schema_6_moves_to_separate_limits() {
        let path = temp_db_path();
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE devices (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     role TEXT NOT NULL CHECK (role IN ('host', 'client')),
                     secret_hash BLOB NOT NULL,
                     noise_key BLOB NOT NULL,
                     created INTEGER NOT NULL,
                     last_seen INTEGER NOT NULL DEFAULT 0,
                     version TEXT NOT NULL DEFAULT ''
                 );
                 CREATE TABLE clients (
                     device INTEGER PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
                     name TEXT NOT NULL,
                     admin_name TEXT,
                     override_on INTEGER NOT NULL DEFAULT 0,
                     override_fps INTEGER NOT NULL DEFAULT 60,
                     override_mbps REAL NOT NULL DEFAULT 0,
                     files INTEGER
                 );
                 CREATE TABLE global_override (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     override_on INTEGER NOT NULL,
                     override_fps INTEGER NOT NULL,
                     override_mbps REAL NOT NULL
                 );
                 INSERT INTO global_override VALUES (1, 1, 30, 40);
                 INSERT INTO devices (id, role, secret_hash, noise_key, created)
                     VALUES (1, 'client', x'00', zeroblob(32), 0),
                            (2, 'client', x'00', zeroblob(32), 0);
                 INSERT INTO clients (device, name, admin_name, override_on, override_fps, override_mbps, files)
                     VALUES (1, 'anna', 'Анна (дом)', 1, 60, 0, 0),
                            (2, 'petr', NULL, 0, 60, 0, NULL);
                 PRAGMA user_version = 6;",
            )
            .unwrap();
        }
        for _ in 0..2 {
            // Opening again changes nothing.
            let db = Db::open(&path).unwrap();
            let anna = db.client(1).unwrap().unwrap();
            assert_eq!(anna.name, "anna");
            assert_eq!(
                anna.limits,
                Limits {
                    fps: Some(60),
                    mbps: Some(0.0)
                }
            );
            assert_eq!(anna.files, Some(false));
            assert_eq!(anna.admin_name.as_deref(), Some("Анна (дом)"));
            assert_eq!(db.client(2).unwrap().unwrap().admin_name, None);
            assert_eq!(db.client(2).unwrap().unwrap().limits, Limits::default());
            assert_eq!(
                db.global_limits().unwrap(),
                Limits {
                    fps: Some(30),
                    mbps: Some(40.0)
                }
            );
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
