//! Token-bucket pacer: spreads queued packets over time. Pure logic; the caller
//! supplies the clock and does the sleeping (see `net::sleep_until`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const NANO: u128 = 1_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct PacerConfig {
    /// Sustained rate in bits per second.
    pub rate_bps: u64,
    /// Bucket capacity in bytes: the largest back-to-back burst.
    pub burst_bytes: usize,
}

/// Packet queue released through a token bucket.
///
/// Packets wait in lanes (one per video stream: monitors of the host); the
/// head is taken from the non-empty lanes in turn, one packet each, so frames
/// of several monitors sent at once arrive side by side instead of one after
/// the other. Within a lane the order is FIFO. [`Pacer::push`] uses lane 0.
///
/// Tokens are counted in bit·ns (1 bit = 1e9 units), so a refill of `dt` ns at
/// `rate_bps` adds exactly `rate_bps * dt` units: no rounding, no drift.
/// The bucket starts full. A packet larger than `burst_bytes` can never be
/// covered by the bucket, so it is released once the bucket is full and
/// empties it (tokens clamp to zero, never negative).
pub struct Pacer {
    rate_bps: u64,
    cap: u128,
    lanes: Vec<VecDeque<Vec<u8>>>,
    /// The lane whose turn it is.
    turn: usize,
    queued_packets: usize,
    queued_bytes: usize,
    tokens: u128,
    /// Time up to which `tokens` is accurate; `None` until the first call.
    last: Option<Instant>,
}

impl Pacer {
    /// # Panics
    /// Panics if `rate_bps == 0` or `burst_bytes == 0`.
    pub fn new(cfg: PacerConfig) -> Self {
        assert!(cfg.rate_bps > 0, "pacer rate must be non-zero");
        assert!(cfg.burst_bytes > 0, "pacer burst must be non-zero");
        let cap = cfg.burst_bytes as u128 * 8 * NANO;
        Self {
            rate_bps: cfg.rate_bps,
            cap,
            lanes: vec![VecDeque::new()],
            turn: 0,
            queued_packets: 0,
            queued_bytes: 0,
            tokens: cap,
            last: None,
        }
    }

    pub fn push(&mut self, packet: Vec<u8>) {
        self.push_lane(0, packet);
    }

    /// Queues `packet` in `lane` (a video stream).
    pub fn push_lane(&mut self, lane: usize, packet: Vec<u8>) {
        if self.lanes.len() <= lane {
            self.lanes.resize_with(lane + 1, VecDeque::new);
        }
        self.queued_bytes += packet.len();
        self.queued_packets += 1;
        self.lanes[lane].push_back(packet);
    }

    /// The lane the next packet comes from: the first non-empty one from `turn` on.
    fn head_lane(&self) -> Option<usize> {
        let n = self.lanes.len();
        (0..n)
            .map(|i| (self.turn + i) % n)
            .find(|&l| !self.lanes[l].is_empty())
    }

    fn head(&self) -> Option<&Vec<u8>> {
        self.lanes[self.head_lane()?].front()
    }

    /// Returns the head packet if the bucket covers it, consuming its tokens.
    pub fn poll(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.refill(now);
        let lane = self.head_lane()?;
        let len = self.lanes[lane].front()?.len();
        if self.tokens < self.cost(len) {
            return None;
        }
        self.tokens = self.tokens.saturating_sub(len as u128 * 8 * NANO);
        let p = self.lanes[lane].pop_front()?;
        self.turn = (lane + 1) % self.lanes.len();
        self.queued_bytes -= p.len();
        self.queued_packets -= 1;
        Some(p)
    }

    /// When the head packet becomes releasable; `now` if it already is.
    /// `None` if the queue is empty.
    pub fn next_release(&self, now: Instant) -> Option<Instant> {
        let need = self.cost(self.head()?.len());
        let (tokens, at) = self.projected(now);
        if tokens >= need {
            return Some(at);
        }
        let wait_ns = (need - tokens).div_ceil(self.rate_bps as u128);
        Some(at + Duration::from_nanos(wait_ns.min(u64::MAX as u128) as u64))
    }

