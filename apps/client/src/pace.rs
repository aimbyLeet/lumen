//! The client picks the host's pacing (`ControlMessage::SetPacing`) from how
//! frames arrive; no setting (owner's decision 2026-10-24).
//!
//! Inside a frame, datagrams arrive at the speed of the slowest hop: a home
//! link of 100 Mbit/s shows as ~100 Mbit/s even when the host sends faster.
//! Sending a frame faster than that only fills a queue on the way, and a
//! queue that overflows drops the frame's tail (bursts of lost datagrams that
//! FEC cannot repair: a freeze until the recovery frame). Through WG over the
//! owner's Wi-Fi, 150 Mbit/s with 8 KiB bursts halved the freezes against the
//! host's 64 KiB bursts at LAN speed, for +3 ms p50 latency; 120 Mbit/s froze
//! no less but cost +7 ms (docs: CLAUDE.md, «Подводные камни»).
//!
//! Every second, from the frames of at least [`MIN_PACKETS`] datagrams:
//! - frames arrive clearly slower than the pacing → pace at 0.9 × that speed;
//! - bursts of lost datagrams in at least two frames → 15 % slower;
//! - [`CLEAN_TO_PROBE`] without such bursts → 10 % faster, up to [`MAX_KBPS`].

use std::time::{Duration, Instant};

use crate::arrival::FrameArrival;

/// Where pacing starts and the most it goes to, kbit/s.
pub const MAX_KBPS: u32 = 150_000;
/// The least: a 4K frame at the 40 Mbit/s cap must still leave within a frame interval.
pub const MIN_KBPS: u32 = 60_000;
/// Frame data back to back at most.
pub const BURST_BYTES: u32 = 8 << 10;
/// Frames smaller than this tell little about the path's speed.
pub const MIN_PACKETS: u32 = 20;
/// Lost datagrams in a row inside a block that count as a burst.
const BURST_LOSS: u16 = 3;
const WINDOW: Duration = Duration::from_secs(1);
const CLEAN_TO_PROBE: Duration = Duration::from_secs(10);
/// Frames arriving below this share of the pacing: the path is slower.
const SLOWER: f64 = 0.85;

/// What one frame's datagrams showed: speed from the first datagram to the
/// last, bit/s (None for small frames), and whether a block lost at least
/// [`BURST_LOSS`] datagrams in a row (or at its end).
fn judge(f: &FrameArrival) -> (Option<f64>, bool) {
    let rate_bps = (f.packets >= MIN_PACKETS && f.last_us > f.first_us)
        .then(|| f.bytes as f64 * 8.0 * 1e6 / (f.last_us - f.first_us) as f64);
    let burst_loss = f.blocks.iter().flatten().any(|b| {
        let mut run = 0u16;
        b.got.iter().any(|&g| {
            run = if g { 0 } else { run + 1 };
            run >= BURST_LOSS
        })
    });
    (rate_bps, burst_loss)
}

pub struct AutoPace {
    kbps: u32,
    window_start: Option<Instant>,
    rates: Vec<f64>,
    bursts: u32,
    clean_since: Option<Instant>,
}

impl Default for AutoPace {
    fn default() -> Self {
        AutoPace {
            kbps: MAX_KBPS,
            window_start: None,
            rates: Vec::new(),
            bursts: 0,
            clean_since: None,
        }
    }
}

impl AutoPace {
    /// Starting from `kbps` (within [`MIN_KBPS`]..=[`MAX_KBPS`]) instead of
    /// the top: a reconnection keeps what the link was found to take.
    pub fn starting_at(kbps: u32) -> Self {
        AutoPace {
            kbps: kbps.clamp(MIN_KBPS, MAX_KBPS),
            ..AutoPace::default()
        }
    }

    /// The pacing to ask for, kbit/s.
    pub fn kbps(&self) -> u32 {
        self.kbps
    }

    /// A frame has arrived (as far as it will).
    pub fn on_frame(&mut self, f: &FrameArrival) {
        let (rate, burst_loss) = judge(f);
        if let Some(r) = rate {
            self.rates.push(r);
        }
        self.bursts += u32::from(burst_loss);
    }

