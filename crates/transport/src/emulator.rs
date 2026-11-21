//! Network link emulator: Gilbert-Elliott loss, bottleneck queue, delay/jitter.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::time::{Duration, Instant};

use crate::rng::Rng;

/// Link parameters. See field docs for units and ranges.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Stationary loss probability, `0.0..1.0`.
    pub loss: f64,
    /// Mean length of a loss burst in packets, `>= 1.0`; 1.0 = independent losses.
    pub mean_burst_len: f64,
    /// Fixed one-way delay.
    pub delay: Duration,
    /// Extra delay, uniform in `[0, jitter]`.
    pub jitter: Duration,
    /// `false`: delivery order equals send order (jitter never reorders).
    pub reorder: bool,
    /// Bottleneck rate; `None` = unlimited.
    pub bandwidth_bps: Option<u64>,
    /// Bottleneck queue size; tail-drop beyond it (ignored if bandwidth is unlimited).
    pub queue_limit_bytes: usize,
    /// RNG seed; the same seed yields the same drop/jitter pattern.
    pub seed: u64,
    /// Every this often (from the first packet) the link stalls for `stall_len`:
    /// what would arrive meanwhile arrives together when it ends (a Wi-Fi hiccup).
    pub stall_every: Option<Duration>,
    pub stall_len: Duration,
    /// Extra delay that drifts on its own, up and down between zero and this
    /// over `wander_period`, with no queue behind it (seen through WG: the
    /// round trip went 72 -> 190 ms over seconds, at any rate).
    pub wander: Duration,
    pub wander_period: Duration,
}

impl Default for LinkConfig {
    /// A perfect link: no loss, no delay, unlimited bandwidth, 1 MiB queue.
    fn default() -> Self {
        Self {
            loss: 0.0,
            mean_burst_len: 1.0,
            delay: Duration::ZERO,
            jitter: Duration::ZERO,
            reorder: false,
            bandwidth_bps: None,
            queue_limit_bytes: 1 << 20,
            seed: 0x4C75_6D65_6E00_0001,
            stall_every: None,
            stall_len: Duration::ZERO,
            wander: Duration::ZERO,
            wander_period: Duration::from_secs(7),
        }
    }
}

impl LinkConfig {
    /// Checks parameter ranges. Also rejects combinations where the requested
    /// stationary loss is unreachable with the given burst length
    /// (`loss / (1 - loss) > mean_burst_len`, i.e. p(good->bad) would exceed 1)
    /// and a zero bandwidth.
    pub fn validate(&self) -> Result<(), String> {
        if self.loss.is_nan() || !(0.0..1.0).contains(&self.loss) {
            return Err(format!("loss must be in [0, 1), got {}", self.loss));
        }
        if self.mean_burst_len.is_nan() || self.mean_burst_len < 1.0 {
            return Err(format!(
                "mean_burst_len must be >= 1, got {}",
                self.mean_burst_len
            ));
        }
        if self.mean_burst_len.is_infinite() {
            return Err("mean_burst_len must be finite".into());
        }
        if self.loss / (1.0 - self.loss) / self.mean_burst_len > 1.0 {
            return Err("loss too high for this mean_burst_len".into());
        }
        if self.bandwidth_bps == Some(0) {
            return Err("bandwidth_bps must be non-zero".into());
        }
        Ok(())
    }
}

/// Counters since creation. `delivered` counts packets returned by `poll`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub sent: u64,
    pub delivered: u64,
    pub dropped_loss: u64,
    pub dropped_queue: u64,
    pub bytes_delivered: u64,
}

struct Pending {
    at: Instant,
    seq: u64,
    packet: Vec<u8>,
}

impl PartialEq for Pending {
    fn eq(&self, o: &Self) -> bool {
        (self.at, self.seq) == (o.at, o.seq)
    }
}
impl Eq for Pending {}
impl PartialOrd for Pending {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Pending {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.at, self.seq).cmp(&(o.at, o.seq))
    }
}

/// Deterministic link model; the caller drives time via `now` arguments.
pub struct LinkEmulator {
    cfg: LinkConfig,
    rng: Rng,
    p_gb: f64,
    p_bg: f64,
    bad: bool,
    /// Packets in the bottleneck: (departure time, length).
    queue: VecDeque<(Instant, usize)>,
    queued_bytes: usize,
    last_departure: Option<Instant>,
    last_delivery: Option<Instant>,
    heap: BinaryHeap<Reverse<Pending>>,
    seq: u64,
    stats: LinkStats,
    /// The first packet's time: stalls are counted from it.
    origin: Option<Instant>,
}

