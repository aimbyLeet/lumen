//! Fragmentation with Reed–Solomon FEC, and reassembly on the receiver.
//!
//! [`Packetizer`] cuts a payload (video access unit, cursor shape) into
//! equal-size data shards, groups them into FEC blocks and appends parity
//! shards; every shard becomes one datagram: `PacketHeader`, `FragmentHeader`,
//! shard bytes (see `protocol`). [`Reassembler`] collects shards, restores missing
//! data shards once a block has any `data_shards` of its shards, and hands out
//! payloads strictly in frame order, reporting gaps as losses. Every video
//! stream (one per monitor of the host, `PacketHeader::stream`) has frames of
//! its own: numbers, order and losses do not mix.
//!
//! Datagrams here are plaintext; encryption wraps the body later.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use protocol::{
    DecodeError, FRAGMENT_HEADER_LEN, Flags, FragmentHeader, HEADER_LEN, PacketHeader, PacketKind,
};
use reed_solomon_simd::{ReedSolomonDecoder, ReedSolomonEncoder};

use crate::seq::SeqCounter;

/// Payloads larger than this are rejected on both sides (a 4K IDR at QP 16 is ~2 MB).
pub const MAX_PAYLOAD: usize = 32 << 20;

#[derive(Debug, Clone, Copy)]
pub struct FecConfig {
    /// Upper bound for one shard; even. Use `protocol::max_shard_len`.
    pub max_shard_len: usize,
    /// Data shards per FEC block at most; larger payloads use several blocks.
    pub max_block_data: u16,
    /// Parity shards per block = ceil(data * ratio), at least `min_parity` (if ratio > 0).
    pub parity_ratio: f64,
    pub min_parity: u16,
}

impl Default for FecConfig {
    fn default() -> Self {
        FecConfig {
            max_shard_len: protocol::max_shard_len(protocol::DEFAULT_MAX_DATAGRAM),
            max_block_data: 256,
            parity_ratio: 0.15,
            min_parity: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FecError {
    PayloadTooLarge(usize),
    BadConfig(&'static str),
    Codec(String),
}

impl std::fmt::Display for FecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FecError::PayloadTooLarge(n) => write!(f, "payload of {n} bytes is too large"),
            FecError::BadConfig(what) => write!(f, "bad FEC config: {what}"),
            FecError::Codec(e) => write!(f, "Reed-Solomon: {e}"),
        }
    }
}

impl std::error::Error for FecError {}

fn parity_for(cfg: &FecConfig, data: u16) -> u16 {
    if cfg.parity_ratio <= 0.0 {
        return 0;
    }
    let p = (f64::from(data) * cfg.parity_ratio).ceil() as u16;
    p.max(cfg.min_parity)
}

/// Sender side: payload → datagrams.
pub struct Packetizer {
    cfg: FecConfig,
    session: u32,
    seq: SeqCounter,
    /// Datagrams get their sequence number later, when they leave
    /// ([`PacedSender::seal_paced_with`](crate::PacedSender::seal_paced_with)):
    /// written as 0 here.
    deferred_seq: bool,
}

impl Packetizer {
    pub fn new(cfg: FecConfig, session: u32) -> Result<Self, FecError> {
        Self::with_seq(cfg, session, SeqCounter::new())
    }

    /// Numbering its datagrams from `seq`, shared with the session's other
    /// senders (one counter per key: it is the nonce).
    pub fn with_seq(cfg: FecConfig, session: u32, seq: SeqCounter) -> Result<Self, FecError> {
        if cfg.max_shard_len < 2 || !cfg.max_shard_len.is_multiple_of(2) {
            return Err(FecError::BadConfig("max_shard_len must be even and >= 2"));
        }
        if cfg.max_block_data == 0 {
            return Err(FecError::BadConfig("max_block_data must be >= 1"));
        }
        if !(0.0..=1.0).contains(&cfg.parity_ratio) {
            return Err(FecError::BadConfig("parity_ratio must be in [0, 1]"));
        }
        Ok(Packetizer {
            cfg,
            session,
            seq,
            deferred_seq: false,
        })
    }

    /// From now on datagrams carry sequence number 0: the sender numbers them
    /// as they leave (several streams' frames interleaved by the pacer would
    /// otherwise reach the receiver far out of number order).
    pub fn defer_seq(&mut self) {
        self.deferred_seq = true;
    }

