//! Bulk data of file transfers (protocol v8, `protocol::FilePacket`): as fast
//! as the path takes, every byte delivered.
//!
//! [`BulkSender`] cuts transfers into chunks, paces them, resends what is
//! lost and sets its own rate from the acks; [`BulkReceiver`] hands each
//! chunk over once, in any order, and acknowledges them selectively. Both are
//! pure logic: the caller supplies the clock, moves the packets and does the
//! file I/O.
//!
//! Rate: losses alone do not slow it down (a Wi-Fi link loses packets at any
//! rate; they are resent). What does is the path running out: the data
//! arrives slower than it is sent, or the round trip grows (a queue builds
//! up in front of the narrowest link). It starts at
//! [`BulkConfig::start_bps`], grows ×1.5 per round trip until what arrives falls
//! short, then follows the path: ×0.95 of what arrives when it runs out, ×0.85
//! of the best that arrived over 3 s when a queue of 30 ms built up (once per
//! what it takes to show; it drains), 0.97 of it when the round trip grew by
//! 10 ms (the queue stops growing); then back to 0.95 of it at once, and +3 %
//! per 100 ms while it keeps up, up to 1.15 of it; never above the cap. A
//! transfer after a pause starts over (the round trip it knows stays).
//!
//! A longer round trip is not always a queue: through WG it drifts by 100 ms
//! over seconds at any rate. A cut tells: a queue drains, the round trip comes
//! down. If it did not by the time the cut shows, and no more went out than
//! arrived ([`OVERRUN`]), it is the path's: it becomes the new base. Without
//! that the rate fell to the bottom and stayed there, with nothing to drain.
//! So one cut is the price of such a move, and the start ends on a longer round
//! trip only with more going out than arrives, or with losses (a full queue).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use protocol::{FILE_CHUNK_LEN, FileAck, FileData, MAX_ACK_RANGES};

/// Bytes a chunk costs on the wire besides its data (header, chunk header, tag).
const OVERHEAD: usize =
    protocol::HEADER_LEN + protocol::FILE_DATA_HEADER_LEN + protocol::AUTH_TAG_LEN;
/// How often the rate is looked at once past the start.
const ADJUST: Duration = Duration::from_millis(100);
/// Window of the arrival rate measured from the acks.
const DELIVERY_WINDOW: Duration = Duration::from_millis(250);
/// Round trip above the base this much: a queue is building up.
const QUEUE_DELAY: Duration = Duration::from_millis(30);
/// More than this times what arrives goes out: our data is filling a queue.
/// Above the share of resends at 5 % losses, below the most the probing sends
/// over what is known to pass (1.15).
const OVERRUN: f64 = 1.08;
/// A round trip that came down at least this much is a queue draining.
const DRAINING: Duration = Duration::from_millis(5);
/// A read longer than this is counted in [`BulkStats::slow_reads`].
const SLOW_READ: Duration = Duration::from_millis(20);
/// How long the best arrival rate is remembered.
const BEST_WINDOW: Duration = Duration::from_secs(3);
/// How long the smallest round trip is remembered as the base.
const BASE_RTT_WINDOW: Duration = Duration::from_secs(10);
/// The pacer's bucket: what the rate earns in this long (the sending thread
/// sleeps up to an OS tick between looks).
const BUCKET_TIME: Duration = Duration::from_millis(1);
/// The receiver acknowledges after this many chunks, or this long after the
/// first one not acknowledged yet.
const ACK_EVERY: u32 = 16;
const ACK_DELAY: Duration = Duration::from_millis(5);
/// Of the ranges an ack has room for, this many are the lowest ones.
const ACK_LOW_RANGES: usize = 24;

/// Fills a buffer from a transfer: `(transfer, offset, buf)`.
pub type ReadAt<'a> = dyn FnMut(u32, u64, &mut [u8]) -> io::Result<()> + 'a;
/// [`ReadAt`], and how long the read took.
type TimedReadAt<'a> = dyn FnMut(u32, u64, &mut [u8]) -> (io::Result<()>, Duration) + 'a;