impl LinkEmulator {
    /// Creates an emulator.
    ///
    /// # Panics
    /// Panics if `cfg.validate()` fails.
    pub fn new(cfg: LinkConfig) -> Self {
        if let Err(e) = cfg.validate() {
            panic!("invalid LinkConfig: {e}");
        }
        let p_bg = 1.0 / cfg.mean_burst_len;
        let p_gb = cfg.loss * p_bg / (1.0 - cfg.loss);
        let mut rng = Rng::new(cfg.seed);
        let bad = rng.next_f64() < cfg.loss;
        Self {
            cfg,
            rng,
            p_gb,
            p_bg,
            bad,
            queue: VecDeque::new(),
            queued_bytes: 0,
            last_departure: None,
            last_delivery: None,
            heap: BinaryHeap::new(),
            seq: 0,
            stats: LinkStats::default(),
            origin: None,
        }
    }

    /// Offers a packet to the link at time `now`. `now` must not go backwards.
    pub fn send(&mut self, now: Instant, packet: Vec<u8>) {
        self.stats.sent += 1;

        // Loss chain, one step per packet.
        let u = self.rng.next_f64();
        self.bad = if self.bad {
            u >= self.p_bg
        } else {
            u < self.p_gb
        };
        if self.bad {
            self.stats.dropped_loss += 1;
            return;
        }

        let mut departure = now;
        if let Some(bps) = self.cfg.bandwidth_bps {
            while let Some(&(dep, len)) = self.queue.front() {
                if dep > now {
                    break;
                }
                self.queued_bytes -= len;
                self.queue.pop_front();
            }
            if self.queued_bytes + packet.len() > self.cfg.queue_limit_bytes {
                self.stats.dropped_queue += 1;
                return;
            }
            let start = self.last_departure.map_or(now, |d| d.max(now));
            let nanos = packet.len() as u128 * 8 * 1_000_000_000 / bps as u128;
            departure = start + Duration::from_nanos(nanos.min(u64::MAX as u128) as u64);
            self.last_departure = Some(departure);
            self.queued_bytes += packet.len();
            self.queue.push_back((departure, packet.len()));
        }

        let jitter_ns = self.cfg.jitter.as_nanos().min(u64::MAX as u128) as u64;
        let extra = Duration::from_nanos(self.rng.below_inclusive(jitter_ns));
        let wander = self.wander_at(now);
        let mut at = self.after_stall(now, departure + self.cfg.delay + extra + wander);
        if !self.cfg.reorder {
            if let Some(prev) = self.last_delivery {
                at = at.max(prev);
            }
            self.last_delivery = Some(at);
        }
        self.heap.push(Reverse(Pending {
            at,
            seq: self.seq,
            packet,
        }));
        self.seq += 1;
    }

    /// The drifting part of the delay at `now`.
    fn wander_at(&mut self, now: Instant) -> Duration {
        let period = self.cfg.wander_period.as_secs_f64();
        if self.cfg.wander.is_zero() || period <= 0.0 {
            return Duration::ZERO;
        }
        let origin = *self.origin.get_or_insert(now);
        let phase = (now - origin).as_secs_f64() / period % 1.0;
        self.cfg.wander.mul_f64(1.0 - (2.0 * phase - 1.0).abs())
    }

    /// A delivery due inside a stall waits for its end.
    fn after_stall(&mut self, now: Instant, at: Instant) -> Instant {
        let Some(every) = self.cfg.stall_every.filter(|e| !e.is_zero()) else {
            return at;
        };
        let origin = *self.origin.get_or_insert(now);
        let since = at.saturating_duration_since(origin).as_nanos();
        let phase = since % every.as_nanos();
        if phase < self.cfg.stall_len.as_nanos() {
            at + Duration::from_nanos((self.cfg.stall_len.as_nanos() - phase) as u64)
        } else {
            at
        }
    }

    /// Returns the next packet whose delivery time is `<= now`.
    pub fn poll(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.heap.peek()?.0.at > now {
            return None;
        }
        let Reverse(p) = self.heap.pop()?;
        self.stats.delivered += 1;
        self.stats.bytes_delivered += p.packet.len() as u64;
        Some(p.packet)
    }