    /// Its datagrams are numbered as they leave (see [`Self::defer_seq`]).
    pub fn defers_seq(&self) -> bool {
        self.deferred_seq
    }

    pub fn config(&self) -> &FecConfig {
        &self.cfg
    }

    pub fn set_parity_ratio(&mut self, ratio: f64) {
        self.cfg.parity_ratio = ratio.clamp(0.0, 1.0);
    }

    /// Hands out a sequence number for a datagram built elsewhere (control messages),
    /// so all datagrams of a session share one counter.
    pub fn take_seq(&self) -> u64 {
        self.seq.take()
    }

    /// The session's counter, for threads that send datagrams of their own.
    pub fn seq_counter(&self) -> SeqCounter {
        self.seq.clone()
    }

    /// Appends one datagram per shard to `out`: for each block, data shards then parity.
    /// `frame_flags`: [`Flags::KEYFRAME`] and/or [`Flags::RECOVERY`]; other bits are ignored.
    /// `stream`: the video stream (< `protocol::MAX_STREAMS`); 0 for cursor shapes.
    #[allow(clippy::too_many_arguments)]
    pub fn packetize(
        &mut self,
        kind: PacketKind,
        stream: u8,
        frame: u32,
        frame_flags: Flags,
        capture_time_us: u64,
        payload: &[u8],
        out: &mut Vec<Vec<u8>>,
    ) -> Result<(), FecError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(FecError::PayloadTooLarge(payload.len()));
        }
        if stream >= protocol::MAX_STREAMS {
            return Err(FecError::BadConfig("stream out of range"));
        }
        let total_len = payload.len();
        let max_shard = self.cfg.max_shard_len;
        let n_data = total_len.div_ceil(max_shard).max(1);
        // Equal shards, as small as possible: less padding for small payloads.
        let shard_len = total_len.div_ceil(n_data).next_multiple_of(2).max(2);
        let max_block = usize::from(self.cfg.max_block_data);
        let block_count = n_data.div_ceil(max_block);
        let (base, extra) = (n_data / block_count, n_data % block_count);
        if block_count > usize::from(u16::MAX) {
            return Err(FecError::PayloadTooLarge(total_len));
        }

        let flags = Flags(frame_flags.0 & (Flags::KEYFRAME.0 | Flags::RECOVERY.0));
        let mut shard_buf = vec![0u8; shard_len];
        let mut offset = 0;
        for block in 0..block_count {
            let data = (base + usize::from(block < extra)) as u16;
            let parity = parity_for(&self.cfg, data);
            let mut encoder = if parity > 0 {
                Some(
                    ReedSolomonEncoder::new(usize::from(data), usize::from(parity), shard_len)
                        .map_err(|e| FecError::Codec(e.to_string()))?,
                )
            } else {
                None
            };
            let mut frag = FragmentHeader {
                frame,
                total_len: total_len as u32,
                capture_time_us,
                block: block as u16,
                block_count: block_count as u16,
                index: 0,
                data_shards: data,
                parity_shards: parity,
            };
            for i in 0..data {
                let end = (offset + shard_len).min(total_len);
                let chunk = &payload[offset.min(total_len)..end];
                shard_buf[..chunk.len()].copy_from_slice(chunk);
                shard_buf[chunk.len()..].fill(0);
                offset += shard_len;
                if let Some(enc) = encoder.as_mut() {
                    enc.add_original_shard(&shard_buf)
                        .map_err(|e| FecError::Codec(e.to_string()))?;
                }
                frag.index = i;
                out.push(self.datagram(kind, stream, flags, &frag, &shard_buf));
            }
            if let Some(enc) = encoder.as_mut() {
                let result = enc.encode().map_err(|e| FecError::Codec(e.to_string()))?;
                let recovery: Vec<Vec<u8>> = result.recovery_iter().map(<[u8]>::to_vec).collect();
                for (j, shard) in recovery.iter().enumerate() {
                    frag.index = data + j as u16;
                    out.push(self.datagram(kind, stream, flags, &frag, shard));
                }
            }
        }
        Ok(())
    }

    fn datagram(
        &mut self,
        kind: PacketKind,
        stream: u8,
        flags: Flags,
        frag: &FragmentHeader,
        shard: &[u8],
    ) -> Vec<u8> {
        let mut d = Vec::with_capacity(HEADER_LEN + FRAGMENT_HEADER_LEN + shard.len());
        PacketHeader {
            kind,
            flags,
            stream,
            session: self.session,
            seq: if self.deferred_seq {
                0
            } else {
                self.take_seq()
            },
        }
        .encode(&mut d);
        frag.encode(&mut d);
        d.extend_from_slice(shard);
        d
    }
}