#[derive(Debug, Clone, Copy)]
pub struct BulkConfig {
    /// The most it may send, bit/s (the server's file speed cap).
    pub cap_bps: u64,
    pub start_bps: u64,
    /// It never goes below this (a link that bad is lost anyway: the
    /// round-trip timeout handles it).
    pub min_bps: u64,
}

impl Default for BulkConfig {
    fn default() -> Self {
        BulkConfig {
            cap_bps: 100_000_000,
            start_bps: 10_000_000,
            min_bps: 1_000_000,
        }
    }
}

/// What became of a transfer on the sender's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Sending,
    /// Every byte acknowledged.
    Done,
    Cancelled,
    /// Reading it failed.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub len: u64,
    /// Bytes the receiver has.
    pub acked: u64,
    pub state: TransferState,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BulkStats {
    pub rate_bps: u64,
    /// Data arriving at the receiver, bit/s (from the acks).
    pub delivery_bps: u64,
    pub srtt: Duration,
    pub base_rtt: Duration,
    pub in_flight: usize,
    /// Chunks sent again.
    pub resent: u64,
    /// Round-trip timeouts.
    pub timeouts: u64,
    pub sent_bytes: u64,
    /// Times a longer round trip turned out to be the path's.
    pub rebased: u64,
    /// Reads of the file that took long (a network drive), and the longest.
    pub slow_reads: u64,
    pub longest_read: Duration,
}

struct Chunk {
    transfer: u32,
    offset: u64,
    len: usize,
    sent_at: Instant,
    /// Counted in `in_flight` (not marked lost).
    flying: bool,
}

struct Transfer {
    id: u32,
    len: u64,
    next: u64,
}

pub struct BulkSender {
    cfg: BulkConfig,
    epoch: Instant,
    rate_bps: u64,
    startup: bool,
    /// Transfers with data not sent yet, taken in turn.
    queue: VecDeque<Transfer>,
    progress: HashMap<u32, TransferProgress>,
    next_chunk: u64,
    /// Chunks sent and not acknowledged.
    chunks: BTreeMap<u64, Chunk>,
    /// Of them, the ones to send again, lowest first.
    lost: BTreeSet<u64>,
    /// Chunks in the order they were (last) sent; stale entries are skipped.
    order: VecDeque<(Instant, u64)>,
    /// Chunks the last ack did not tell about (more ranges than fit).
    unknown: Option<(u64, u64)>,
    in_flight: usize,
    tokens: f64,
    refilled: Option<Instant>,
    srtt: Option<Duration>,
    base_rtt: Option<(Duration, Instant)>,
    /// How much the round trip varies (RFC 6298).
    rttvar: Duration,
    /// When a queue last cut the rate.
    last_cut: Option<Instant>,
    /// A cut not shown yet: when, and the round trip then.
    after_cut: Option<(Instant, Duration)>,
    /// More went out than arrived at some time since that cut.
    overran: bool,
    /// [`BulkStats::resent`] at the last look at the rate.
    resent_seen: u64,
    /// Smallest round trip since the last adjustment.
    recent_rtt: Option<Duration>,
    /// `(when, bytes)` of the acks in the delivery window.
    delivered: VecDeque<(Instant, u64)>,
    /// `(when, data bytes sent so far)`, for what was sent a round trip
    /// before the delivery window.
    sent: VecDeque<(Instant, u64)>,
    sent_total: u64,
    /// The best arrival rates of the last [`BEST_WINDOW`], falling.
    best: VecDeque<(Instant, u64)>,
    last_progress: Instant,
    last_adjust: Instant,
    /// Ran out of data to send since the last adjustment: the rate was not
    /// tried, it says nothing about the path.
    starved: bool,
    /// A transfer came while it was idle: the next poll starts over (what
    /// it measured before the pause is stale).
    resume: bool,
    stats: BulkStats,
}

