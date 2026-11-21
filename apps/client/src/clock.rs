//! Round-trip time to the host and the offset between its clock and ours,
//! from `Ping` / `Pong` (NTP-style). Frame timestamps are the host's clock:
//! with the offset, "capture -> presented" is right across two machines too.

/// How long a `Pong` counts for the offset. Two machines' clocks drift apart
/// (the quartz, Windows setting the time), so the best answer of the whole
/// session goes stale: the network time would go below zero.
const OFFSET_WINDOW_US: u64 = 30_000_000;

/// `Pong`s kept for the offset: more than come in [`OFFSET_WINDOW_US`] (one a second).
const KEPT: usize = 32;

/// A round trip this much slower than the window's fastest still counts as
/// fast for the offset (or a quarter of the fastest, if more), µs.
const NEAR_US: u64 = 200;

/// One `Pong`: when it came (our clock), its round trip, host clock minus ours.
#[derive(Default, Clone, Copy)]
struct Answer {
    recv_us: u64,
    rtt_us: u64,
    offset_us: i64,
}

/// RTT and clock offset seen so far in a session.
#[derive(Default, Clone, Copy)]
pub struct ClockSync {
    samples: u32,
    rtt_sum_us: u64,
    rtt_min_us: u64,
    rtt_max_us: u64,
    last_rtt_us: u64,
    /// The latest answers, a ring: `samples % KEPT` is the next place.
    recent: [Answer; KEPT],
    /// Host clock minus ours, from the answer with the smallest RTT in the
    /// last [`OFFSET_WINDOW_US`] (its one-way delays are the most nearly
    /// equal, so the estimate is best).
    offset_us: i64,
}

impl ClockSync {
    /// A `Pong`: our clock when the `Ping` left (`sent_us`) and when the
    /// answer arrived (`recv_us`), the host's clock when it answered.
    pub fn sample(&mut self, sent_us: u64, host_us: u64, recv_us: u64) {
        let Some(rtt) = recv_us.checked_sub(sent_us) else {
            return;
        };
        let mid = sent_us as i128 + i128::from(rtt) / 2;
        let offset = (i128::from(host_us) - mid) as i64;
        self.recent[self.samples as usize % KEPT] = Answer {
            recv_us,
            rtt_us: rtt,
            offset_us: offset,
        };
        if self.samples == 0 || rtt < self.rtt_min_us {
            self.rtt_min_us = rtt;
        }
        self.samples += 1;
        self.rtt_sum_us += rtt;
        self.rtt_max_us = self.rtt_max_us.max(rtt);
        self.last_rtt_us = rtt;
        let kept = &self.recent[..(self.samples as usize).min(KEPT)];
        let from = recv_us.saturating_sub(OFFSET_WINDOW_US);
        let window = || {
            kept.iter()
                .filter(move |a| a.recv_us >= from && a.recv_us <= recv_us)
        };
        // The answer just stored is always in the window: there is a best one.
        let best = window().map(|a| a.rtt_us).min().unwrap_or(rtt);
        // Of the answers about as fast as the best, the latest: an older one
        // carries the drift since. A slower one errs by at most half the extra.
        let near = best + (best / 4).max(NEAR_US);
        if let Some(a) = window()
            .filter(|a| a.rtt_us <= near)
            .max_by_key(|a| a.recv_us)
        {
            self.offset_us = a.offset_us;
        }
    }

    /// The latest RTT, ms (pings go once a second).
    pub fn last_rtt_ms(&self) -> Option<f64> {
        (self.samples > 0).then(|| self.last_rtt_us as f64 / 1e3)
    }

    /// Host clock minus ours, µs; `None` before the first `Pong`.
    pub fn offset_us(&self) -> Option<i64> {
        (self.samples > 0).then_some(self.offset_us)
    }

    /// RTT min / mean / max in ms and the number of samples.
    pub fn rtt_ms(&self) -> Option<(f64, f64, f64, u32)> {
        (self.samples > 0).then(|| {
            (
                self.rtt_min_us as f64 / 1e3,
                self.rtt_sum_us as f64 / 1e3 / f64::from(self.samples),
                self.rtt_max_us as f64 / 1e3,
                self.samples,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ClockSync;

    #[test]
    fn offset_comes_from_the_fastest_round_trip() {
        let mut c = ClockSync::default();
        assert_eq!(c.offset_us(), None);
        // Host clock 5 s ahead. Symmetric 10 ms each way.
        c.sample(1_000_000, 6_010_000, 1_020_000);
        assert_eq!(c.offset_us(), Some(5_000_000));
        // Slow, asymmetric answer (queued on the way back): ignored for the offset.
        c.sample(2_000_000, 7_010_000, 2_090_000);
        assert_eq!(c.offset_us(), Some(5_000_000));
        // Faster one wins: host 4 ms after our midpoint.
        c.sample(3_000_000, 8_006_000, 3_004_000);
        assert_eq!(c.offset_us(), Some(5_004_000));
        let (min, mean, max, n) = c.rtt_ms().unwrap();
        assert_eq!((min, max, n), (4.0, 90.0, 3));
        assert!((mean - 38.0).abs() < 1e-9);
        assert_eq!(c.last_rtt_ms(), Some(4.0));
        // Host clock behind ours; an answer "before" the ping is dropped.
        let mut c = ClockSync::default();
        c.sample(10_000_000, 3_000_500, 10_001_000);
        assert_eq!(c.offset_us(), Some(-7_000_000));
        c.sample(10_000_000, 1, 9_000_000);
        assert_eq!(c.rtt_ms().unwrap().3, 1);
    }

    #[test]
    fn offset_follows_a_drifting_clock() {
        // The host's clock runs 100 ppm fast (6 ms a minute); the fastest
        // round trip of the session is the very first one.
        let host = |ours: u64| ours + 5_000_000 + ours / 10_000;
        let mut c = ClockSync::default();
        let (rtt_first, rtt) = (1_000, 4_000);
        c.sample(0, host(rtt_first / 2), rtt_first);
        for s in 1..=600u64 {
            let sent = s * 1_000_000;
            c.sample(sent, host(sent + rtt / 2), sent + rtt);
        }
        // Ten minutes on: the host is 60 ms further ahead; the offset is the
        // one of the last 30 s, not the stale first answer.
        let now = 600_000_000 + rtt;
        let truth = host(now) as i64 - now as i64;
        let off = c.offset_us().unwrap();
        assert!((truth - off).abs() < 4_000, "truth {truth} offset {off}");
        assert_eq!(c.rtt_ms().unwrap().0, 1.0);
        // A frame captured now arrives 2 ms later: the network time is not negative.
        let capture = host(now);
        let net = (now + 2_000) as i64 - (capture as i64 - off);
        assert!(net > 0, "net {net}");
    }

    #[test]
    fn offset_with_jitter_on_the_way_back() {
        // 2 ms to the host every time; back 2..5 ms (queues): the round trip
        // jitters, the host's clock drifts 100 ppm.
        let host = |ours: u64| ours + 5_000_000 + ours / 10_000;
        let mut c = ClockSync::default();
        let mut worst = 0i64;
        for s in 0..=600u64 {
            let sent = s * 1_000_000;
            let back = 2_000 + (s * 7_919) % 3_000;
            c.sample(sent, host(sent + 2_000), sent + 2_000 + back);
            if s >= 30 {
                let now = sent + 2_000 + back;
                let err = (host(now) as i64 - now as i64) - c.offset_us().unwrap();
                worst = worst.max(err.abs());
            }
        }
        // Half the extra allowed over the fastest, and a few seconds' drift.
        assert!(worst < 1_000, "worst {worst}");
    }
}
