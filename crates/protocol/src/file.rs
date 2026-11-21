//! The body of a `File` datagram (protocol v8): bulk data of file transfers
//! and its acknowledgements, either way.
//!
//! The data goes in chunks, each numbered once when first sent (`chunk`; a
//! resent chunk keeps its number) and acknowledged selectively by number. A
//! chunk says which transfer it belongs to and where in it (`offset`), so the
//! receiver can write it at once, in any order.
//!
//! Layout (little-endian), after a type byte (0: data, 1: ack):
//! ```text
//! data:
//!  0  chunk     u64  number of this chunk
//!  8  floor     u64  every chunk below it is done with (received or no
//!                    longer wanted, e.g. its transfer was cancelled)
//! 16  transfer  u32
//! 20  offset    u64  of the data in the transfer
//! 28  sent_us   u32  sender's clock, µs (wraps), echoed back for the RTT
//! 32  data      ..   the rest
//! ack:
//!  0  below     u64  every chunk below it has arrived
//!  8  echo_us   u32  `sent_us` of the newest chunk that arrived
//! 12  held_us   u32  how long ago that chunk arrived
//! 16  bytes     u64  data bytes that arrived so far (each chunk once)
//! 24  count     u8   then `count` ranges of chunks above `below` that
//!                    arrived: start u64, end u64 (exclusive), ascending
//! 25  gap       u8   0, or: the ranges are not all of them — between range
//!                    `gap - 1` and range `gap` there are more, not listed
//!                    (the lowest ones and the newest ones are)
//! ```

use crate::DecodeError;
use crate::wire::{Reader, WriteExt, wire_enum};

/// The most ranges an ack carries (the lowest and the newest ones).
pub const MAX_ACK_RANGES: usize = 64;
/// Bytes of a data chunk's header, with its type byte.
pub const FILE_DATA_HEADER_LEN: usize = 1 + 32;
/// Bytes of data in a chunk that fills a datagram of [`DEFAULT_MAX_DATAGRAM`](crate::DEFAULT_MAX_DATAGRAM).
pub const FILE_CHUNK_LEN: usize =
    crate::DEFAULT_MAX_DATAGRAM - crate::HEADER_LEN - crate::AUTH_TAG_LEN - FILE_DATA_HEADER_LEN;

