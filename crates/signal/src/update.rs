//! The device's side of the update door (`protocol::update`): what the server offers, and its installer
//! downloaded and checked. No login: a device of any signaling version gets it.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::update::{DOOR_VERSION, DoorMessage, SHA256_LEN};
pub use protocol::update::{file_sha256 as sha256, newer, parse_version};

use crate::{APP_VERSION, SignalClient, Trust};

/// The installer the server has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    pub size: u64,
    pub sha256: [u8; SHA256_LEN],
}

impl Offer {
    /// Newer than this program: worth installing.
    pub fn is_newer(&self) -> bool {
        newer(&self.version, APP_VERSION)
    }

    /// The installer's file name.
    pub fn file_name(&self) -> String {
        format!("lumen-setup-{}.exe", self.version)
    }
}

/// Why there is no installer to run. [`UpdateError::code`] is for the interface.
#[derive(Debug)]
pub enum UpdateError {
    /// No connection, cut off, or nonsense from the server.
    Network(io::Error),
    /// Too many downloads at once: later.
    Busy,
    /// The bytes do not match the offer's SHA-256 (the file is deleted).
    Sha,
    /// Could not write the file.
    Disk(io::Error),
}

impl UpdateError {
    pub fn code(&self) -> &'static str {
        match self {
            UpdateError::Network(_) => "err_update_download",
            UpdateError::Busy => "err_update_busy",
            UpdateError::Sha => "err_update_sha",
            UpdateError::Disk(_) => "err_update_disk",
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "download: {e}"),
            UpdateError::Busy => write!(f, "the server is busy with other downloads"),
            UpdateError::Sha => write!(f, "the installer's SHA-256 does not match"),
            UpdateError::Disk(e) => write!(f, "write the installer: {e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

fn bad_data(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// Opens the door: the connection and the server's answer (None: no installer).
fn open(
    server: &str,
    trust: &Trust,
    timeout: Duration,
) -> io::Result<(SignalClient, Option<Offer>)> {
    let mut c = SignalClient::connect(server, trust, timeout)?;
    c.send_frame(
        &DoorMessage::Query {
            door: DOOR_VERSION,
            app_version: APP_VERSION.into(),
        }
        .to_frame(),
    )?;
    let body = c.recv_frame(timeout)?.ok_or(io::ErrorKind::TimedOut)?;
    let offer = match DoorMessage::decode(&body).map_err(|_| bad_data("update door: bad reply"))? {
        DoorMessage::Offer {
            version,
            size,
            sha256,
        } => Some(Offer {
            version,
            size,
            sha256,
        }),
        DoorMessage::NoUpdate => None,
        _ => return Err(bad_data("update door: unexpected reply")),
    };
    Ok((c, offer))
}

/// What the server offers (None: no installer there). Its version may be
/// lower than ours, or the same: [`Offer::is_newer`].
pub fn query(server: &str, trust: &Trust, timeout: Duration) -> io::Result<Option<Offer>> {
    let (c, offer) = open(server, trust, timeout)?;
    c.close();
    Ok(offer)
}

/// Downloads `offer`'s installer into `dir` and checks its SHA-256; returns
/// its path. A partial file left by an earlier try is resumed. `timeout`:
/// for connecting and for each wait on the server.
pub fn download(
    server: &str,
    trust: &Trust,
    offer: &Offer,
    dir: &Path,
    timeout: Duration,
) -> Result<PathBuf, UpdateError> {
    fs::create_dir_all(dir).map_err(UpdateError::Disk)?;
    let done = dir.join(offer.file_name());
    if done.exists() && sha256(&done).map_err(UpdateError::Disk)? == (offer.size, offer.sha256) {
        return Ok(done);
    }
    let part = dir.join(format!("{}.part", offer.file_name()));
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > offer.size {
        fs::remove_file(&part).map_err(UpdateError::Disk)?;
        have = 0;
    }
    if have < offer.size {
        let (mut c, now) = open(server, trust, timeout).map_err(UpdateError::Network)?;
        // The server changed its installer meanwhile: that is not this one.
        if now.as_ref() != Some(offer) {
            return Err(UpdateError::Network(bad_data(
                "update door: the offer changed",
            )));
        }
        c.send_frame(&DoorMessage::Fetch { offset: have }.to_frame())
            .map_err(UpdateError::Network)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .map_err(UpdateError::Disk)?;
        while have < offer.size {
            let body = c
                .recv_frame(timeout)
                .map_err(UpdateError::Network)?
                .ok_or(UpdateError::Network(io::ErrorKind::TimedOut.into()))?;
            match DoorMessage::decode(&body) {
                Ok(DoorMessage::Chunk(data)) if have + data.len() as u64 <= offer.size => {
                    file.write_all(&data).map_err(UpdateError::Disk)?;
                    have += data.len() as u64;
                }
                Ok(DoorMessage::Busy) => return Err(UpdateError::Busy),
                _ => return Err(UpdateError::Network(bad_data("update door: bad chunk"))),
            }
        }
        file.sync_all().map_err(UpdateError::Disk)?;
        drop(file);
        c.close();
    }
    if sha256(&part).map_err(UpdateError::Disk)? != (offer.size, offer.sha256) {
        let _ = fs::remove_file(&part);
        return Err(UpdateError::Sha);
    }
    fs::rename(&part, &done).map_err(UpdateError::Disk)?;
    Ok(done)
}