/// A reassembled payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub kind: PacketKind,
    /// Video stream; 0 for cursor shapes.
    pub stream: u8,
    pub frame: u32,
    pub keyframe: bool,
    /// First frame after a recovery request (see `Flags::RECOVERY`).
    pub recovery: bool,
    pub capture_time_us: u64,
    pub data: Vec<u8>,
    /// At least one data shard was restored from parity.
    pub recovered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Complete(Payload),
    /// Frames `first..=last` of `kind` (and video `stream`) will never be delivered.
    Lost {
        kind: PacketKind,
        stream: u8,
        first: u32,
        last: u32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReassemblyStats {
    pub datagrams: u64,
    pub malformed: u64,
    /// Shards that were not needed: duplicates, parity arriving after its block
    /// was complete, shards of frames already delivered or declared lost.
    pub stale: u64,
    pub shards_restored: u64,
    pub frames_complete: u64,
    pub frames_recovered: u64,
    pub frames_lost: u64,
}

struct Block {
    data: u16,
    parity: u16,
    shards: Vec<Option<Vec<u8>>>,
    received: u16,
    /// The highest shard index received: shards are sent in index order, so
    /// the ones below it that are missing will not come.
    highest: u16,
    /// Data bytes of the whole block once complete.
    done: Option<Vec<u8>>,
}

struct Partial {
    keyframe: bool,
    recovery: bool,
    total_len: u32,
    capture_time_us: u64,
    shard_len: usize,
    blocks: Vec<Option<Block>>,
    blocks_done: usize,
    recovered: bool,
    first_seen: Instant,
    /// Since when the frame cannot be completed any more, judging by the
    /// order of arrival (see [`REORDER_GRACE`]).
    doomed: Option<Instant>,
}

/// A frame that cannot be completed any more — too many shards of a block
/// missing below the highest one received, or a newer frame of its stream
/// already arriving — is reported lost this long after, unless the missing
/// shards come after all (reordered). Datagrams of one stream leave the host
/// in order and arrived in order through WireGuard (2026-10-24: 379 277 video
/// datagrams, none out of order), so this is a margin. Without it a loss was
/// noticed only when the next frame completed, ~17 ms later at 60 fps.
pub const REORDER_GRACE: Duration = Duration::from_millis(3);

#[derive(Default)]
struct Stream {
    partial: BTreeMap<u32, Partial>,
    /// Every frame `< next` has been delivered or reported lost.
    next: Option<u32>,
}

/// Receiver side: datagrams → payloads in frame order, per [`PacketKind`].
pub struct Reassembler {
    session: u32,
    timeout: Duration,
    video: [Stream; protocol::MAX_STREAMS as usize],
    cursor: Stream,
    stats: ReassemblyStats,
}

impl Reassembler {
    /// `timeout`: an incomplete frame older than this is declared lost.
    pub fn new(session: u32, timeout: Duration) -> Self {
        Reassembler {
            session,
            timeout,
            video: Default::default(),
            cursor: Stream::default(),
            stats: ReassemblyStats::default(),
        }
    }

    pub fn stats(&self) -> ReassemblyStats {
        self.stats
    }

    /// The frames of `kind` and `stream` (the header checked `stream`).
    fn stream(&mut self, kind: PacketKind, stream: u8) -> (&mut Stream, &mut ReassemblyStats) {
        match kind {
            PacketKind::CursorShape => (&mut self.cursor, &mut self.stats),
            _ => (&mut self.video[usize::from(stream)], &mut self.stats),
        }
    }

    /// Forgets the frames of video `stream`: the host starts it over (its
    /// frame numbers may begin anew).
    pub fn reset_stream(&mut self, stream: u8) {
        if let Some(s) = self.video.get_mut(usize::from(stream)) {
            *s = Stream::default();
        }
    }

    /// Feeds one decrypted fragment datagram (header already parsed). Completed
    /// payloads and losses are appended to `out` in frame order.
    pub fn push(
        &mut self,
        now: Instant,
        header: &PacketHeader,
        body: &[u8],
        out: &mut Vec<Delivery>,
    ) -> Result<(), DecodeError> {
        self.stats.datagrams += 1;
        let r = self.push_inner(now, header, body, out);
        if r.is_err() {
            self.stats.malformed += 1;
        }
        r
    }

    fn push_inner(
        &mut self,
        now: Instant,
        header: &PacketHeader,
        body: &[u8],
        out: &mut Vec<Delivery>,
    ) -> Result<(), DecodeError> {
        let kind = header.kind;
        // Cursor shapes have one sequence of their own, whatever the header says.
        let sid = if kind == PacketKind::Video {
            header.stream
        } else {
            0
        };
        if matches!(
            kind,
            PacketKind::Control | PacketKind::Handshake | PacketKind::Audio
        ) {
            return Err(DecodeError::InvalidValue("Reassembler: not a fragment"));
        }
        if header.session != self.session {
            self.stats.stale += 1;
            return Ok(());
        }
        let (f, shard) = FragmentHeader::decode(body)?;
        if f.total_len as usize > MAX_PAYLOAD || shard.len() % 2 != 0 {
            return Err(DecodeError::InvalidValue("fragment size"));
        }
        let keyframe = header.flags.contains(Flags::KEYFRAME);
        let recovery = header.flags.contains(Flags::RECOVERY);
        let (stream, stats) = self.stream(kind, sid);
        if stream.next.is_some_and(|n| f.frame < n) {
            stats.stale += 1;
            return Ok(());
        }

        // A newer frame arriving: the older incomplete ones got all they will get.
        if !stream.partial.contains_key(&f.frame) {
            for older in stream.partial.range_mut(..f.frame).map(|(_, p)| p) {
                older.doomed.get_or_insert(now);
            }
        }
        let p = stream.partial.entry(f.frame).or_insert_with(|| Partial {
            keyframe,
            recovery,
            total_len: f.total_len,
            capture_time_us: f.capture_time_us,
            shard_len: shard.len(),
            blocks: (0..f.block_count).map(|_| None).collect(),
            blocks_done: 0,
            recovered: false,
            first_seen: now,
            doomed: None,
        });
        if p.total_len != f.total_len
            || p.shard_len != shard.len()
            || p.blocks.len() != usize::from(f.block_count)
        {
            return Err(DecodeError::InvalidValue(
                "fragment inconsistent with frame",
            ));
        }
        let max_data = (p.total_len as usize).div_ceil(p.shard_len).max(1);
        let b = p.blocks[usize::from(f.block)].get_or_insert_with(|| Block {
            data: f.data_shards,
            parity: f.parity_shards,
            shards: vec![None; usize::from(f.data_shards) + usize::from(f.parity_shards)],
            received: 0,
            highest: 0,
            done: None,
        });
        if b.data != f.data_shards || b.parity != f.parity_shards || usize::from(b.data) > max_data
        {
            return Err(DecodeError::InvalidValue(
                "fragment inconsistent with block",
            ));
        }
        if b.done.is_some() || b.shards[usize::from(f.index)].is_some() {
            stats.stale += 1;
            return Ok(());
        }
        b.shards[usize::from(f.index)] = Some(shard.to_vec());
        b.received += 1;
        b.highest = b.highest.max(f.index);
        if b.received < b.data {
            let still_to_come = b.data + b.parity - 1 - b.highest;
            if b.received + still_to_come < b.data {
                p.doomed.get_or_insert(now);
            }
            return Ok(());
        }

        let restored = complete_block(b, p.shard_len)?;
        if restored > 0 {
            p.recovered = true;
        }
        p.blocks_done += 1;
        stats.shards_restored += restored;
        if p.blocks_done == p.blocks.len() {
            self.finish(kind, sid, f.frame, out)?;
        }
        Ok(())
    }

    /// Emits a completed frame, reporting every older undelivered frame as lost.
    fn finish(
        &mut self,
        kind: PacketKind,
        sid: u8,
        frame: u32,
        out: &mut Vec<Delivery>,
    ) -> Result<(), DecodeError> {
        let (stream, stats) = self.stream(kind, sid);
        let p = stream
            .partial
            .remove(&frame)
            .expect("finished frame exists");
        let mut data = Vec::with_capacity(p.blocks.len() * p.shard_len);
        for b in p.blocks.into_iter().flatten() {
            data.extend_from_slice(b.done.as_deref().unwrap_or_default());
        }
        if data.len() < p.total_len as usize {
            return Err(DecodeError::InvalidValue("frame shorter than total_len"));
        }
        data.truncate(p.total_len as usize);

        let lost = report_gap(stream, kind, sid, frame, out);
        stream.next = Some(frame + 1);
        stats.frames_lost += lost;
        stats.frames_complete += 1;
        stats.frames_recovered += u64::from(p.recovered);
        out.push(Delivery::Complete(Payload {
            kind,
            stream: sid,
            frame,
            keyframe: p.keyframe,
            recovery: p.recovery,
            capture_time_us: p.capture_time_us,
            data,
            recovered: p.recovered,
        }));
        Ok(())
    }

    /// Declares frames lost that cannot be completed any more (for
    /// [`REORDER_GRACE`]) or whose first shard arrived more than `timeout` ago.
    pub fn expire(&mut self, now: Instant, out: &mut Vec<Delivery>) {
        let all = (0..protocol::MAX_STREAMS)
            .map(|s| (PacketKind::Video, s))
            .chain([(PacketKind::CursorShape, 0)]);
        for (kind, sid) in all {
            let timeout = self.timeout;
            let (stream, stats) = self.stream(kind, sid);
            let Some(last) = stream
                .partial
                .iter()
                .filter(|(_, p)| {
                    now.saturating_duration_since(p.first_seen) > timeout
                        || p.doomed
                            .is_some_and(|t| now.saturating_duration_since(t) >= REORDER_GRACE)
                })
                .map(|(&f, _)| f)
                .next_back()
            else {
                continue;
            };
            let lost = report_gap(stream, kind, sid, last + 1, out);
            stream.next = Some(last + 1);
            stats.frames_lost += lost;
        }
    }
}

/// Drops every partial frame below `frame` and reports `next..frame` as lost.
/// Before the first delivery, the gap starts at the oldest partial frame.
fn report_gap(
    stream: &mut Stream,
    kind: PacketKind,
    sid: u8,
    frame: u32,
    out: &mut Vec<Delivery>,
) -> u64 {
    let oldest = stream.partial.keys().next().copied();
    stream.partial = stream.partial.split_off(&frame);
    let Some(first) = stream.next.or(oldest) else {
        return 0;
    };
    if frame <= first {
        return 0;
    }
    out.push(Delivery::Lost {
        kind,
        stream: sid,
        first,
        last: frame - 1,
    });
    u64::from(frame - first)
}

/// Fills `block.done` with the block's data bytes; returns how many shards were restored.
fn complete_block(block: &mut Block, shard_len: usize) -> Result<u64, DecodeError> {
    let data = usize::from(block.data);
    let missing: Vec<usize> = (0..data).filter(|&i| block.shards[i].is_none()).collect();
    let mut restored_shards = BTreeMap::new();
    if !missing.is_empty() {
        let codec = |_| DecodeError::InvalidValue("Reed-Solomon decode");
        let mut dec =
            ReedSolomonDecoder::new(data, usize::from(block.parity), shard_len).map_err(codec)?;
        for (i, s) in block.shards.iter().enumerate() {
            if let Some(s) = s {
                if i < data {
                    dec.add_original_shard(i, s).map_err(codec)?;
                } else {
                    dec.add_recovery_shard(i - data, s).map_err(codec)?;
                }
            }
        }
        let result = dec.decode().map_err(codec)?;
        for &i in &missing {
            let s = result
                .restored_original(i)
                .ok_or(DecodeError::InvalidValue("Reed-Solomon restore"))?;
            restored_shards.insert(i, s.to_vec());
        }
    }
    let mut bytes = Vec::with_capacity(data * shard_len);
    for i in 0..data {
        match &block.shards[i] {
            Some(s) => bytes.extend_from_slice(s),
            None => bytes.extend_from_slice(&restored_shards[&i]),
        }
    }
    block.shards = Vec::new();
    block.done = Some(bytes);
    Ok(missing.len() as u64)
}

#[cfg(test)]
#[path = "fec_tests.rs"]
mod tests;
