//! The body of an `Audio` datagram: Opus frames of the host's sound.
//!
//! Each datagram carries the newest 10 ms frame and the one before it, so a
//! single lost datagram costs nothing (the next one has its frame).
//!
//! Layout (little-endian):
//! ```text
//!  0  frame       u64  index of the newest frame (10 ms each, from the start of
//!                      the stream; a jump means the host had nothing to send)
//!  8  capture_us  u64  host wall clock of the newest frame's first sample: when
//!                      it sounds from the host's own device (loopback), or
//!                      when it was made (a test tone)
//! 16  len         u16  then the newest frame's Opus packet
//!     prev_len    u16  then the previous frame's (0: none)
//! ```

use crate::DecodeError;
use crate::wire::{Reader, WriteExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub frame: u64,
    pub capture_us: u64,
    pub opus: Vec<u8>,
    /// Frame `frame - 1`, if the host has it.
    pub previous: Option<Vec<u8>>,
}

/// The largest Opus packet a frame may have (RFC 6716).
pub const MAX_OPUS_PACKET: usize = 1275;

impl AudioPacket {
    /// A whole datagram: header + body (to be sealed).
    pub fn datagram(&self, session: u32, seq: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(crate::HEADER_LEN + 20 + self.opus.len() * 2);
        crate::PacketHeader {
            kind: crate::PacketKind::Audio,
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
        out.put_u64(self.frame);
        out.put_u64(self.capture_us);
        out.put_u16(self.opus.len() as u16);
        out.extend_from_slice(&self.opus);
        let prev = self.previous.as_deref().unwrap_or_default();
        out.put_u16(prev.len() as u16);
        out.extend_from_slice(prev);
    }

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        let frame = r.u64()?;
        let capture_us = r.u64()?;
        let packet = |r: &mut Reader<'_>| -> Result<Vec<u8>, DecodeError> {
            let n = usize::from(r.u16()?);
            if n > MAX_OPUS_PACKET {
                return Err(DecodeError::InvalidValue("opus packet length"));
            }
            Ok(r.take(n)?.to_vec())
        };
        let opus = packet(&mut r)?;
        if opus.is_empty() {
            return Err(DecodeError::InvalidValue("empty audio frame"));
        }
        let previous = Some(packet(&mut r)?).filter(|p| !p.is_empty());
        r.finish()?;
        Ok(AudioPacket {
            frame,
            capture_us,
            opus,
            previous,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for previous in [None, Some(vec![9; 150])] {
            let p = AudioPacket {
                frame: 1 << 40,
                capture_us: 123_456_789,
                opus: vec![7; 160],
                previous,
            };
            let d = p.datagram(5, 42);
            let (h, body) = crate::PacketHeader::decode(&d).unwrap();
            assert_eq!(h.kind, crate::PacketKind::Audio);
            assert_eq!(AudioPacket::decode(body).unwrap(), p);
        }
    }

    #[test]
    fn rejects_garbage() {
        let mut body = Vec::new();
        AudioPacket {
            frame: 1,
            capture_us: 2,
            opus: vec![1; 10],
            previous: None,
        }
        .encode(&mut body);
        assert!(AudioPacket::decode(&body[..body.len() - 3]).is_err());
        let mut long = body.clone();
        long.push(0);
        assert!(AudioPacket::decode(&long).is_err());
        let mut empty = Vec::new();
        empty.put_u64(1);
        empty.put_u64(2);
        empty.put_u16(0);
        empty.put_u16(0);
        assert!(AudioPacket::decode(&empty).is_err());
    }
}