impl BulkSender {
    pub fn new(cfg: BulkConfig, now: Instant) -> BulkSender {
        let rate_bps = cfg
            .start_bps
            .clamp(cfg.min_bps, cfg.cap_bps.max(cfg.min_bps));
        BulkSender {
            cfg,
            epoch: now,
            rate_bps,
            startup: true,
            queue: VecDeque::new(),
            progress: HashMap::new(),
            next_chunk: 0,
            chunks: BTreeMap::new(),
            lost: BTreeSet::new(),
            order: VecDeque::new(),
            unknown: None,
            in_flight: 0,
            tokens: (rate_bps as f64 / 8.0 * BUCKET_TIME.as_secs_f64())
                .max((4 * (FILE_CHUNK_LEN + OVERHEAD)) as f64),
            refilled: None,
            srtt: None,
            base_rtt: None,
            rttvar: Duration::ZERO,
            last_cut: None,
            after_cut: None,
            overran: false,
            resent_seen: 0,
            recent_rtt: None,
            delivered: VecDeque::new(),
            sent: VecDeque::new(),
            sent_total: 0,
            best: VecDeque::new(),
            last_progress: now,
            last_adjust: now,
            starved: false,
            resume: false,
            stats: BulkStats::default(),
        }
    }

    /// A new transfer of `len` bytes (its id unused before).
    pub fn start(&mut self, id: u32, len: u64) {
        self.start_from(id, len, 0);
    }

    /// A transfer the receiver has up to byte `from` already (a session
    /// that reconnected): only the rest goes.
    pub fn start_from(&mut self, id: u32, len: u64, from: u64) {
        let from = from.min(len);
        self.resume |= self.idle();
        let state = if from == len {
            TransferState::Done
        } else {
            TransferState::Sending
        };
        self.progress.insert(
            id,
            TransferProgress {
                len,
                acked: from,
                state,
            },
        );
        if from < len {
            self.queue.push_back(Transfer {
                id,
                len,
                next: from,
            });
        }
    }

    /// A new cap from now on (the admin changed it).
    pub fn set_cap(&mut self, cap_bps: u64) {
        self.cfg.cap_bps = cap_bps.max(self.cfg.min_bps);
        self.rate_bps = self.rate_bps.min(self.cfg.cap_bps);
    }

    /// Stops sending `id`: what is in flight is forgotten.
    pub fn cancel(&mut self, id: u32) {
        self.drop_transfer(id, TransferState::Cancelled);
    }

    fn drop_transfer(&mut self, id: u32, state: TransferState) {
        self.queue.retain(|t| t.id != id);
        let gone: Vec<u64> = self
            .chunks
            .iter()
            .filter(|(_, c)| c.transfer == id)
            .map(|(&n, _)| n)
            .collect();
        for n in gone {
            self.forget(n);
        }
        if let Some(p) = self.progress.get_mut(&id)
            && p.state == TransferState::Sending
        {
            p.state = state;
        }
    }

    /// Removes chunk `n`; returns it.
    fn forget(&mut self, n: u64) -> Option<Chunk> {
        let c = self.chunks.remove(&n)?;
        if c.flying {
            self.in_flight -= c.len;
        }
        self.lost.remove(&n);
        Some(c)
    }

    pub fn progress(&self, id: u32) -> Option<TransferProgress> {
        self.progress.get(&id).copied()
    }

