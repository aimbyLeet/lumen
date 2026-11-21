//! The update door: how a device learns the
//! server's version and downloads the installer of that version.
//!
//! **Frozen since 0.7.0: never change this format.** Signaling changes
//! freely, and the server refuses a device of another signaling version at
//! login; a device too old to log in still finds its update here.
//!
//! The door shares the signaling connection (the same port and TLS) and its
//! framing (`u32` little-endian body length, then the body, at most
//! [`MAX_FRAME`] bytes); its tags start at [`DOOR_TAGS`], far from
//! signaling's. Flow, before any login:
//! * device → server, the connection's first frame: [`DoorMessage::Query`];
//! * server → device: [`DoorMessage::Offer`] (its installer's version, size
//!   and SHA-256) or [`DoorMessage::NoUpdate`]. A device that wants it — the
//!   version is higher than its own ([`newer`]) — sends
//!   [`DoorMessage::Fetch`] (from `offset`, to resume), else closes;
//! * server → device: [`DoorMessage::Chunk`]s, from `offset` to the end,
//!   then it closes; or [`DoorMessage::Busy`] (too many downloads at once:
//!   try later), then it closes.
//!
//! Strings: `u16` byte length + UTF-8. Integers: little-endian.

use std::io::{self, Read};
use std::path::Path;

use crate::DecodeError;
use crate::signal::MAX_FRAME;
use crate::wire::{Reader, WriteExt};

/// The door's format version a device sends; the server answers in this
/// format whatever it gets.
pub const DOOR_VERSION: u16 = 1;
/// Tags below this one are signaling's.
pub const DOOR_TAGS: u8 = 0xF0;
pub const SHA256_LEN: usize = 32;
/// Installer bytes in one [`DoorMessage::Chunk`].
pub const CHUNK_LEN: usize = 60 * 1024;
/// Longest version string, in bytes.
pub const MAX_VERSION: usize = 64;

mod tag {
    pub const QUERY: u8 = 0xF0;
    pub const OFFER: u8 = 0xF1;
    pub const NO_UPDATE: u8 = 0xF2;
    pub const FETCH: u8 = 0xF3;
    pub const CHUNK: u8 = 0xF4;
    pub const BUSY: u8 = 0xF5;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoorMessage {
    /// Device → server. `app_version`: the device's own (for the server's log).
    Query { door: u16, app_version: String },
    /// Server → device: the installer it has.
    Offer {
        version: String,
        size: u64,
        sha256: [u8; SHA256_LEN],
    },
    /// Server → device: no installer here.
    NoUpdate,
    /// Device → server: send the installer from this byte on.
    Fetch { offset: u64 },
    /// Server → device: the installer's next bytes.
    Chunk(Vec<u8>),
    /// Server → device: too many downloads at once; try later.
    Busy,
}

/// Whether a frame body is the door's, not signaling's.
pub fn is_door(body: &[u8]) -> bool {
    body.first().is_some_and(|&t| t >= DOOR_TAGS)
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(MAX_VERSION)];
    out.put_u16(b.len() as u16);
    out.extend_from_slice(b);
}

fn str(r: &mut Reader<'_>) -> Result<String, DecodeError> {
    let n = usize::from(r.u16()?);
    if n > MAX_VERSION {
        return Err(DecodeError::InvalidValue("door string length"));
    }
    String::from_utf8(r.take(n)?.to_vec()).map_err(|_| DecodeError::InvalidValue("utf-8 string"))
}

impl DoorMessage {
    /// The whole frame: length prefix and body.
    pub fn to_frame(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        match self {
            DoorMessage::Query { door, app_version } => {
                out.put_u8(tag::QUERY);
                out.put_u16(*door);
                put_str(&mut out, app_version);
            }
            DoorMessage::Offer {
                version,
                size,
                sha256,
            } => {
                out.put_u8(tag::OFFER);
                put_str(&mut out, version);
                out.put_u64(*size);
                out.extend_from_slice(sha256);
            }
            DoorMessage::NoUpdate => out.put_u8(tag::NO_UPDATE),
            DoorMessage::Fetch { offset } => {
                out.put_u8(tag::FETCH);
                out.put_u64(*offset);
            }
            DoorMessage::Chunk(data) => {
                out.put_u8(tag::CHUNK);
                out.extend_from_slice(&data[..data.len().min(CHUNK_LEN)]);
            }
            DoorMessage::Busy => out.put_u8(tag::BUSY),
        }
        let len = (out.len() - 4) as u32;
        out[..4].copy_from_slice(&len.to_le_bytes());
        out
    }