    /// Changes the rate. Applies from the last `poll` time on (tokens up to
    /// that point were earned at the old rate); use `set_rate_at` to pick the
    /// switch time explicitly.
    ///
    /// # Panics
    /// Panics if `rate_bps == 0`.
    pub fn set_rate(&mut self, rate_bps: u64) {
        assert!(rate_bps > 0, "pacer rate must be non-zero");
        self.rate_bps = rate_bps;
    }

    /// Like `set_rate`, but first accounts tokens earned until `now` at the old rate.
    pub fn set_rate_at(&mut self, now: Instant, rate_bps: u64) {
        self.refill(now);
        self.set_rate(rate_bps);
    }

    /// Changes the largest back-to-back burst from `now` on (tokens above the
    /// new capacity are dropped).
    ///
    /// # Panics
    /// Panics if `burst_bytes == 0`.
    pub fn set_burst_at(&mut self, now: Instant, burst_bytes: usize) {
        assert!(burst_bytes > 0, "pacer burst must be non-zero");
        self.refill(now);
        self.cap = burst_bytes as u128 * 8 * NANO;
        self.tokens = self.tokens.min(self.cap);
    }

    pub fn queued_packets(&self) -> usize {
        self.queued_packets
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// Tokens needed to release a packet of `len` bytes.
    fn cost(&self, len: usize) -> u128 {
        (len as u128 * 8 * NANO).min(self.cap)
    }

    /// Token count and the time it is valid for, without mutating.
    fn projected(&self, now: Instant) -> (u128, Instant) {
        match self.last {
            None => (self.tokens, now),
            Some(last) => {
                let at = now.max(last);
                let dt = (at - last).as_nanos();
                ((self.tokens + self.rate_bps as u128 * dt).min(self.cap), at)
            }
        }
    }

    fn refill(&mut self, now: Instant) {
        let (tokens, at) = self.projected(now);
        self.tokens = tokens;
        self.last = Some(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(i: u32, len: usize) -> Vec<u8> {
        let mut p = vec![0u8; len];
        p[..4].copy_from_slice(&i.to_le_bytes());
        p
    }

    fn id(p: &[u8]) -> u32 {
        u32::from_le_bytes(p[..4].try_into().unwrap())
    }

    #[test]
    fn rate_burst_and_order() {
        let (rate, burst, len, n) = (100_000_000u64, 12_000usize, 1200usize, 1000u32);
        let t0 = Instant::now();
        let mut p = Pacer::new(PacerConfig {
            rate_bps: rate,
            burst_bytes: burst,
        });
        for i in 0..n {
            p.push(pkt(i, len));
        }
        assert_eq!(p.queued_packets(), n as usize);
        assert_eq!(p.queued_bytes(), n as usize * len);
        let mut cum = 0u128;
        let mut now = t0;
        let mut last_t = t0;
        for i in 0..n {
            let t = p.next_release(now).unwrap();
            assert!(t >= now);
            let out = p.poll(t).expect("releasable at next_release");
            assert_eq!(id(&out), i, "FIFO order");
            cum += len as u128;
            let dt = (t - t0).as_nanos();
            // (a) cumulative bytes <= burst + rate * dt, in exact integers.
            assert!(
                cum * 8 * NANO <= burst as u128 * 8 * NANO + rate as u128 * dt,
                "packet {i} released too early"
            );
            now = t;
            last_t = t;
        }
        let expect = Duration::from_secs_f64(((n as usize * len - burst) * 8) as f64 / rate as f64);
        let got = last_t - t0;
        let diff = got.abs_diff(expect);
        eprintln!("pacer: last release {got:?}, ideal {expect:?}, diff {diff:?}");
        assert!(diff <= Duration::from_micros(96));
        assert_eq!(p.queued_packets(), 0);
        assert!(p.next_release(now).is_none());
    }

    #[test]
    fn set_rate_changes_slope() {
        let t0 = Instant::now();
        let mut p = Pacer::new(PacerConfig {
            rate_bps: 100_000_000,
            burst_bytes: 12_000,
        });
        for i in 0..400 {
            p.push(pkt(i, 1200));
        }
        let mut now = t0;
        let mut times = vec![];
        for i in 0..200 {
            if i == 100 {
                p.set_rate(50_000_000);
            }
            let t = p.next_release(now).unwrap();
            p.poll(t).unwrap();
            times.push(t);
            now = t;
        }
        // Index 10.. is steady state (burst of 10 packets drained first).
        let before = times[60] - times[59];
        let after = times[160] - times[159];
        eprintln!("pacer set_rate: interval before {before:?}, after {after:?}");
        assert_eq!(before, Duration::from_micros(96));
        assert_eq!(after, Duration::from_micros(192));
    }

    #[test]
    fn oversize_packet_and_idle_cap() {
        let t0 = Instant::now();
        let mut p = Pacer::new(PacerConfig {
            rate_bps: 8_000_000, // 1 MB/s
            burst_bytes: 1000,
        });
        p.push(pkt(0, 3000));
        p.push(pkt(1, 1000));
        // Bucket starts full, so the oversize packet goes immediately and empties it.
        assert_eq!(p.next_release(t0), Some(t0));
        assert!(p.poll(t0).is_some());
        assert_eq!(p.next_release(t0), Some(t0 + Duration::from_millis(1)));
        // A long idle period does not bank more than `burst` bytes.
        let later = t0 + Duration::from_secs(10);
        p.push(pkt(2, 1000));
        p.push(pkt(3, 1000));
        assert!(p.poll(later).is_some());
        assert!(p.poll(later).is_none());
    }

    #[test]
    fn a_smaller_burst_spreads_a_frame() {
        let t0 = Instant::now();
        let mut p = Pacer::new(PacerConfig {
            rate_bps: 96_000_000,
            burst_bytes: 64 << 10,
        });
        p.set_burst_at(t0, 2400);
        for i in 0..10 {
            p.push(pkt(i, 1200));
        }
        // Two packets at once, then one per 100 µs (1200 B at 96 Mbit/s).
        assert!(p.poll(t0).is_some() && p.poll(t0).is_some());
        assert!(p.poll(t0).is_none());
        assert_eq!(p.next_release(t0), Some(t0 + Duration::from_micros(100)));
    }

    #[test]
    fn lanes_take_turns() {
        let t0 = Instant::now();
        let mut p = Pacer::new(PacerConfig {
            rate_bps: 1_000_000_000,
            burst_bytes: 1 << 20,
        });
        // Stream 0: 4 packets, stream 1: 2, stream 2: 3; all queued at once.
        for (lane, n) in [(0, 4), (1, 2), (2, 3)] {
            for i in 0..n {
                p.push_lane(lane, pkt(lane as u32 * 100 + i, 1200));
            }
        }
        assert_eq!(p.queued_packets(), 9);
        let order: Vec<u32> = std::iter::from_fn(|| p.poll(t0)).map(|d| id(&d)).collect();
        assert_eq!(order, [0, 100, 200, 1, 101, 201, 2, 202, 3]);
        assert_eq!((p.queued_packets(), p.queued_bytes()), (0, 0));
        // One lane alone: plain FIFO.
        for i in 0..3 {
            p.push_lane(1, pkt(i, 1200));
        }
        let order: Vec<u32> = std::iter::from_fn(|| p.poll(t0)).map(|d| id(&d)).collect();
        assert_eq!(order, [0, 1, 2]);
    }

    #[test]
    #[should_panic]
    fn zero_rate_panics() {
        Pacer::new(PacerConfig {
            rate_bps: 0,
            burst_bytes: 1,
        });
    }
}