    /// Nothing left to send or to be acknowledged.
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.chunks.is_empty()
    }

    pub fn stats(&self) -> BulkStats {
        BulkStats {
            rate_bps: self.rate_bps,
            delivery_bps: self.delivery_bps().unwrap_or(0),
            srtt: self.srtt.unwrap_or_default(),
            base_rtt: self.base_rtt.map(|b| b.0).unwrap_or_default(),
            in_flight: self.in_flight,
            ..self.stats
        }
    }

    fn now_us(&self, now: Instant) -> u32 {
        now.saturating_duration_since(self.epoch).as_micros() as u32
    }

    /// The most that may be in flight: what the rate sends in a round trip,
    /// twice, plus some (acks come every few ms).
    fn window(&self) -> usize {
        let rtt = self.srtt.unwrap_or(Duration::from_millis(100)) + Duration::from_millis(50);
        let w = self.rate_bps as f64 / 8.0 * rtt.as_secs_f64() * 2.0;
        (w as usize).max(64 * FILE_CHUNK_LEN)
    }

    fn bucket(&self) -> f64 {
        (self.rate_bps as f64 / 8.0 * BUCKET_TIME.as_secs_f64())
            .max((4 * (FILE_CHUNK_LEN + OVERHEAD)) as f64)
    }

    fn refill(&mut self, now: Instant) {
        if let Some(t) = self.refilled {
            let dt = now.saturating_duration_since(t).as_secs_f64();
            self.tokens = (self.tokens + self.rate_bps as f64 / 8.0 * dt).min(self.bucket());
        }
        self.refilled = Some(now);
    }

    fn has_data(&self) -> bool {
        !self.lost.is_empty() || !self.queue.is_empty()
    }

    /// The next chunk to send now, if any: lost ones first, then new data of
    /// the transfers in turn. `read(transfer, offset, buf)` fills `buf` from
    /// the transfer (all of it: `buf` never goes past its end); a failed
    /// read fails the transfer.
    pub fn poll(&mut self, now: Instant, read: &mut ReadAt<'_>) -> Option<FileData> {
        self.poll_timed(now, &mut |transfer, offset, data| {
            let from = Instant::now();
            let got = read(transfer, offset, data);
            (got, from.elapsed())
        })
    }

    /// [`Self::poll`] with a `read` that says itself how long it took: the
    /// tests run in virtual time, and a real clock there turns a preempted
    /// thread into a round trip that much shorter (the base falls, the rest
    /// looks like a queue).
    fn poll_timed(&mut self, now: Instant, timed: &mut TimedReadAt<'_>) -> Option<FileData> {
        if std::mem::take(&mut self.resume) {
            self.start_over(now);
        }
        self.maintain(now);
        self.refill(now);
        if !self.has_data() {
            self.starved = true;
            return None;
        }
        if self.in_flight >= self.window() || self.tokens < (FILE_CHUNK_LEN + OVERHEAD) as f64 {
            return None;
        }
        let (d, read_took) = loop {
            if let Some(n) = self.lost.pop_first() {
                let floor = self.floor();
                let Some(c) = self.chunks.get_mut(&n) else {
                    continue;
                };
                let mut data = vec![0u8; c.len];
                let (got, took) = timed(c.transfer, c.offset, &mut data);
                if let Err(e) = got {
                    let t = c.transfer;
                    eprintln!("file transfer {t}: read failed: {e}");
                    self.drop_transfer(t, TransferState::Failed);
                    continue;
                }
                c.sent_at = now;
                c.flying = true;
                self.in_flight += c.len;
                self.order.push_back((now, n));
                self.stats.resent += 1;
                break (
                    FileData {
                        chunk: n,
                        floor,
                        transfer: c.transfer,
                        offset: c.offset,
                        sent_us: 0,
                        data,
                    },
                    took,
                );
            }
            let mut t = self.queue.pop_front()?;
            let len = (t.len - t.next).min(FILE_CHUNK_LEN as u64) as usize;
            let mut data = vec![0u8; len];
            let (got, took) = timed(t.id, t.next, &mut data);
            if let Err(e) = got {
                eprintln!("file transfer {}: read failed: {e}", t.id);
                self.drop_transfer(t.id, TransferState::Failed);
                if !self.has_data() {
                    return None;
                }
                continue;
            }
            let offset = t.next;
            t.next += len as u64;
            let id = t.id;
            if t.next < t.len {
                self.queue.push_back(t);
            }
            let n = self.next_chunk;
            self.next_chunk += 1;
            self.chunks.insert(
                n,
                Chunk {
                    transfer: id,
                    offset,
                    len,
                    sent_at: now,
                    flying: true,
                },
            );
            self.in_flight += len;
            self.order.push_back((now, n));
            break (
                FileData {
                    chunk: n,
                    floor: self.floor(),
                    transfer: id,
                    offset,
                    sent_us: 0,
                    data,
                },
                took,
            );
        };
        self.tokens -= (d.data.len() + OVERHEAD) as f64;
        self.sent_total += d.data.len() as u64;
        self.sent.push_back((now, self.sent_total));
        let keep = DELIVERY_WINDOW + self.srtt.unwrap_or_default() + Duration::from_secs(1);
        while self
            .sent
            .front()
            .is_some_and(|&(t, _)| now.saturating_duration_since(t) > keep)
        {
            self.sent.pop_front();
        }
        self.stats.sent_bytes += (d.data.len() + OVERHEAD) as u64;
        if read_took > SLOW_READ {
            self.stats.slow_reads += 1;
        }
        self.stats.longest_read = self.stats.longest_read.max(read_took);
        // It leaves now: a slow read is not a part of the round trip.
        Some(FileData {
            sent_us: self.now_us(now + read_took),
            ..d
        })
    }

    /// After a pause (copies through the clipboard come seconds apart): the
    /// start again, from the start rate or the last one if higher. Without
    /// it the first chunk timed out at once (the last ack was before the
    /// pause) and the arrival rate counted the pause: each transfer after
    /// one crept up from the bottom, +3 % per 100 ms.
    fn start_over(&mut self, now: Instant) {
        self.startup = true;
        self.rate_bps = self
            .rate_bps
            .max(self.cfg.start_bps)
            .clamp(self.cfg.min_bps, self.cfg.cap_bps.max(self.cfg.min_bps));
        self.last_progress = now;
        self.last_adjust = now;
        self.delivered.clear();
        self.sent.clear();
        self.best.clear();
        self.recent_rtt = None;
        self.starved = false;
        self.last_cut = None;
        self.after_cut = None;
        self.overran = false;
    }

    /// Every chunk below it is acknowledged or not wanted any more.
    fn floor(&self) -> u64 {
        self.chunks
            .first_key_value()
            .map_or(self.next_chunk, |(&n, _)| n)
    }

    /// When [`poll`](Self::poll) may have something next (the pacer, or a
    /// timeout to look at); `None`: nothing until an ack or a new transfer.
    pub fn next_poll(&self, now: Instant) -> Option<Instant> {
        let timeout = (!self.chunks.is_empty()).then(|| self.last_progress + self.rto());
        let send = (self.has_data() && self.in_flight < self.window()).then(|| {
            let need = (FILE_CHUNK_LEN + OVERHEAD) as f64 - self.tokens;
            if need <= 0.0 {
                now
            } else {
                now + Duration::from_secs_f64(need * 8.0 / self.rate_bps as f64)
            }
        });
        match (send, timeout) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn rto(&self) -> Duration {
        let rtt = self.srtt.unwrap_or(Duration::from_millis(300));
        (rtt * 4).max(Duration::from_secs(1))
    }

    pub fn on_ack(&mut self, now: Instant, ack: &FileAck) {
        let rtt = Duration::from_micros(
            self.now_us(now)
                .wrapping_sub(ack.echo_us)
                .wrapping_sub(ack.held_us) as u64,
        );
        if rtt < Duration::from_secs(10) {
            if let Some(s) = self.srtt {
                self.rttvar = (self.rttvar * 3 + s.abs_diff(rtt)) / 4;
            }
            self.srtt = Some(match self.srtt {
                None => rtt,
                Some(s) => (s * 7 + rtt) / 8,
            });
            self.recent_rtt = Some(self.recent_rtt.map_or(rtt, |r| r.min(rtt)));
            match self.base_rtt {
                Some((b, at)) if rtt > b && now.saturating_duration_since(at) < BASE_RTT_WINDOW => {
                }
                _ => self.base_rtt = Some((rtt, now)),
            }
        }

        let mut newest_acked: Option<Instant> = None;
        let mut progress = false;
        // Sooner than any round trip since it was sent: the ack is of the first
        // copy of a chunk sent again, and says nothing of what went out before
        // the second one (else one needless resend makes all those "lost").
        let shortest = self.base_rtt.map_or(Duration::ZERO, |b| b.0);
        let mut take = |s: &mut Self, n: u64| {
            if let Some(c) = s.forget(n) {
                progress = true;
                if now.saturating_duration_since(c.sent_at) >= shortest {
                    newest_acked = Some(newest_acked.map_or(c.sent_at, |t| t.max(c.sent_at)));
                }
                if let Some(p) = s.progress.get_mut(&c.transfer) {
                    p.acked += c.len as u64;
                    if p.acked >= p.len && p.state == TransferState::Sending {
                        p.state = TransferState::Done;
                    }
                }
            }
        };
        let below: Vec<u64> = self.chunks.range(..ack.below).map(|(&n, _)| n).collect();
        for n in below {
            take(self, n);
        }
        for &(s, e) in &ack.ranges {
            let got: Vec<u64> = self.chunks.range(s..e).map(|(&n, _)| n).collect();
            for n in got {
                take(self, n);
            }
        }
        self.unknown = (ack.gap > 0).then(|| {
            let g = usize::from(ack.gap);
            (ack.ranges[g - 1].1, ack.ranges[g].0)
        });
        if progress {
            self.last_progress = now;
        }
        if let Some(newest) = newest_acked {
            // Sent well before a chunk that arrived, and still missing: lost.
            let reorder = (self.srtt.unwrap_or_default() / 8).max(Duration::from_millis(2));
            self.detect(newest.checked_sub(reorder), false);
        }
        self.delivered.push_back((now, ack.bytes));
        while self
            .delivered
            .front()
            .is_some_and(|&(t, _)| now.saturating_duration_since(t) > DELIVERY_WINDOW)
            && self.delivered.len() > 2
        {
            self.delivered.pop_front();
        }
    }

    fn delivery_bps(&self) -> Option<u64> {
        let (&(t0, b0), &(t1, b1)) = (self.delivered.front()?, self.delivered.back()?);
        let dt = t1.saturating_duration_since(t0);
        (dt >= DELIVERY_WINDOW / 2)
            .then(|| (b1.saturating_sub(b0) as f64 * 8.0 / dt.as_secs_f64()) as u64)
    }

    /// Data sent over the delivery window shifted a round trip back: what
    /// the receiver should have got over the window if the path kept up.
    fn sent_bps(&self) -> Option<u64> {
        let (&(t0, _), &(t1, _)) = (self.delivered.front()?, self.delivered.back()?);
        let rtt = self.srtt?;
        let (a, b) = (t0.checked_sub(rtt)?, t1.checked_sub(rtt)?);
        let at = |t: Instant| {
            let i = self.sent.partition_point(|&(s, _)| s <= t);
            if i == 0 { 0 } else { self.sent[i - 1].1 }
        };
        let dt = b.saturating_duration_since(a);
        (dt >= DELIVERY_WINDOW / 2)
            .then(|| (at(b).saturating_sub(at(a)) as f64 * 8.0 / dt.as_secs_f64()) as u64)
    }

    /// Marks lost the chunks in flight sent before `before`, except the
    /// ones the last ack did not tell about (unless `all`).
    fn detect(&mut self, before: Option<Instant>, all: bool) {
        let Some(before) = before else {
            return;
        };
        let mut i = 0;
        while let Some(&(at, n)) = self.order.get(i) {
            let live = self
                .chunks
                .get(&n)
                .is_some_and(|c| c.sent_at == at && c.flying);
            if !live {
                self.order.remove(i);
                continue;
            }
            if at >= before {
                break;
            }
            if !all && self.unknown.is_some_and(|(lo, hi)| (lo..hi).contains(&n)) {
                i += 1;
                continue;
            }
            let c = self.chunks.get_mut(&n).unwrap();
            c.flying = false;
            self.in_flight -= c.len;
            self.lost.insert(n);
            self.order.remove(i);
        }
    }

    /// Losses by time, the timeout, the rate.
    fn maintain(&mut self, now: Instant) {
        // Not acknowledged long after it was sent: lost. Only at the tail of
        // the data, with no later chunks to show it: otherwise the order of the
        // acks tells (and the timeout below, if nothing comes at all). By time
        // alone a round trip that jumps makes everything in flight "lost":
        // through WG 3305 chunks went again in a set with none lost, the rate
        // fell, and the set took twice as long.
        if let Some(srtt) = self.srtt.filter(|_| self.queue.is_empty()) {
            let wait =
                (srtt * 2).max(srtt + self.rttvar * 4) + ACK_DELAY * 2 + Duration::from_millis(5);
            self.detect(now.checked_sub(wait), false);
        }
        if !self.chunks.is_empty() && now.saturating_duration_since(self.last_progress) > self.rto()
        {
            // Nothing acknowledged for too long (the round trip is not known
            // yet, or the path is down): everything in flight is lost. The
            // rate follows what arrives, not this.
            self.detect(Some(now + Duration::from_nanos(1)), true);
            self.unknown = None;
            self.startup = false;
            self.last_progress = now;
            self.stats.timeouts += 1;
        }
        let every = if self.startup {
            self.srtt.unwrap_or(ADJUST).max(Duration::from_millis(20)) + ACK_DELAY
        } else {
            ADJUST
        };
        if now.saturating_duration_since(self.last_adjust) < every {
            return;
        }
        self.last_adjust = now;
        let starved = std::mem::take(&mut self.starved);
        let recent = self.recent_rtt.take();
        let (Some(delivery), Some((base, _))) = (self.delivery_bps(), self.base_rtt) else {
            return;
        };
        // What arrives, counted as the rate is: with the datagrams' overhead.
        let delivery = delivery * (FILE_CHUNK_LEN + OVERHEAD) as u64 / FILE_CHUNK_LEN as u64;
        while self
            .best
            .front()
            .is_some_and(|&(t, _)| now.saturating_duration_since(t) > BEST_WINDOW)
        {
            self.best.pop_front();
        }
        while self.best.back().is_some_and(|&(_, b)| b <= delivery) {
            self.best.pop_back();
        }
        self.best.push_back((now, delivery));
        let best = self.best.front().map_or(delivery, |&(_, b)| b);
        // What went out a round trip ago, counted the same way.
        let sent = self
            .sent_bps()
            .map(|s| s as f64 * (FILE_CHUNK_LEN + OVERHEAD) as f64 / FILE_CHUNK_LEN as f64);
        let overrun = sent.is_some_and(|s| s > delivery as f64 * OVERRUN);
        // What the rate does shows after a round trip and the arrival window.
        let shows_in = self.srtt.unwrap_or_default() + DELIVERY_WINDOW;
        let mut base = base;
        // What the last cut showed.
        self.overran |= overrun;
        if let (Some(r), Some((since, then))) = (recent, self.after_cut)
            && now.saturating_duration_since(since) >= shows_in
        {
            if !self.overran && r > base + QUEUE_DELAY / 3 && r + DRAINING > then {
                // Not ours: nothing of ours is there to drain.
                base = r;
                self.base_rtt = Some((r, now));
                self.stats.rebased += 1;
            }
            self.after_cut = None;
        }
        let queued = recent.is_some_and(|r| r > base + QUEUE_DELAY);
        // Some queue already: hold the rate, do not add to it.
        let filling = recent.is_some_and(|r| r > base + QUEUE_DELAY / 3);
        // Arrives slower than it went out a round trip ago: the path ran out.
        let short = sent.is_some_and(|s| s > 0.0 && (delivery as f64) < s * 0.85);
        // The same queue does not cut it again before the cut reaches the
        // queue and comes back.
        let settled = self.last_cut.is_none_or(|t| {
            now.saturating_duration_since(t) >= self.srtt.unwrap_or_default() + ADJUST
        });
        let lost = self.stats.resent > self.resent_seen;
        self.resent_seen = self.stats.resent;
        let rate = self.rate_bps as f64;
        self.rate_bps = if self.startup {
            // A round trip that grew alone would end it at the start rate on
            // a drifting path, and from there it climbs for seconds.
            if short || (queued && (overrun || lost)) {
                self.startup = false;
                (best as f64 * if queued { 0.8 } else { 0.95 }) as u64
            } else if starved {
                self.rate_bps
            } else {
                (rate * 1.5) as u64
            }
        } else if queued && !settled {
            self.rate_bps
        } else if queued {
            // Below what the path takes for a while: the queue drains.
            self.last_cut = Some(now);
            if self.after_cut.is_none() {
                self.after_cut = recent.map(|r| (now, r));
                self.overran = false;
            }
            (rate.min(best as f64) * 0.85) as u64
        } else if short {
            (rate.min(delivery as f64) * 0.95) as u64
        } else if filling {
            // Just under what the path takes: the queue stops growing.
            rate.min(best as f64 * 0.97) as u64
        } else if starved {
            self.rate_bps
        } else if rate < best as f64 * 0.9 {
            // Back to what the path took lately (after a drain).
            (best as f64 * 0.95) as u64
        } else {
            // Probing for more, not far above what is known to pass.
            (rate * 1.03).min(best as f64 * 1.15) as u64
        };
        self.rate_bps = self.rate_bps.clamp(self.cfg.min_bps, self.cfg.cap_bps);
    }
}