    /// Decodes one frame body. A `Query` of a later door version may carry
    /// more after its fields (ignored); anything else must end where it does.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() > MAX_FRAME {
            return Err(DecodeError::InvalidValue("door frame length"));
        }
        let mut r = Reader::new(body);
        let msg = match r.u8()? {
            tag::QUERY => {
                let door = r.u16()?;
                let app_version = str(&mut r)?;
                r.rest();
                DoorMessage::Query { door, app_version }
            }
            tag::OFFER => DoorMessage::Offer {
                version: str(&mut r)?,
                size: r.u64()?,
                sha256: r.array()?,
            },
            tag::NO_UPDATE => DoorMessage::NoUpdate,
            tag::FETCH => DoorMessage::Fetch { offset: r.u64()? },
            tag::CHUNK => {
                let data = r.rest();
                if data.len() > CHUNK_LEN {
                    return Err(DecodeError::InvalidValue("door chunk length"));
                }
                DoorMessage::Chunk(data.to_vec())
            }
            tag::BUSY => DoorMessage::Busy,
            other => return Err(DecodeError::UnknownMessage(other)),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// A version `major.minor.patch` (numbers only; anything else is not a version).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.split('.').map(|p| {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            p.parse::<u32>().ok()
        }
    });
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

/// Whether `candidate` is a higher version than `current` (both must parse).
pub fn newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// A file's size and SHA-256: the installer as [`DoorMessage::Offer`] describes it.
pub fn file_sha256(path: &Path) -> io::Result<(u64, [u8; SHA256_LEN])> {
    let mut f = std::fs::File::open(path)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
        size += n as u64;
    }
    let mut out = [0u8; SHA256_LEN];
    out.copy_from_slice(ctx.finish().as_ref());
    Ok((size, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<DoorMessage> {
        vec![
            DoorMessage::Query {
                door: DOOR_VERSION,
                app_version: "0.7.0".into(),
            },
            DoorMessage::Offer {
                version: "0.7.1".into(),
                size: 12_345_678,
                sha256: [7; SHA256_LEN],
            },
            DoorMessage::NoUpdate,
            DoorMessage::Fetch { offset: 61_440 },
            DoorMessage::Chunk(vec![1, 2, 3]),
            DoorMessage::Busy,
        ]
    }

    #[test]
    fn round_trip() {
        for m in all() {
            let frame = m.to_frame();
            let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
            assert_eq!(len, frame.len() - 4);
            assert!(is_door(&frame[4..]));
            assert_eq!(DoorMessage::decode(&frame[4..]).unwrap(), m);
        }
    }

    /// The format is frozen: these bytes must never change.
    #[test]
    fn frozen_bytes() {
        let hex = |m: DoorMessage| {
            m.to_frame()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let [query, offer, none, fetch, chunk, busy] = all().try_into().unwrap();
        assert_eq!(hex(query), "0a000000f001000500302e372e30");
        assert_eq!(
            hex(offer),
            format!(
                "30000000f10500302e372e314e61bc0000000000{}",
                "07".repeat(32)
            )
        );
        assert_eq!(hex(none), "01000000f2");
        assert_eq!(hex(fetch), "09000000f300f0000000000000");
        assert_eq!(hex(chunk), "04000000f4010203");
        assert_eq!(hex(busy), "01000000f5");
    }

    #[test]
    fn later_door_query_still_reads() {
        let mut frame = DoorMessage::Query {
            door: 9,
            app_version: "1.0.0".into(),
        }
        .to_frame();
        frame.extend_from_slice(b"more");
        assert_eq!(
            DoorMessage::decode(&frame[4..]).unwrap(),
            DoorMessage::Query {
                door: 9,
                app_version: "1.0.0".into()
            }
        );
    }

    #[test]
    fn malformed_is_an_error() {
        assert!(DoorMessage::decode(&[]).is_err());
        assert!(DoorMessage::decode(&[0xF9]).is_err());
        assert!(DoorMessage::decode(&[tag::FETCH, 1, 2]).is_err());
        assert!(DoorMessage::decode(&[tag::NO_UPDATE, 0]).is_err());
        let mut big = vec![tag::CHUNK];
        big.resize(CHUNK_LEN + 2, 0);
        assert!(DoorMessage::decode(&big).is_err());
        assert!(!is_door(&[3, 0]));
        assert!(!is_door(&[]));
    }

    #[test]
    fn versions() {
        assert!(newer("0.7.1", "0.7.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.10", "0.7.0"));
        assert!(!newer("0.7", "0.6.0"));
        assert!(!newer("0.7.1-test", "0.7.0"));
        assert!(!newer("0.7.1", ""));
        assert_eq!(parse_version("0.6.10"), Some((0, 6, 10)));
        assert_eq!(parse_version("0.6.+1"), None);
        assert_eq!(parse_version("0.6.1.2"), None);
    }
}
