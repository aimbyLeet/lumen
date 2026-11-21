//! A test tone and its check: is a sine of a known frequency all there, with
//! no samples lost or added along the way?
//!
//! The signal is cut into 10 ms blocks; in each, the tone's amplitude and
//! phase are measured against one clock for the whole signal. A continuous
//! sine keeps its phase from block to block; samples dropped or inserted turn
//! it (by 2π·f·k / rate for k samples), a gap of silence shows as a block
//! without the tone between blocks with it.

use std::f64::consts::TAU;

/// A sine at `freq`, `amplitude`, `seconds` long, at `rate`.
pub fn sine(rate: u32, freq: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
    let n = (f64::from(rate) * seconds).round() as usize;
    (0..n)
        .map(|i| amplitude * (TAU * freq * i as f64 / f64::from(rate)).sin() as f32)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToneReport {
    /// 10 ms blocks in the signal, and those with the tone.
    pub blocks: usize,
    pub tone_blocks: usize,
    /// Where the tone starts and ends, s.
    pub start_s: f64,
    pub end_s: f64,
    /// Its amplitude (median of the blocks with it).
    pub amplitude: f32,
    /// Blocks without the tone between the first and the last with it.
    pub dropouts: usize,
    /// Neighbouring blocks with the tone whose phases differ by more than
    /// [`PHASE_TOLERANCE`]: samples lost or added there.
    pub phase_jumps: usize,
    /// The largest phase step between neighbouring blocks, radians.
    pub max_phase_step: f64,
}

impl ToneReport {
    /// The tone came through whole.
    pub fn intact(&self) -> bool {
        self.tone_blocks > 0 && self.dropouts == 0 && self.phase_jumps == 0
    }
}

impl std::fmt::Display for ToneReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.2}..{:.2} s, amplitude {:.3}, dropouts {}, phase jumps {}, max phase step {:.3} rad)",
            if self.intact() { "INTACT" } else { "BROKEN" },
            self.start_s,
            self.end_s,
            self.amplitude,
            self.dropouts,
            self.phase_jumps,
            self.max_phase_step
        )
    }
}

/// Phase steps up to this are rounding, not lost samples (one sample at
/// 1 kHz / 48 kHz turns the phase by 0.13 rad).
pub const PHASE_TOLERANCE: f64 = 0.05;

/// Checks a tone at `freq` in `samples` (mono) at `rate`. The first and last
/// blocks with the tone are left out (the tone starts and ends inside them).
pub fn analyse(samples: &[f32], rate: u32, freq: f64) -> ToneReport {
    let block = (rate / 100) as usize;
    let blocks = samples.len() / block;
    let w = TAU * freq / f64::from(rate);
    let measured: Vec<(f64, f64)> = (0..blocks)
        .map(|b| {
            let (mut i, mut q) = (0.0, 0.0);
            for (k, &s) in samples[b * block..(b + 1) * block].iter().enumerate() {
                let (n, x) = (b * block + k, f64::from(s));
                i += x * (w * n as f64).sin();
                q += x * (w * n as f64).cos();
            }
            let amp = 2.0 * (i * i + q * q).sqrt() / block as f64;
            (amp, q.atan2(i))
        })
        .collect();
    let peak = measured.iter().map(|m| m.0).fold(0.0, f64::max);
    let has = |m: &(f64, f64)| peak > 1e-4 && m.0 > peak * 0.5;
    let with: Vec<usize> = (0..blocks).filter(|&b| has(&measured[b])).collect();
    let (Some(&first), Some(&last)) = (with.first(), with.last()) else {
        return ToneReport {
            blocks,
            tone_blocks: 0,
            start_s: 0.0,
            end_s: 0.0,
            amplitude: 0.0,
            dropouts: 0,
            phase_jumps: 0,
            max_phase_step: 0.0,
        };
    };
    let inner = (first + 1)..last;
    let dropouts = inner.clone().filter(|&b| !has(&measured[b])).count();
    let mut phase_jumps = 0;
    let mut max_phase_step: f64 = 0.0;
    for b in inner.clone().skip(1) {
        if has(&measured[b]) && has(&measured[b - 1]) {
            let d = (measured[b].1 - measured[b - 1].1 + TAU * 1.5).rem_euclid(TAU) - TAU / 2.0;
            max_phase_step = max_phase_step.max(d.abs());
            if d.abs() > PHASE_TOLERANCE {
                phase_jumps += 1;
            }
        }
    }
    let mut amps: Vec<f64> = with.iter().map(|&b| measured[b].0).collect();
    amps.sort_by(f64::total_cmp);
    let block_s = block as f64 / f64::from(rate);
    ToneReport {
        blocks,
        tone_blocks: with.len(),
        start_s: first as f64 * block_s,
        end_s: (last + 1) as f64 * block_s,
        amplitude: amps[amps.len() / 2] as f32,
        dropouts,
        phase_jumps,
        max_phase_step,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence, the tone, silence.
    fn padded(tone: Vec<f32>) -> Vec<f32> {
        let mut v = vec![0.0; 48_000 / 2];
        v.extend(tone);
        v.extend(vec![0.0; 48_000 / 2]);
        v
    }

    #[test]
    fn a_whole_tone_is_intact() {
        let r = analyse(&padded(sine(48_000, 1000.0, 0.1, 2.0)), 48_000, 1000.0);
        assert!(r.intact(), "{r:?}");
        assert!((r.amplitude - 0.1).abs() < 0.002, "{r:?}");
        assert!(
            (r.start_s - 0.5).abs() <= 0.01 && (r.end_s - 2.5).abs() <= 0.01,
            "{r:?}"
        );
        assert!(r.max_phase_step < 1e-6, "{r:?}");
    }

    #[test]
    fn a_lost_sample_turns_the_phase() {
        let mut t = sine(48_000, 1000.0, 0.1, 2.0);
        t.remove(48_000 + 123);
        let r = analyse(&padded(t), 48_000, 1000.0);
        assert_eq!(r.phase_jumps, 1, "{r:?}");
        assert_eq!(r.dropouts, 0, "{r:?}");
        assert!(!r.intact());
    }

    #[test]
    fn a_gap_is_a_dropout() {
        let mut t = sine(48_000, 1000.0, 0.1, 2.0);
        // 30 ms of silence in the middle (the tone goes on after it in phase).
        t[48_000..48_000 + 1440].fill(0.0);
        let r = analyse(&padded(t), 48_000, 1000.0);
        assert!(r.dropouts >= 2, "{r:?}");
        assert_eq!(r.phase_jumps, 0, "{r:?}");
    }

    #[test]
    fn silence_has_no_tone() {
        let r = analyse(&vec![0.0; 48_000], 48_000, 1000.0);
        assert_eq!(r.tone_blocks, 0);
        assert!(!r.intact());
    }
}