/// The receiving side: which chunks arrived, and the acks.
#[derive(Default)]
pub struct BulkReceiver {
    /// Every chunk below it arrived (or is not wanted).
    below: u64,
    /// Chunks above `below` that arrived: start → end (exclusive).
    ranges: BTreeMap<u64, u64>,
    bytes: u64,
    newest: Option<(u32, Instant)>,
    unacked: u32,
    first_unacked: Option<Instant>,
    duplicates: u64,
}

impl BulkReceiver {
    pub fn new() -> BulkReceiver {
        BulkReceiver::default()
    }

    /// A chunk came: true if it is new (write it), false if it came before.
    pub fn on_data(&mut self, now: Instant, d: &FileData) -> bool {
        self.newest = Some((d.sent_us, now));
        self.unacked += 1;
        self.first_unacked.get_or_insert(now);
        let new = !self.has(d.chunk);
        if new {
            self.insert(d.chunk);
            self.bytes += d.data.len() as u64;
        } else {
            self.duplicates += 1;
        }
        if d.floor > self.below {
            self.below = d.floor;
            self.settle();
        }
        new
    }

    fn has(&self, n: u64) -> bool {
        n < self.below
            || self
                .ranges
                .range(..=n)
                .next_back()
                .is_some_and(|(_, &e)| n < e)
    }