#[derive(Clone, PartialEq, Eq)]
pub struct FileData {
    pub chunk: u64,
    pub floor: u64,
    pub transfer: u32,
    pub offset: u64,
    pub sent_us: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for FileData {
    // The bytes are the user's file: logs get their length only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "FileData {{ chunk: {}, floor: {}, transfer: {}, offset: {}, sent_us: {}, len: {} }}",
            self.chunk,
            self.floor,
            self.transfer,
            self.offset,
            self.sent_us,
            self.data.len()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAck {
    pub below: u64,
    pub echo_us: u32,
    pub held_us: u32,
    pub bytes: u64,
    /// `(start, end)`, end exclusive; all above `below`, ascending.
    pub ranges: Vec<(u64, u64)>,
    /// 0, or: chunks between `ranges[gap - 1]` and `ranges[gap]` are not
    /// told about (more ranges than fit).
    pub gap: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePacket {
    Data(FileData),
    Ack(FileAck),
}

mod tag {
    pub const DATA: u8 = 0;
    pub const ACK: u8 = 1;
}

impl FilePacket {
    /// A whole datagram: header + body (to be sealed).
    pub fn datagram(&self, session: u32, seq: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(crate::DEFAULT_MAX_DATAGRAM);
        crate::PacketHeader {
            kind: crate::PacketKind::File,
            flags: crate::Flags::empty(),
            stream: 0,
            session,
            seq,
        }
        .encode(&mut d);
        self.encode(&mut d);
        d
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            FilePacket::Data(d) => {
                out.put_u8(tag::DATA);
                out.put_u64(d.chunk);
                out.put_u64(d.floor);
                out.put_u32(d.transfer);
                out.put_u64(d.offset);
                out.put_u32(d.sent_us);
                out.extend_from_slice(&d.data);
            }
            FilePacket::Ack(a) => {
                out.put_u8(tag::ACK);
                out.put_u64(a.below);
                out.put_u32(a.echo_us);
                out.put_u32(a.held_us);
                out.put_u64(a.bytes);
                let n = a.ranges.len().min(MAX_ACK_RANGES);
                out.put_u8(n as u8);
                out.put_u8(a.gap);
                for &(s, e) in &a.ranges[..n] {
                    out.put_u64(s);
                    out.put_u64(e);
                }
            }
        }
    }

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        match r.u8()? {
            tag::DATA => {
                let chunk = r.u64()?;
                let floor = r.u64()?;
                let transfer = r.u32()?;
                let offset = r.u64()?;
                let sent_us = r.u32()?;
                let data = r.rest().to_vec();
                if data.is_empty() {
                    return Err(DecodeError::InvalidValue("empty file chunk"));
                }
                if offset.checked_add(data.len() as u64).is_none() {
                    return Err(DecodeError::InvalidValue("file chunk offset"));
                }
                Ok(FilePacket::Data(FileData {
                    chunk,
                    floor,
                    transfer,
                    offset,
                    sent_us,
                    data,
                }))
            }
            tag::ACK => {
                let below = r.u64()?;
                let echo_us = r.u32()?;
                let held_us = r.u32()?;
                let bytes = r.u64()?;
                let n = usize::from(r.u8()?);
                let gap = r.u8()?;
                if n > MAX_ACK_RANGES || usize::from(gap) >= n.max(1) {
                    return Err(DecodeError::InvalidValue("ack ranges"));
                }
                let mut ranges = Vec::with_capacity(n);
                let mut last = below;
                for _ in 0..n {
                    let (s, e) = (r.u64()?, r.u64()?);
                    if s >= e || s <= last {
                        return Err(DecodeError::InvalidValue("ack range"));
                    }
                    last = e;
                    ranges.push((s, e));
                }
                r.finish()?;
                Ok(FilePacket::Ack(FileAck {
                    below,
                    echo_us,
                    held_us,
                    bytes,
                    ranges,
                    gap,
                }))
            }
            t => Err(DecodeError::UnknownMessage(t)),
        }
    }
}

/// The longest name of a file in a transfer (UTF-8 bytes): its path inside
/// what was copied, `/` between folders. A message must fit in a datagram.
pub const MAX_FILE_NAME: usize = 900;

wire_enum! {
    /// Why a transfer stopped short.
    pub enum FileCancel {
        /// The user cancelled it (on either side), or the session ended.
        User = 0,
        /// Not enough room on the receiving disk.
        Disk = 1,
        /// The file could not be read.
        Read = 2,
        /// It could not be written.
        Write = 3,
        /// What arrived differs from what was sent (hash).
        Hash = 4,
        /// File transfers are off for this client (the admin).
        NotAllowed = 5,
        /// Nobody is signed in to Windows on the host.
        NoUser = 6,
        /// Windows on the host is locked: files wait for a sign-in.
        Locked = 7,
    }
}

/// Where a transferred file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Into `Downloads\Lumen` (what was copied there, more than fits the
    /// clipboard, after a paste here). `batch`: the files that went at once
    /// (one paste), numbered by the sender — a folder of them is one row.
    Download { batch: u32 },
    /// Part of clipboard set `set` (files copied there, small enough): onto
    /// the clipboard here once the whole set came.
    Clip { set: u32 },
    /// Clipboard format `format` of the picture copied there as set `set`.
    Image { set: u32, format: u8 },
}

impl Purpose {
    /// The clipboard set it belongs to.
    pub fn clip_set(&self) -> Option<u32> {
        match *self {
            Purpose::Download { .. } => None,
            Purpose::Clip { set } | Purpose::Image { set, .. } => Some(set),
        }
    }
}