    /// Delivery time of the next packet in flight, if any.
    pub fn next_delivery(&self) -> Option<Instant> {
        self.heap.peek().map(|p| p.0.at)
    }

    pub fn stats(&self) -> LinkStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn id_packet(i: u64, len: usize) -> Vec<u8> {
        let mut p = vec![0u8; len.max(8)];
        p[..8].copy_from_slice(&i.to_le_bytes());
        p
    }

    fn id_of(p: &[u8]) -> u64 {
        u64::from_le_bytes(p[..8].try_into().unwrap())
    }

    /// Sends `n` packets at one instant, returns per-packet delivered flags.
    fn loss_pattern(cfg: LinkConfig, n: u64) -> Vec<bool> {
        let t0 = Instant::now();
        let mut e = LinkEmulator::new(cfg);
        let mut got = vec![false; n as usize];
        for i in 0..n {
            e.send(t0, id_packet(i, 100));
        }
        while let Some(p) = e.poll(t0) {
            got[id_of(&p) as usize] = true;
        }
        got
    }

    fn run_lengths(got: &[bool]) -> Vec<usize> {
        let mut runs = vec![];
        let mut cur = 0;
        for &g in got {
            if g {
                if cur > 0 {
                    runs.push(cur);
                }
                cur = 0;
            } else {
                cur += 1;
            }
        }
        if cur > 0 {
            runs.push(cur);
        }
        runs
    }

    #[test]
    fn default_is_perfect() {
        let t0 = Instant::now();
        let mut e = LinkEmulator::new(LinkConfig::default());
        for i in 0..1000 {
            e.send(t0, id_packet(i, 50 + i as usize % 7));
        }
        for i in 0..1000 {
            assert_eq!(e.next_delivery(), Some(t0));
            let p = e.poll(t0).unwrap();
            assert_eq!(p, id_packet(i, 50 + i as usize % 7));
        }
        assert!(e.poll(t0).is_none());
        assert_eq!(e.stats().delivered, 1000);
        assert_eq!(e.stats().sent, 1000);
    }

    #[test]
    fn bernoulli_loss() {
        for (loss, seed) in [(0.01, 1), (0.05, 2)] {
            let cfg = LinkConfig {
                loss,
                seed,
                ..Default::default()
            };
            let got = loss_pattern(cfg, 100_000);
            let lost = got.iter().filter(|g| !**g).count() as f64 / 100_000.0;
            eprintln!(
                "bernoulli target {:.2} % measured {:.3} %",
                loss * 100.0,
                lost * 100.0
            );
            assert!((lost - loss).abs() < 0.003);
        }
    }

    #[test]
    fn burst_loss() {
        let cfg = LinkConfig {
            loss: 0.05,
            mean_burst_len: 4.0,
            seed: 3,
            ..Default::default()
        };
        let n = 200_000;
        let got = loss_pattern(cfg, n);
        let lost = got.iter().filter(|g| !**g).count() as f64 / n as f64;
        let runs = run_lengths(&got);
        let mean = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
        eprintln!(
            "burst loss measured {:.3} % (target 5), mean run {:.3} (target 4), runs {}",
            lost * 100.0,
            mean,
            runs.len()
        );
        assert!((lost - 0.05).abs() < 0.005);
        assert!((mean - 4.0).abs() < 0.6);
    }

    #[test]
    fn fixed_delay() {
        let t0 = Instant::now();
        let mut e = LinkEmulator::new(LinkConfig {
            delay: 20 * MS,
            ..Default::default()
        });
        e.send(t0, vec![1, 2, 3]);
        assert_eq!(e.next_delivery(), Some(t0 + 20 * MS));
        assert!(e.poll(t0 + Duration::from_micros(19_999)).is_none());
        assert_eq!(e.poll(t0 + 20 * MS), Some(vec![1, 2, 3]));
        assert_eq!(e.next_delivery(), None);
    }

    #[test]
    fn stalls_hold_deliveries_until_they_end() {
        let t0 = Instant::now();
        let mut e = LinkEmulator::new(LinkConfig {
            stall_every: Some(100 * MS),
            stall_len: 30 * MS,
            ..Default::default()
        });
        // The first packet opens a stall at t0; later ones fall inside or after.
        for (ms, due) in [(0, 30), (10, 30), (29, 30), (30, 30), (50, 50), (110, 130)] {
            e.send(t0 + ms * MS, vec![ms as u8]);
            let got = e.heap.iter().map(|p| p.0.at).max().unwrap();
            assert_eq!(got, t0 + due * MS, "packet sent at {ms} ms");
        }
    }

