//! Fragmentation + FEC layout for large payloads (video access units, cursor shapes).
//!
//! A payload of `total_len` bytes is cut into equal-size data shards (the last
//! one zero-padded), and shards are grouped into FEC blocks. Each block carries
//! `data_shards` original shards followed by `parity_shards` Reed–Solomon
//! recovery shards; `index` runs over both. Every shard of a payload has the
//! same length, so the receiver learns the shard size from any datagram.
//!
//! Layout (26 bytes, little-endian), directly after the packet header:
//! ```text
//!  0  frame            u32  video frame number / cursor shape id
//!  4  total_len        u32  payload length before padding
//!  8  capture_time_us  u64  host clock at capture
//! 16  block            u16  FEC block index within the payload
//! 18  block_count      u16
//! 20  index            u16  shard index within the block: data first, then parity
//! 22  data_shards      u16  data shards in this block (>= 1)
//! 24  parity_shards    u16  parity shards in this block
//! ```

use crate::DecodeError;
use crate::wire::{Reader, WriteExt};

pub const FRAGMENT_HEADER_LEN: usize = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentHeader {
    pub frame: u32,
    pub total_len: u32,
    pub capture_time_us: u64,
    pub block: u16,
    pub block_count: u16,
    pub index: u16,
    pub data_shards: u16,
    pub parity_shards: u16,
}

impl FragmentHeader {
    pub fn is_parity(&self) -> bool {
        self.index >= self.data_shards
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u32(self.frame);
        out.put_u32(self.total_len);
        out.put_u64(self.capture_time_us);
        out.put_u16(self.block);
        out.put_u16(self.block_count);
        out.put_u16(self.index);
        out.put_u16(self.data_shards);
        out.put_u16(self.parity_shards);
    }

    /// Parses the fragment header from a decrypted body; returns it and the shard bytes.
    pub fn decode(body: &[u8]) -> Result<(FragmentHeader, &[u8]), DecodeError> {
        let mut r = Reader::new(body);
        let h = FragmentHeader {
            frame: r.u32()?,
            total_len: r.u32()?,
            capture_time_us: r.u64()?,
            block: r.u16()?,
            block_count: r.u16()?,
            index: r.u16()?,
            data_shards: r.u16()?,
            parity_shards: r.u16()?,
        };
        if h.block_count == 0 || h.block >= h.block_count {
            return Err(DecodeError::InvalidValue("FragmentHeader.block"));
        }
        if h.data_shards == 0 {
            return Err(DecodeError::InvalidValue("FragmentHeader.data_shards"));
        }
        if u32::from(h.index) >= u32::from(h.data_shards) + u32::from(h.parity_shards) {
            return Err(DecodeError::InvalidValue("FragmentHeader.index"));
        }
        let shard = r.rest();
        if shard.is_empty() {
            return Err(DecodeError::Truncated);
        }
        Ok((h, shard))
    }
}
