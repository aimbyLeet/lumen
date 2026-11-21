//! Stereo resampling with a step that may change at every call: 4-point
//! cubic (Hermite) interpolation. It plays the stream at another rate (a
//! device not at 48 kHz) and a touch faster or slower (the client keeps its
//! buffer level against the drift between the host's and its own clocks).

use std::collections::VecDeque;

pub struct Resampler {
    /// Input frames; `buf[1]` is the frame at `pos` 0 (one frame of history before it).
    buf: VecDeque<[f32; 2]>,
    /// Where the next output frame is, in input frames from `buf[1]`.
    pos: f64,
    /// Input frames consumed so far (dropped from `buf`).
    consumed: u64,
}

impl Default for Resampler {
    fn default() -> Self {
        Resampler::new()
    }
}

impl Resampler {
    pub fn new() -> Resampler {
        let mut buf = VecDeque::with_capacity(4096);
        buf.push_back([0.0; 2]);
        Resampler {
            buf,
            pos: 0.0,
            consumed: 0,
        }
    }

    /// Appends stereo input (interleaved).
    pub fn push(&mut self, stereo: &[f32]) {
        self.buf.extend(stereo.as_chunks::<2>().0.iter().copied());
    }

    /// Input frames not played yet (from the next output on).
    pub fn buffered(&self) -> f64 {
        (self.buf.len() as f64 - 1.0 - self.pos).max(0.0)
    }

    /// The input frame (counted from the start) the next output frame is at.
    pub fn position(&self) -> f64 {
        self.consumed as f64 + self.pos
    }

    /// Output frames that can be made now at `step` (input frames per output frame).
    pub fn available(&self, step: f64) -> usize {
        // At position p: `buf[floor(p)..=floor(p) + 3]`, so p < len - 3.
        let limit = self.buf.len() as f64 - 3.0;
        if self.pos >= limit {
            0
        } else {
            ((limit - self.pos) / step).ceil() as usize
        }
    }

    /// Up to `max` output frames at `step` (interleaved stereo) to `out`;
    /// returns how many.
    pub fn pull(&mut self, step: f64, max: usize, out: &mut Vec<f32>) -> usize {
        let n = self.available(step).min(max);
        for _ in 0..n {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (x0, x1, x2, x3) = (
                self.buf[i],
                self.buf[i + 1],
                self.buf[i + 2],
                self.buf[i + 3],
            );
            for c in 0..2 {
                let (a, b, cc, d) = (x0[c], x1[c], x2[c], x3[c]);
                let c1 = 0.5 * (cc - a);
                let c2 = a - 2.5 * b + 2.0 * cc - 0.5 * d;
                let c3 = 0.5 * (d - a) + 1.5 * (b - cc);
                out.push(((c3 * t + c2) * t + c1) * t + b);
            }
            self.pos += step;
        }
        // Keep one frame of history before the next position.
        let drop = (self.pos.floor() as usize).min(self.buf.len().saturating_sub(1));
        self.buf.drain(..drop);
        self.pos -= drop as f64;
        self.consumed += drop as u64;
        n
    }

    /// Drops `frames` input frames from the front (the stream jumps ahead).
    pub fn skip(&mut self, frames: usize) {
        let n = frames.min(self.buf.len().saturating_sub(2));
        self.buf.drain(1..1 + n);
        self.consumed += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tone;

    fn stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|&s| [s, s]).collect()
    }

    #[test]
    fn step_one_is_the_input() {
        let mut r = Resampler::new();
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        r.push(&stereo(&input));
        let mut out = Vec::new();
        // The last two frames wait for the ones after them.
        let n = r.pull(1.0, 1000, &mut out);
        assert_eq!(n, 98);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(
            left.iter().enumerate().all(|(i, &v)| v == i as f32),
            "{left:?}"
        );
    }

    #[test]
    fn pieces_of_any_size_give_the_same_tone() {
        // A tone pushed and pulled in odd pieces stays whole.
        let mono = tone::sine(48_000, 1000.0, 0.1, 1.0);
        let mut r = Resampler::new();
        let mut out = Vec::new();
        for piece in stereo(&mono).chunks(2 * 37) {
            r.push(piece);
            r.pull(1.0, 53, &mut out);
        }
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let rep = tone::analyse(&left, 48_000, 1000.0);
        assert!(rep.intact(), "{rep:?}");
    }

    #[test]
    fn to_44100_keeps_the_frequency() {
        let mono = tone::sine(48_000, 1000.0, 0.1, 1.0);
        let mut r = Resampler::new();
        r.push(&stereo(&mono));
        let mut out = Vec::new();
        r.pull(48_000.0 / 44_100.0, usize::MAX, &mut out);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!((left.len() as f64 - 44_100.0).abs() < 5.0, "{}", left.len());
        // The same 1 kHz at the new rate.
        let rep = tone::analyse(&left, 44_100, 1000.0);
        assert!(rep.intact(), "{rep:?}");
        assert!((rep.amplitude - 0.1).abs() < 0.002, "{rep:?}");
    }

    #[test]
    fn buffered_counts_what_is_left() {
        let mut r = Resampler::new();
        r.push(&stereo(&[0.0; 480]));
        assert_eq!(r.buffered(), 480.0);
        let mut out = Vec::new();
        r.pull(1.0, 100, &mut out);
        assert_eq!(r.buffered(), 380.0);
        assert_eq!(r.position(), 100.0);
        r.skip(200);
        assert_eq!(r.buffered(), 180.0);
    }
}