    #[test]
    fn jitter_order() {
        let t0 = Instant::now();
        let step = Duration::from_micros(100);
        let n = 10_000u64;
        let run = |reorder: bool| {
            let mut e = LinkEmulator::new(LinkConfig {
                delay: 10 * MS,
                jitter: 5 * MS,
                reorder,
                seed: 5,
                ..Default::default()
            });
            for i in 0..n {
                e.send(t0 + step * i as u32, id_packet(i, 100));
            }
            let end = t0 + step * n as u32 + 100 * MS;
            let mut out = vec![];
            while let Some(at) = e.next_delivery() {
                let p = e.poll(end).unwrap();
                out.push((id_of(&p), at - (t0 + step * id_of(&p) as u32)));
            }
            out
        };
        let ordered = run(false);
        assert_eq!(ordered.len(), n as usize);
        assert!(ordered.windows(2).all(|w| w[0].0 < w[1].0));
        let (min, max) = ordered
            .iter()
            .fold((Duration::MAX, Duration::ZERO), |(lo, hi), &(_, d)| {
                (lo.min(d), hi.max(d))
            });
        eprintln!("jitter reorder=false delay range {min:?} .. {max:?}");
        assert!(min >= 10 * MS && max <= 15 * MS);

        let shuffled = run(true);
        let inversions = shuffled.windows(2).filter(|w| w[0].0 > w[1].0).count();
        eprintln!("jitter reorder=true adjacent inversions {inversions}");
        assert!(inversions > 0);
    }

    #[test]
    fn bandwidth_limit() {
        let t0 = Instant::now();
        let bps = 250_000_000u64;
        let limit = 1 << 20;
        let mut e = LinkEmulator::new(LinkConfig {
            bandwidth_bps: Some(bps),
            queue_limit_bytes: limit,
            ..Default::default()
        });
        let len = 1200usize;
        let step_ns = len as u64 * 8 * 1_000_000_000 / 500_000_000; // 19_200 ns
        let n = 1_000_000_000 / step_ns;
        let mut sent_at = vec![];
        let mut max_q = Duration::ZERO;
        let mut drain = |e: &mut LinkEmulator, now: Instant, sent_at: &[Instant]| {
            while let Some(at) = e.next_delivery() {
                if at > now {
                    break;
                }
                let p = e.poll(now).unwrap();
                max_q = max_q.max(at - sent_at[id_of(&p) as usize]);
            }
        };
        for i in 0..n {
            let now = t0 + Duration::from_nanos(i * step_ns);
            sent_at.push(now);
            e.send(now, id_packet(i, len));
            drain(&mut e, now, &sent_at);
        }
        drain(&mut e, t0 + Duration::from_secs(1), &sent_at);
        let s = e.stats();
        let mbps = s.bytes_delivered as f64 * 8.0 / 1e6;
        let ns_at = |bytes: u64| bytes * 8 * 1_000_000_000 / bps;
        let bound = Duration::from_nanos(ns_at(limit as u64) + ns_at(len as u64));
        eprintln!(
            "bandwidth: delivered {mbps:.2} Mbit/s over 1 s, dropped_queue {}, max queue delay {:?} (bound {:?})",
            s.dropped_queue, max_q, bound
        );
        assert!((mbps - 250.0).abs() < 2.5);
        assert!(s.dropped_queue > 0);
        assert!(max_q <= bound);
    }

    #[test]
    fn seed_determinism() {
        let mk = |seed| LinkConfig {
            loss: 0.1,
            mean_burst_len: 3.0,
            seed,
            ..Default::default()
        };
        let a = loss_pattern(mk(7), 5000);
        assert_eq!(a, loss_pattern(mk(7), 5000));
        assert_ne!(a, loss_pattern(mk(8), 5000));
    }

    #[test]
    fn validation() {
        let bad = |f: fn(&mut LinkConfig)| {
            let mut c = LinkConfig::default();
            f(&mut c);
            c.validate().is_err()
        };
        assert!(LinkConfig::default().validate().is_ok());
        assert!(bad(|c| c.loss = 1.0));
        assert!(bad(|c| c.loss = -0.1));
        assert!(bad(|c| c.mean_burst_len = 0.5));
        assert!(bad(|c| c.loss = f64::NAN));
        assert!(bad(|c| c.mean_burst_len = f64::NAN));
    }
}
