//! Keeps the real video bitrate of a capped session close to (and not above)
//! the cap.
//!
//! NVENC's low-latency rate control with a one-frame VBV undershoots a lot: on
//! 4K60 synthetic content a 40 Mbit/s setting yields ~24 Mbit/s while the QP is
//! already well above the minimum. So the host
//! measures what actually leaves the encoder and steers the encoder's nominal
//! bitrate: up while the stream is below the cap and the cap is what limits the
//! quality (QP above the minimum), down as soon as it exceeds the cap.

use std::time::{Duration, Instant};

/// How often the nominal rate is revisited.
pub const WINDOW: Duration = Duration::from_millis(500);
/// Bounds of the nominal rate, relative to the cap. The upper one also bounds
/// a one-window overshoot when the picture suddenly gets harder.
const MIN_NOMINAL: f64 = 0.5;
const MAX_NOMINAL: f64 = 2.2;
/// Steer up while the stream is below this share of the cap.
const LOW_WATER: f64 = 0.85;
/// Aim for this share of the cap.
const TARGET: f64 = 0.92;
/// At most this much higher per window (no wild jumps on one quiet window).
const MAX_STEP_UP: f64 = 1.15;
/// Inside a window, react at once when the stream already runs this far above
/// the cap (the picture got harder while the nominal rate was high)...
const BURST: f64 = 1.1;
/// ...measured over at least this long.
const BURST_MIN: Duration = Duration::from_millis(100);

pub struct CapController {
    cap_bps: f64,
    min_qp: u32,
    nominal_bps: f64,
    bytes: u64,
    qp_sum: u64,
    frames: u32,
    window_start: Instant,
}

impl CapController {
    pub fn new(cap_bps: u32, min_qp: u32, now: Instant) -> Self {
        CapController {
            cap_bps: f64::from(cap_bps),
            min_qp,
            nominal_bps: f64::from(cap_bps),
            bytes: 0,
            qp_sum: 0,
            frames: 0,
            window_start: now,
        }
    }

    /// A new cap from now on (the host lowers it while files go out): the
    /// nominal rate scales with it. Returns the rate to set the encoder to.
    pub fn set_cap(&mut self, cap_bps: u32, now: Instant) -> u32 {
        let new = f64::from(cap_bps.max(1));
        self.nominal_bps =
            (self.nominal_bps * new / self.cap_bps).clamp(MIN_NOMINAL * new, MAX_NOMINAL * new);
        self.cap_bps = new;
        self.bytes = 0;
        self.qp_sum = 0;
        self.frames = 0;
        self.window_start = now;
        self.nominal_bps as u32
    }

    /// The rate the encoder is set to now, bit/s.
    pub fn nominal_bps(&self) -> u32 {
        self.nominal_bps as u32
    }

    pub fn on_frame(&mut self, bytes: usize, avg_qp: u32) {
        self.bytes += bytes as u64;
        self.qp_sum += u64::from(avg_qp);
        self.frames += 1;
    }

    /// At the end of a window (or early, on a burst over the cap): the new
    /// nominal rate if it should change.
    pub fn poll(&mut self, now: Instant) -> Option<u32> {
        let elapsed = now.duration_since(self.window_start);
        if self.frames == 0 || elapsed < BURST_MIN {
            return None;
        }
        let actual = self.bytes as f64 * 8.0 / elapsed.as_secs_f64();
        if elapsed < WINDOW && actual <= BURST * self.cap_bps {
            return None;
        }
        let avg_qp = self.qp_sum as f64 / f64::from(self.frames);
        self.bytes = 0;
        self.qp_sum = 0;
        self.frames = 0;
        self.window_start = now;

        let factor = if actual > self.cap_bps {
            TARGET * self.cap_bps / actual
        } else if actual < LOW_WATER * self.cap_bps && avg_qp > f64::from(self.min_qp) + 0.5 {
            (TARGET * self.cap_bps / actual.max(1.0)).min(MAX_STEP_UP)
        } else {
            return None;
        };
        let nominal = (self.nominal_bps * factor)
            .clamp(MIN_NOMINAL * self.cap_bps, MAX_NOMINAL * self.cap_bps);
        if (nominal / self.nominal_bps - 1.0).abs() < 0.02 {
            return None;
        }
        self.nominal_bps = nominal;
        Some(nominal as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake encoder that delivers `efficiency` × the nominal rate, at QP
    /// `qp_at_cap` (above the minimum) unless the content needs less than that.
    fn simulate(efficiency: f64, content_bps: f64, windows: usize) -> (CapController, Vec<f64>) {
        let t0 = Instant::now();
        let mut c = CapController::new(40_000_000, 16, t0);
        let mut actual = Vec::new();
        for w in 1..=windows {
            let rate = (c.nominal_bps() as f64 * efficiency).min(content_bps);
            let qp = if rate < content_bps { 24 } else { 16 };
            for _ in 0..30 {
                c.on_frame((rate / 8.0 / 60.0) as usize, qp);
            }
            actual.push(rate);
            c.poll(t0 + WINDOW * w as u32);
        }
        (c, actual)
    }

    #[test]
    fn steers_an_undershooting_encoder_up_to_the_cap() {
        let (_, actual) = simulate(0.6, 1e9, 20);
        let last = *actual.last().unwrap();
        assert!(last <= 40e6 && last > 0.9 * 40e6, "{actual:?}");
    }

    #[test]
    fn steers_an_overshooting_encoder_down() {
        let (_, actual) = simulate(1.4, 1e9, 20);
        assert!(actual[3..].iter().all(|&r| r <= 40e6 * 1.01), "{actual:?}");
    }

    #[test]
    fn leaves_easy_content_alone() {
        // 10 Mbit/s of content at the minimum QP: the cap is not what limits it.
        let (c, _) = simulate(0.6, 10e6, 20);
        assert_eq!(c.nominal_bps(), 40_000_000);
    }

    #[test]
    fn cuts_a_burst_before_the_window_ends() {
        let t0 = Instant::now();
        let mut c = CapController::new(40_000_000, 16, t0);
        // 2x the cap for 120 ms.
        for _ in 0..7 {
            c.on_frame((80e6 / 8.0 / 60.0) as usize, 24);
        }
        let cut = c.poll(t0 + Duration::from_millis(120)).expect("early cut");
        assert!(cut < 40_000_000 / 2 + 1_000_000, "{cut}");
    }

    #[test]
    fn nominal_stays_within_bounds() {
        let (c, _) = simulate(0.1, 1e9, 50);
        assert_eq!(c.nominal_bps(), (40e6 * MAX_NOMINAL) as u32);
    }
}