    /// Once a second: a new pacing if it changed, and why.
    pub fn tick(&mut self, now: Instant) -> Option<(u32, String)> {
        let start = *self.window_start.get_or_insert(now);
        let clean_since = *self.clean_since.get_or_insert(now);
        if now.duration_since(start) < WINDOW {
            return None;
        }
        self.window_start = Some(now);
        let bursts = std::mem::take(&mut self.bursts);
        let mut rates = std::mem::take(&mut self.rates);
        if bursts > 0 {
            self.clean_since = Some(now);
        }
        let old = self.kbps;
        rates.sort_by(f64::total_cmp);
        let arrive = (rates.len() >= 5).then(|| rates[rates.len() / 2]);
        let why = match arrive {
            Some(a) if a < SLOWER * f64::from(old) * 1000.0 => {
                self.kbps = ((0.9 * a / 1000.0) as u32).clamp(MIN_KBPS, MAX_KBPS);
                format!("frames arrive at {:.0} Mbit/s", a / 1e6)
            }
            _ if bursts >= 2 => {
                self.kbps = (old * 85 / 100).max(MIN_KBPS);
                format!("{bursts} frames lost bursts of datagrams")
            }
            _ if now.duration_since(clean_since) >= CLEAN_TO_PROBE && old < MAX_KBPS => {
                self.kbps = (old * 110 / 100).min(MAX_KBPS);
                self.clean_since = Some(now);
                format!("{} s without bursts of loss", CLEAN_TO_PROBE.as_secs())
            }
            _ => return None,
        };
        (self.kbps != old).then_some((self.kbps, why))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrival::{Collector, tests::feed};

    #[test]
    fn a_reconnection_starts_where_the_last_session_was() {
        assert_eq!(AutoPace::starting_at(80_000).kbps(), 80_000);
        assert_eq!(AutoPace::starting_at(1_000).kbps(), MIN_KBPS);
        assert_eq!(AutoPace::starting_at(999_000).kbps(), MAX_KBPS);
    }

    /// One frame of `n` datagrams of 1200 bytes at `mbps`, starting at `t0_us`;
    /// datagrams in `lost` do not arrive.
    fn frame(
        p: &mut AutoPace,
        c: &mut Collector,
        number: u32,
        t0_us: u64,
        n: u16,
        mbps: f64,
        lost: &[u16],
    ) {
        for f in feed(c, number, t0_us, n, 0, mbps, lost) {
            p.on_frame(&f);
        }
    }

    /// One second of 60 frames; then the window's decision.
    fn second(
        p: &mut AutoPace,
        c: &mut Collector,
        sec: u32,
        t0: Instant,
        n: u16,
        mbps: f64,
        lost: &[u16],
    ) -> Option<(u32, String)> {
        for k in 0..60 {
            let number = sec * 60 + k;
            frame(
                p,
                c,
                number,
                u64::from(number) * 16_667,
                n,
                mbps,
                if k % 20 == 0 { lost } else { &[] },
            );
        }
        // A one-datagram frame finishes the second's last frame.
        frame(
            p,
            c,
            u32::MAX,
            u64::from((sec + 1) * 60) * 16_667,
            1,
            mbps,
            &[],
        );
        p.tick(t0 + Duration::from_secs(u64::from(sec)))
    }

    #[test]
    fn a_slower_path_sets_the_pace_below_it() {
        let mut p = AutoPace::default();
        let mut c = Collector::default();
        let t0 = Instant::now();
        assert!(p.tick(t0).is_none());
        // A home link of 100 Mbit/s: frames arrive at 100 whatever the pacing.
        let (kbps, why) = second(&mut p, &mut c, 1, t0, 80, 100.0, &[]).expect("slower");
        assert!((88_000..=91_000).contains(&kbps), "{kbps}");
        assert!(why.contains("100 Mbit/s"), "{why}");
        // Now frames arrive at the pace itself: no further change.
        assert!(second(&mut p, &mut c, 2, t0, 80, f64::from(kbps) / 1000.0, &[]).is_none());
    }

    #[test]
    fn bursts_of_loss_slow_down_and_a_clean_link_speeds_up_again() {
        let mut p = AutoPace::default();
        let mut c = Collector::default();
        let t0 = Instant::now();
        p.tick(t0);
        // Frames arrive at the pace (150), 3 of 60 lose 4 datagrams in a row.
        let (kbps, _) =
            second(&mut p, &mut c, 1, t0, 80, 150.0, &[40, 41, 42, 43]).expect("bursts");
        assert_eq!(kbps, 127_500);
        // Single losses are FEC's job: no change.
        assert!(second(&mut p, &mut c, 2, t0, 80, 127.5, &[10, 30]).is_none());
        // 10 s clean: 10 % up, then again after 10 more.
        let mut ups = Vec::new();
        for s in 3..=24 {
            let at_pace = f64::from(p.kbps()) / 1000.0;
            if let Some((k, _)) = second(&mut p, &mut c, s, t0, 80, at_pace, &[]) {
                ups.push((s, k));
            }
        }
        assert_eq!(ups, vec![(11, 140_250), (21, 150_000)]);
    }

    #[test]
    fn small_frames_and_the_floor() {
        let mut p = AutoPace::default();
        let mut c = Collector::default();
        let t0 = Instant::now();
        p.tick(t0);
        // A still screen: frames of 5 datagrams tell nothing.
        assert!(second(&mut p, &mut c, 1, t0, 5, 10.0, &[]).is_none());
        // A very slow path: not below the floor.
        let (kbps, _) = second(&mut p, &mut c, 2, t0, 80, 30.0, &[]).expect("slower");
        assert_eq!(kbps, MIN_KBPS);
    }
}
