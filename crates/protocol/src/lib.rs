//! Lumen wire format shared by host and client.
//!
//! Every UDP datagram is a [`PacketHeader`] followed by a body whose meaning
//! depends on [`PacketKind`]:
//! - `Video`, `CursorShape`: [`FragmentHeader`] + one shard of a larger payload;
//! - `Control`: one [`ControlMessage`];
//! - `Audio`: one [`AudioPacket`];
//! - `File`: one [`FilePacket`] (file transfer data or its ack).
//!
//! Encryption is applied by the transport to the body only (see `header.rs`);
//! this crate parses decrypted bodies. Decoders never panic on malformed input.

mod audio;
pub mod clipboard;
mod control;
mod cursor;
mod file;
mod format;
mod fragment;
mod header;
pub mod signal;
pub mod update;
mod wire;

pub use audio::{AudioPacket, MAX_OPUS_PACKET};
pub use control::{
    ClientStats, ControlMessage, HostStats, InputEvent, MAX_INPUTS, MAX_MONITORS, Monitor,
    MouseButton,
};
pub use cursor::{CursorKind, CursorShape, MAX_CURSOR_DIM};
pub use file::{
    FILE_CHUNK_LEN, FILE_DATA_HEADER_LEN, FileAck, FileCancel, FileData, FileMsg, FilePacket,
    MAX_ACK_RANGES, MAX_FILE_NAME, Purpose,
};
pub use format::{CHROMA_LOC_LEFT, Capabilities, ChromaFormat, Codec, H273_BT709, VideoFormat};
pub use fragment::{FRAGMENT_HEADER_LEN, FragmentHeader};
pub use header::{
    Flags, HEADER_LEN, MAGIC, MAX_STREAMS, PROTOCOL_VERSION, PacketHeader, PacketKind,
    handshake_datagram,
};

/// Length of the authentication tag appended to encrypted bodies.
pub const AUTH_TAG_LEN: usize = 16;

/// Default upper bound for a whole UDP payload; safe for IPv6 and most tunnels.
pub const DEFAULT_MAX_DATAGRAM: usize = 1200;

/// Shard size that fills a datagram of `max_datagram` bytes, including room for
/// the auth tag. Rounded down to even, as Reed–Solomon (SIMD) requires.
pub const fn max_shard_len(max_datagram: usize) -> usize {
    max_datagram.saturating_sub(HEADER_LEN + FRAGMENT_HEADER_LEN + AUTH_TAG_LEN) & !1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u8),
    UnknownKind(u8),
    UnknownMessage(u8),
    InvalidValue(&'static str),
    TrailingBytes,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "packet truncated"),
            DecodeError::BadMagic => write!(f, "not a Lumen packet"),
            DecodeError::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            DecodeError::UnknownKind(k) => write!(f, "unknown packet kind {k}"),
            DecodeError::UnknownMessage(t) => write!(f, "unknown control message {t}"),
            DecodeError::InvalidValue(what) => write!(f, "invalid value in {what}"),
            DecodeError::TrailingBytes => write!(f, "trailing bytes after message"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests;