/// What the two sides tell each other about transfers, reliable and in order
/// (client → host in the input stream, host → client as
/// `ControlMessage::File`). Each side numbers the transfers it sends; the
/// numbers live as long as its program (a session that reconnects goes on
/// with them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileMsg {
    /// Sender: transfer `transfer` is `len` bytes of the file `name`. Sent
    /// again on each new session until the file is saved.
    Offer {
        transfer: u32,
        len: u64,
        name: String,
        purpose: Purpose,
    },
    /// Receiver: send it from byte `from` on (what it has already, from the
    /// start); `len`: it has it all.
    Accept { transfer: u32, from: u64 },
    /// Sender: every byte acknowledged; the file's SHA-256.
    Done { transfer: u32, sha256: [u8; 32] },
    /// Receiver: written and checked, in its place.
    Saved { transfer: u32 },
    /// Either: the transfer is over. `mine`: the one who says it is the
    /// transfer's sender.
    Cancel {
        transfer: u32,
        mine: bool,
        reason: FileCancel,
    },
    /// Files were copied on this side (clipboard set `set`, numbered by the
    /// side that copied): `files` files of `bytes` in all, `name` the first
    /// thing copied. Not `big`: they come at once (`Purpose::Clip`); `big`:
    /// only on a paste there (`Fetch`).
    Copied {
        set: u32,
        files: u32,
        bytes: u64,
        big: bool,
        name: String,
    },
    /// A picture was copied on this side: `formats` of it come as
    /// `Purpose::Image`.
    CopiedImage { set: u32, formats: u8 },
    /// The other side pasted while set `set` (big) was what it had: send it.
    Fetch { set: u32 },
    /// Set `set` is on the clipboard of the side that says it.
    Placed { set: u32 },
    /// A paste of big set `set` did not start (`reason`): a row for it where
    /// the overlay is. `yours`: the set is the receiver's of this message.
    Refused {
        set: u32,
        yours: bool,
        reason: FileCancel,
    },
}

mod msg_tag {
    pub const OFFER: u8 = 1;
    pub const ACCEPT: u8 = 2;
    pub const DONE: u8 = 3;
    pub const SAVED: u8 = 4;
    pub const CANCEL: u8 = 5;
    pub const COPIED: u8 = 6;
    pub const COPIED_IMAGE: u8 = 7;
    pub const FETCH: u8 = 8;
    pub const PLACED: u8 = 9;
    pub const REFUSED: u8 = 10;
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    let mut n = name.len().min(MAX_FILE_NAME);
    while !name.is_char_boundary(n) {
        n -= 1;
    }
    out.put_u16(n as u16);
    out.extend_from_slice(&name.as_bytes()[..n]);
}

fn name(r: &mut Reader) -> Result<String, DecodeError> {
    let n = usize::from(r.u16()?);
    if n > MAX_FILE_NAME {
        return Err(DecodeError::InvalidValue("file name"));
    }
    String::from_utf8(r.take(n)?.to_vec()).map_err(|_| DecodeError::InvalidValue("file name"))
}

impl Purpose {
    fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Purpose::Download { batch } => {
                out.put_u8(0);
                out.put_u32(batch);
            }
            Purpose::Clip { set } => {
                out.put_u8(1);
                out.put_u32(set);
            }
            Purpose::Image { set, format } => {
                out.put_u8(2);
                out.put_u32(set);
                out.put_u8(format);
            }
        }
    }

    fn decode(r: &mut Reader) -> Result<Self, DecodeError> {
        Ok(match r.u8()? {
            0 => Purpose::Download { batch: r.u32()? },
            1 => Purpose::Clip { set: r.u32()? },
            2 => Purpose::Image {
                set: r.u32()?,
                format: r.u8()?,
            },
            _ => return Err(DecodeError::InvalidValue("Purpose")),
        })
    }
}

