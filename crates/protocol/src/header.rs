//! Common header that starts every datagram.
//!
//! Layout (20 bytes, little-endian):
//! ```text
//!  0  magic     [u8; 2]  b"LM"
//!  2  version   u8       PROTOCOL_VERSION
//!  3  kind      u8       PacketKind
//!  4  flags     u8       Flags
//!  5  stream    u8       video stream (one per monitor of the host, < MAX_STREAMS); 0 otherwise
//!  6  reserved  [u8; 2]  written as 0, ignored on read
//!  8  session   u32      drops stale packets after a reconnect
//! 12  seq       u64      per-sender packet counter, never reused within a session
//! ```
//! When [`Flags::ENCRYPTED`] is set the body is ciphertext followed by an
//! [`AUTH_TAG_LEN`](crate::AUTH_TAG_LEN)-byte tag; the header itself stays in the
//! clear and is authenticated as associated data, with `seq` as the nonce.
//!
//! The magic's first byte (0x4C) has its top two bits set to `01`, so these
//! packets never collide with STUN messages (top bits `00`) on a shared socket.

use crate::DecodeError;
use crate::wire::{Reader, WriteExt, wire_enum};

pub const MAGIC: [u8; 2] = *b"LM";
/// v3: `Pong` carries the host's clock. v4: `InputEvent::SecureAttention`
/// (an older host would never acknowledge it, and input would stall).
/// v5: clipboard text (`InputEvent::Clipboard`, `ControlMessage::Clipboard`).
/// v6: `ControlMessage::HostStats` (the client's overlay).
/// v7: `PacketKind::Audio` (the host's sound).
/// v8: `PacketKind::File` (file transfers).
/// v9: `FileCancel::Locked`, `FileMsg::Refused` (the host's lock screen).
/// v10: several monitors: `stream` in the header, `Monitors` / `ShowMonitors`,
/// the stream in `StreamStart`, `RequestKeyframe` and `RequestRecovery`.
/// v11: `ControlMessage::Inputs` (input events with earlier unacknowledged ones).
/// v12: `keep_unlocked` in `Hello` (the host does not lock Windows after the session).
/// v13: a host without monitors: `MonitorWanted` / `MonitorChoice`, `NoMonitors`.
/// v14: `Purpose::Download { batch }` (a folder's files are one row).
pub const PROTOCOL_VERSION: u8 = 14;
/// Video streams a session can carry (monitors of the host).
pub const MAX_STREAMS: u8 = 4;
pub const HEADER_LEN: usize = 20;

wire_enum! {
    /// What the body of a datagram carries.
    pub enum PacketKind {
        /// One shard of a video access unit: [`FragmentHeader`](crate::FragmentHeader) + bytes.
        Video = 1,
        /// One shard of a cursor shape: [`FragmentHeader`](crate::FragmentHeader) + bytes
        /// of an encoded [`CursorShape`](crate::CursorShape).
        CursorShape = 2,
        /// A single [`ControlMessage`](crate::ControlMessage).
        Control = 3,
        /// A Noise handshake message (`transport::secure`); never encrypted.
        Handshake = 4,
        /// Opus frames of the host's sound: [`AudioPacket`](crate::AudioPacket).
        Audio = 5,
        /// File transfer data or its ack: [`FilePacket`](crate::FilePacket).
        File = 6,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(pub u8);

impl Flags {
    /// The fragment belongs to an IDR access unit.
    pub const KEYFRAME: Flags = Flags(1 << 0);
    /// Body is encrypted and ends with an auth tag.
    pub const ENCRYPTED: Flags = Flags(1 << 1);
    /// The receiver must answer with `ControlMessage::Ack { seq }`.
    pub const ACK_REQUESTED: Flags = Flags(1 << 2);
    /// The fragment belongs to the first frame after a `RequestRecovery`: it
    /// references only frames up to the client's `last_good`.
    pub const RECOVERY: Flags = Flags(1 << 3);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn set(&mut self, other: Flags, on: bool) {
        if on {
            self.0 |= other.0;
        } else {
            self.0 &= !other.0;
        }
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub kind: PacketKind,
    pub flags: Flags,
    /// Video stream of a `Video` fragment; 0 for every other kind.
    pub stream: u8,
    pub session: u32,
    pub seq: u64,
}

/// A handshake datagram: header and the Noise message, in the clear (Noise
/// protects it itself).
pub fn handshake_datagram(session: u32, message: &[u8]) -> Vec<u8> {
    let mut d = Vec::with_capacity(message.len() + 32);
    PacketHeader {
        kind: PacketKind::Handshake,
        flags: Flags::empty(),
        stream: 0,
        session,
        seq: 0,
    }
    .encode(&mut d);
    d.extend_from_slice(message);
    d
}

impl PacketHeader {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&MAGIC);
        out.put_u8(PROTOCOL_VERSION);
        out.put_u8(self.kind as u8);
        out.put_u8(self.flags.0);
        out.put_u8(self.stream);
        out.extend_from_slice(&[0; 2]);
        out.put_u32(self.session);
        out.put_u64(self.seq);
    }

    /// Splits a datagram into its header and the (possibly encrypted) body.
    pub fn decode(datagram: &[u8]) -> Result<(PacketHeader, &[u8]), DecodeError> {
        let mut r = Reader::new(datagram);
        if r.array::<2>()? != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let version = r.u8()?;
        if version != PROTOCOL_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let kind = r.u8()?;
        let kind = PacketKind::try_from(kind).map_err(|_| DecodeError::UnknownKind(kind))?;
        let flags = Flags(r.u8()?);
        let stream = r.u8()?;
        if stream >= MAX_STREAMS {
            return Err(DecodeError::InvalidValue("PacketHeader.stream"));
        }
        r.take(2)?;
        let session = r.u32()?;
        let seq = r.u64()?;
        let header = PacketHeader {
            kind,
            flags,
            stream,
            session,
            seq,
        };
        Ok((header, r.rest()))
    }
}