    fn insert(&mut self, n: u64) {
        let (mut s, mut e) = (n, n + 1);
        if let Some((&ps, &pe)) = self.ranges.range(..=n).next_back()
            && pe == n
        {
            s = ps;
            self.ranges.remove(&ps);
        }
        if let Some(ne) = self.ranges.remove(&e) {
            e = ne;
        }
        self.ranges.insert(s, e);
        self.settle();
    }

    /// Folds the ranges at or below `below` into it.
    fn settle(&mut self) {
        while let Some((&s, &e)) = self.ranges.first_key_value() {
            if s > self.below {
                break;
            }
            self.ranges.remove(&s);
            self.below = self.below.max(e);
        }
    }

    pub fn ack_due(&self, now: Instant) -> bool {
        self.unacked >= ACK_EVERY
            || self
                .first_unacked
                .is_some_and(|t| now.saturating_duration_since(t) >= ACK_DELAY)
    }

    /// When an ack is due, if one will be.
    pub fn next_ack(&self) -> Option<Instant> {
        self.first_unacked.map(|t| t + ACK_DELAY)
    }

    pub fn ack(&mut self, now: Instant) -> FileAck {
        self.unacked = 0;
        self.first_unacked = None;
        let (echo_us, held_us) = match self.newest {
            Some((us, at)) => (us, now.saturating_duration_since(at).as_micros() as u32),
            None => (0, 0),
        };
        // All ranges, or the lowest ones (the oldest losses, to resend first)
        // and the newest ones (what the sender sent last).
        let all = self.ranges.len() <= MAX_ACK_RANGES;
        let (low, high) = if all {
            (MAX_ACK_RANGES, 0)
        } else {
            (ACK_LOW_RANGES, MAX_ACK_RANGES - ACK_LOW_RANGES)
        };
        let mut ranges: Vec<(u64, u64)> = self
            .ranges
            .iter()
            .take(low)
            .map(|(&s, &e)| (s, e))
            .collect();
        let n = ranges.len();
        ranges.extend(
            self.ranges
                .iter()
                .rev()
                .take(high)
                .map(|(&s, &e)| (s, e))
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        );
        FileAck {
            below: self.below,
            echo_us,
            held_us,
            bytes: self.bytes,
            ranges,
            gap: if all { 0 } else { n as u8 },
        }
    }

    /// Data bytes that arrived (each chunk once).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