impl FileMsg {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        match self {
            FileMsg::Offer {
                transfer,
                len,
                name,
                purpose,
            } => {
                out.put_u8(msg_tag::OFFER);
                out.put_u32(*transfer);
                out.put_u64(*len);
                put_name(out, name);
                purpose.encode(out);
            }
            FileMsg::Accept { transfer, from } => {
                out.put_u8(msg_tag::ACCEPT);
                out.put_u32(*transfer);
                out.put_u64(*from);
            }
            FileMsg::Done { transfer, sha256 } => {
                out.put_u8(msg_tag::DONE);
                out.put_u32(*transfer);
                out.extend_from_slice(sha256);
            }
            FileMsg::Saved { transfer } => {
                out.put_u8(msg_tag::SAVED);
                out.put_u32(*transfer);
            }
            FileMsg::Cancel {
                transfer,
                mine,
                reason,
            } => {
                out.put_u8(msg_tag::CANCEL);
                out.put_u32(*transfer);
                out.put_bool(*mine);
                out.put_u8(*reason as u8);
            }
            FileMsg::Copied {
                set,
                files,
                bytes,
                big,
                name,
            } => {
                out.put_u8(msg_tag::COPIED);
                out.put_u32(*set);
                out.put_u32(*files);
                out.put_u64(*bytes);
                out.put_bool(*big);
                put_name(out, name);
            }
            FileMsg::CopiedImage { set, formats } => {
                out.put_u8(msg_tag::COPIED_IMAGE);
                out.put_u32(*set);
                out.put_u8(*formats);
            }
            FileMsg::Fetch { set } => {
                out.put_u8(msg_tag::FETCH);
                out.put_u32(*set);
            }
            FileMsg::Placed { set } => {
                out.put_u8(msg_tag::PLACED);
                out.put_u32(*set);
            }
            FileMsg::Refused { set, yours, reason } => {
                out.put_u8(msg_tag::REFUSED);
                out.put_u32(*set);
                out.put_bool(*yours);
                out.put_u8(*reason as u8);
            }
        }
    }

    pub(crate) fn decode(r: &mut Reader) -> Result<Self, DecodeError> {
        Ok(match r.u8()? {
            msg_tag::OFFER => FileMsg::Offer {
                transfer: r.u32()?,
                len: r.u64()?,
                name: name(r)?,
                purpose: Purpose::decode(r)?,
            },
            msg_tag::ACCEPT => FileMsg::Accept {
                transfer: r.u32()?,
                from: r.u64()?,
            },
            msg_tag::DONE => FileMsg::Done {
                transfer: r.u32()?,
                sha256: r.array()?,
            },
            msg_tag::SAVED => FileMsg::Saved { transfer: r.u32()? },
            msg_tag::CANCEL => FileMsg::Cancel {
                transfer: r.u32()?,
                mine: r.bool()?,
                reason: FileCancel::try_from(r.u8()?)?,
            },
            msg_tag::COPIED => FileMsg::Copied {
                set: r.u32()?,
                files: r.u32()?,
                bytes: r.u64()?,
                big: r.bool()?,
                name: name(r)?,
            },
            msg_tag::COPIED_IMAGE => FileMsg::CopiedImage {
                set: r.u32()?,
                formats: r.u8()?,
            },
            msg_tag::FETCH => FileMsg::Fetch { set: r.u32()? },
            msg_tag::PLACED => FileMsg::Placed { set: r.u32()? },
            msg_tag::REFUSED => FileMsg::Refused {
                set: r.u32()?,
                yours: r.bool()?,
                reason: FileCancel::try_from(r.u8()?)?,
            },
            _ => return Err(DecodeError::InvalidValue("FileMsg")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fits(msg: &crate::ControlMessage) -> bool {
        let mut body = Vec::new();
        msg.encode(&mut body);
        body.len() + crate::HEADER_LEN + crate::AUTH_TAG_LEN <= crate::DEFAULT_MAX_DATAGRAM
    }

    #[test]
    fn file_messages_roundtrip() {
        for m in [
            FileMsg::Offer {
                transfer: 7,
                len: 5 << 30,
                name: "папка/кадр 001.exr".into(),
                purpose: Purpose::Download { batch: 7 },
            },
            FileMsg::Offer {
                transfer: 8,
                len: 1,
                name: "a".into(),
                purpose: Purpose::Clip { set: 3 },
            },
            FileMsg::Offer {
                transfer: 9,
                len: 1,
                name: "PNG".into(),
                purpose: Purpose::Image { set: 4, format: 1 },
            },
            FileMsg::Copied {
                set: 3,
                files: 12,
                bytes: 40 << 20,
                big: true,
                name: "кадры".into(),
            },
            FileMsg::CopiedImage { set: 4, formats: 2 },
            FileMsg::Fetch { set: 3 },
            FileMsg::Placed { set: 4 },
            FileMsg::Refused {
                set: 3,
                yours: true,
                reason: FileCancel::Locked,
            },
            FileMsg::Accept {
                transfer: 7,
                from: 1131 * 9,
            },
            FileMsg::Done {
                transfer: 7,
                sha256: [3; 32],
            },
            FileMsg::Saved { transfer: 7 },
            FileMsg::Cancel {
                transfer: 7,
                mine: true,
                reason: FileCancel::Disk,
            },
        ] {
            for msg in [
                crate::ControlMessage::File {
                    id: 4,
                    msg: m.clone(),
                },
                crate::ControlMessage::Input {
                    id: 5,
                    event: crate::InputEvent::File(m.clone()),
                },
            ] {
                assert!(fits(&msg));
                let mut body = Vec::new();
                msg.encode(&mut body);
                assert_eq!(crate::ControlMessage::decode(&body).unwrap(), msg);
            }
        }
        // The longest name still fits; a longer one is cut on a char boundary.
        let msg = crate::ControlMessage::Input {
            id: 1,
            event: crate::InputEvent::File(FileMsg::Offer {
                transfer: 1,
                len: 1,
                name: "я".repeat(MAX_FILE_NAME),
                purpose: Purpose::Image { set: 1, format: 1 },
            }),
        };
        assert!(fits(&msg));
        let mut body = Vec::new();
        msg.encode(&mut body);
        assert!(crate::ControlMessage::decode(&body).is_ok());
    }

    #[test]
    fn data_fills_a_datagram() {
        let p = FilePacket::Data(FileData {
            chunk: 1 << 40,
            floor: 7,
            transfer: 3,
            offset: 5 << 30,
            sent_us: 0xDEAD_BEEF,
            data: vec![0xA5; FILE_CHUNK_LEN],
        });
        let d = p.datagram(9, 42);
        assert_eq!(
            d.len() + crate::AUTH_TAG_LEN,
            crate::DEFAULT_MAX_DATAGRAM,
            "a full chunk, sealed, is one datagram"
        );
        let (h, body) = crate::PacketHeader::decode(&d).unwrap();
        assert_eq!(h.kind, crate::PacketKind::File);
        assert_eq!(FilePacket::decode(body).unwrap(), p);
    }

    #[test]
    fn ack_roundtrip() {
        let p = FilePacket::Ack(FileAck {
            below: 100,
            echo_us: 5,
            held_us: 6,
            bytes: 1 << 33,
            ranges: vec![(200, 201), (300, 310)],
            gap: 1,
        });
        let mut body = Vec::new();
        p.encode(&mut body);
        assert_eq!(FilePacket::decode(&body).unwrap(), p);
    }

    #[test]
    fn rejects_garbage() {
        assert!(FilePacket::decode(&[]).is_err());
        assert!(FilePacket::decode(&[7]).is_err());
        // A data chunk without data.
        let mut body = Vec::new();
        FilePacket::Data(FileData {
            chunk: 1,
            floor: 0,
            transfer: 1,
            offset: 0,
            sent_us: 0,
            data: vec![1],
        })
        .encode(&mut body);
        assert!(FilePacket::decode(&body[..body.len() - 1]).is_err());
        // A range at or below `below`, an empty one, one out of order, a gap
        // past the ranges.
        for (ranges, gap) in [
            (vec![(100, 105)], 0),
            (vec![(200, 200)], 0),
            (vec![(300, 310), (200, 201)], 0),
            (vec![(200, 201)], 1),
        ] {
            let mut body = Vec::new();
            FilePacket::Ack(FileAck {
                below: 100,
                echo_us: 0,
                held_us: 0,
                bytes: 0,
                ranges,
                gap,
            })
            .encode(&mut body);
            assert!(FilePacket::decode(&body).is_err());
        }
        let mut body = Vec::new();
        FilePacket::Ack(FileAck {
            below: 1,
            echo_us: 0,
            held_us: 0,
            bytes: 0,
            ranges: vec![(5, 6)],
            gap: 0,
        })
        .encode(&mut body);
        assert!(FilePacket::decode(&body[..body.len() - 1]).is_err());
    }
}
