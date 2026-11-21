//! From captured packets to Opus frames: 10 ms of stereo at 48 kHz each,
//! numbered by their place in the device's stream.
//!
//! The device's stream has no holes in theory; in practice a packet may come
//! after a gap (nothing played, the device was busy) or overlap the last one.
//! Short gaps become silence (the frames keep their place in time); a long one
//! starts the frames afresh further on (the index jumps, the client knows
//! there was nothing). Overlaps are dropped.

use crate::opus::{CHANNELS, FRAME, RATE};
use crate::resample::Resampler;

/// One packet as it came from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub frames: u32,
    /// Where its first frame is in the device's stream, frames.
    pub device_position: u64,
    /// When its first frame was recorded (performance counter, 100 ns).
    pub qpc_100ns: u64,
    /// Windows marked it silent (it is read as zeros).
    pub silent: bool,
    /// Windows says frames were lost before it.
    pub discontinuity: bool,
}

/// One 10 ms stereo frame, ready for the encoder.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Counts 10 ms from the start of the stream.
    pub index: u64,
    /// When its first sample was recorded (performance counter, 100 ns).
    pub qpc_100ns: u64,
    /// `FRAME` × `CHANNELS` samples, interleaved.
    pub pcm: Vec<f32>,
}

/// Gaps up to this many frames are filled with silence: 200 ms.
pub const MAX_FILL: u64 = RATE as u64 / 5;

/// What a [`Framer`] met in the device's stream, frames and packets.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FramerStats {
    /// Silence put in for short gaps, frames; gaps that restarted the frames.
    pub filled: u64,
    pub restarts: u64,
    /// Packets with a gap (filled) and with an overlap; frames dropped as
    /// repeats of what came before.
    pub gaps: u64,
    pub overlaps: u64,
    pub skipped: u64,
    /// Silence put in while the device gave nothing, frames.
    pub idled: u64,
}

impl std::ops::AddAssign for FramerStats {
    fn add_assign(&mut self, o: FramerStats) {
        self.filled += o.filled;
        self.restarts += o.restarts;
        self.gaps += o.gaps;
        self.overlaps += o.overlaps;
        self.skipped += o.skipped;
        self.idled += o.idled;
    }
}

pub struct Framer {
    /// Channels in what the device gives.
    channels: usize,
    /// Stereo samples of frame `index` onwards.
    buf: Vec<f32>,
    index: u64,
    /// The device's stream position just after `buf`; `None` before the first packet.
    next_pos: Option<u64>,
    /// A position in the device's stream and its time: the time of any other.
    anchor: (u64, u64),
    pub stats: FramerStats,
    /// After [`Framer::idle`]: the next packet is taken where it says it
    /// starts, as if right after.
    resync: bool,
}

impl Framer {
    /// For a device with `channels` channels (48 kHz).
    pub fn new(channels: u16) -> Framer {
        Framer {
            channels: usize::from(channels.max(1)),
            buf: Vec::with_capacity(FRAME * CHANNELS * 4),
            index: 0,
            next_pos: None,
            anchor: (0, 0),
            stats: FramerStats::default(),
            resync: false,
        }
    }

    fn qpc_at(&self, pos: u64) -> u64 {
        let (p, q) = self.anchor;
        let d = (pos as i128 - p as i128) * 10_000_000 / i128::from(RATE);
        (q as i128 + d).max(0) as u64
    }

    /// The device's samples of `p` (interleaved, its channels): to stereo,
    /// into frames; every frame that is complete goes to `out`.
    pub fn push(&mut self, p: &Packet, samples: &[f32], out: &mut Vec<Frame>) {
        let ch = self.channels;
        let frames = (samples.len() / ch).min(p.frames as usize);
        let mut skip = 0usize;
        if std::mem::take(&mut self.resync) {
            self.next_pos = Some(p.device_position);
        }
        match self.next_pos {
            None => self.next_pos = Some(p.device_position),
            Some(next) if p.device_position > next => {
                let gap = p.device_position - next;
                if gap <= MAX_FILL {
                    self.buf
                        .resize(self.buf.len() + gap as usize * CHANNELS, 0.0);
                    self.stats.filled += gap;
                    self.stats.gaps += 1;
                } else {
                    // Further on: the partial frame is dropped, the index moves
                    // past the gap.
                    let had = (self.buf.len() / CHANNELS) as u64;
                    self.index += (had + gap).div_ceil(FRAME as u64);
                    self.buf.clear();
                    self.stats.restarts += 1;
                }
            }
            Some(next) if p.device_position < next => {
                skip = ((next - p.device_position) as usize).min(frames);
                self.stats.overlaps += 1;
                self.stats.skipped += skip as u64;
            }
            Some(_) => {}
        }
        self.anchor = (p.device_position, p.qpc_100ns);
        for f in skip..frames {
            let s = &samples[f * ch..(f + 1) * ch];
            let (l, r) = if ch == 1 { (s[0], s[0]) } else { (s[0], s[1]) };
            self.buf.push(l);
            self.buf.push(r);
        }
        self.next_pos =
            Some(p.device_position.max(self.next_pos.unwrap_or(0)) + (frames - skip) as u64);
        let end = self.next_pos.expect("set above");
        self.emit(end, out);
    }

    /// Nothing comes from the device: nothing plays (loopback then gives no
    /// packets, and its stream stands still). `frames` of silence up to
    /// `qpc_end_100ns` keep the frames going at the pace of the clock; the
    /// next packet is taken where it says it starts, as if right after.
    pub fn idle(&mut self, frames: u64, qpc_end_100ns: u64, out: &mut Vec<Frame>) {
        if frames == 0 {
            return;
        }
        let end = self.next_pos.unwrap_or(0) + frames;
        self.buf
            .resize(self.buf.len() + frames as usize * CHANNELS, 0.0);
        self.anchor = (end, qpc_end_100ns);
        self.next_pos = Some(end);
        self.stats.idled += frames;
        self.resync = true;
        self.emit(end, out);
    }

    /// Every complete frame of `buf` (its stream ends at `end`) to `out`.
    fn emit(&mut self, end: u64, out: &mut Vec<Frame>) {
        while self.buf.len() >= FRAME * CHANNELS {
            let start = end - (self.buf.len() / CHANNELS) as u64;
            out.push(Frame {
                index: self.index,
                qpc_100ns: self.qpc_at(start),
                pcm: self.buf.drain(..FRAME * CHANNELS).collect(),
            });
            self.index += 1;
        }
    }
}

/// A device at any rate to [`Framer`] at 48 kHz: its packets to stereo,
/// resampled, with their stream positions at 48 kHz (a gap in the device's
/// stream is a gap here). At 48 kHz it is the framer as it is.
pub struct To48k {
    rate: u32,
    channels: usize,
    framer: Framer,
    rs: Resampler,
    /// The device's position after the last packet, and ours (48 kHz) after
    /// what the resampler gave.
    next_device: Option<u64>,
    pos: u64,
    stereo: Vec<f32>,
    out: Vec<f32>,
}

impl To48k {
    pub fn new(rate: u32, channels: u16) -> To48k {
        To48k {
            rate,
            channels: usize::from(channels.max(1)),
            framer: Framer::new(if rate == RATE { channels } else { 2 }),
            rs: Resampler::new(),
            next_device: None,
            pos: 0,
            stereo: Vec::new(),
            out: Vec::new(),
        }
    }

    pub fn framer(&self) -> &Framer {
        &self.framer
    }

    /// Silence while the device gives nothing ([`Framer::idle`]); `frames` at 48 kHz.
    pub fn idle(&mut self, frames: u64, qpc_end_100ns: u64, out: &mut Vec<Frame>) {
        if self.rate != RATE {
            // What the resampler holds (a few samples) goes; afresh after.
            self.rs = Resampler::new();
            self.next_device = None;
        }
        self.framer.idle(frames, qpc_end_100ns, out);
    }

    pub fn push(&mut self, p: &Packet, samples: &[f32], out: &mut Vec<Frame>) {
        if self.rate == RATE {
            return self.framer.push(p, samples, out);
        }
        let to48 = |frames: u64, rate: u32| frames * u64::from(RATE) / u64::from(rate);
        // A gap in the device's stream: as much silence (a long one: the
        // framer moves its index on).
        if let Some(next) = self.next_device
            && p.device_position > next
        {
            let gap = p.device_position - next;
            if to48(gap, self.rate) <= MAX_FILL {
                self.rs.push(&vec![0.0; gap as usize * CHANNELS]);
            } else {
                self.rs = Resampler::new();
                self.pos += to48(gap, self.rate);
            }
        }
        self.next_device = Some(p.device_position + u64::from(p.frames));
        let ch = self.channels;
        self.stereo.clear();
        for f in samples.chunks_exact(ch).take(p.frames as usize) {
            let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
            self.stereo.extend([l, r]);
        }
        self.rs.push(&self.stereo);
        self.out.clear();
        let step = f64::from(self.rate) / f64::from(RATE);
        let n = self.rs.pull(step, usize::MAX, &mut self.out);
        if n == 0 {
            return;
        }
        let q = Packet {
            frames: n as u32,
            device_position: self.pos,
            qpc_100ns: p.qpc_100ns,
            silent: p.silent,
            discontinuity: false,
        };
        self.pos += n as u64;
        self.framer.push(&q, &self.out, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pos: u64, frames: u32) -> Packet {
        Packet {
            frames,
            device_position: pos,
            qpc_100ns: pos * 10_000_000 / u64::from(RATE),
            silent: false,
            discontinuity: false,
        }
    }

    /// Stereo samples whose left channel is their stream position.
    fn samples(pos: u64, frames: u32) -> Vec<f32> {
        (pos..pos + u64::from(frames))
            .flat_map(|p| [p as f32, -(p as f32)])
            .collect()
    }

    fn run(packets: &[(u64, u32)]) -> (Framer, Vec<Frame>) {
        let mut f = Framer::new(2);
        let mut out = Vec::new();
        for &(pos, n) in packets {
            f.push(&packet(pos, n), &samples(pos, n), &mut out);
        }
        (f, out)
    }

    #[test]
    fn packets_of_any_size_make_10ms_frames() {
        let (_, out) = run(&[(1000, 300), (1300, 300), (1600, 400)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].index, 0);
        assert_eq!((out[0].pcm[0], out[0].pcm[1]), (1000.0, -1000.0));
        assert_eq!(out[1].index, 1);
        assert_eq!(out[1].pcm[0], 1480.0);
        // Its time is its first sample's.
        assert_eq!(out[1].qpc_100ns, 1480 * 10_000_000 / 48_000);
    }

    #[test]
    fn a_short_gap_is_silence_in_place() {
        let (f, out) = run(&[(0, 480), (480 + 100, 860)]);
        assert_eq!(f.stats.filled, 100);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].pcm[0], 0.0, "the gap is silent");
        assert_eq!(out[1].pcm[200], 580.0, "the sound after it is in its place");
        assert_eq!(out.iter().map(|f| f.index).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn a_long_gap_moves_the_index_on() {
        let gap = MAX_FILL + 1000;
        let (f, out) = run(&[(0, 480), (480 + gap, 480)]);
        assert_eq!(f.stats.restarts, 1);
        assert_eq!(out.len(), 2);
        // 10 ms frames: the second is as far on as the gap (rounded up).
        assert_eq!(out[1].index, 1 + gap.div_ceil(480));
        assert_eq!(out[1].pcm[0], (480 + gap) as f32);
    }

    #[test]
    fn an_overlap_is_dropped() {
        // 100 frames again: only the 80 new ones count, 400 + 80 = one frame.
        let (_, out) = run(&[(0, 400), (300, 180)]);
        assert_eq!(out.len(), 1);
        let left: Vec<f32> = out[0].pcm.iter().step_by(2).copied().collect();
        assert!(
            left.iter().enumerate().all(|(i, &v)| v == i as f32),
            "0..480 in order"
        );
    }

    #[test]
    fn silence_keeps_the_frames_going_while_the_device_stands_still() {
        let (mut f, mut out) = run(&[(0, 580)]);
        // 30 ms with nothing: the partial frame and two more, silent.
        f.idle(1440, 0, &mut out);
        assert_eq!(
            out.iter().map(|f| f.index).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(out[1].pcm[0], 480.0);
        assert_eq!(out[1].pcm[2 * 99], 579.0, "the partial frame's sound");
        assert_eq!(out[1].pcm[2 * 100], 0.0, "then silence");
        // The device goes on from where it stood: right after the silence.
        f.push(&packet(580, 480), &samples(580, 480), &mut out);
        assert_eq!(out.last().unwrap().index, 4);
        assert_eq!(out.last().unwrap().pcm[2 * 100], 580.0);
        let s = f.stats;
        assert_eq!((s.gaps, s.overlaps, s.restarts), (0, 0, 0));
    }

    #[test]
    fn mono_is_doubled() {
        let mut f = Framer::new(1);
        let mut out = Vec::new();
        let mono: Vec<f32> = (0..480).map(|i| i as f32).collect();
        f.push(&packet(0, 480), &mono, &mut out);
        assert_eq!(&out[0].pcm[..4], &[0.0, 0.0, 1.0, 1.0]);
    }
    #[test]
    fn a_device_at_44100_gives_whole_48k_frames() {
        let mono = crate::tone::sine(44_100, 1000.0, 0.1, 2.0);
        let mut c = To48k::new(44_100, 2);
        let mut out = Vec::new();
        // 10 ms packets of the device's.
        for (k, chunk) in mono.chunks(441).enumerate() {
            let stereo: Vec<f32> = chunk.iter().flat_map(|&s| [s, s]).collect();
            let p = packet(k as u64 * 441, chunk.len() as u32);
            c.push(&p, &stereo, &mut out);
        }
        // 2 s at 48 kHz, in order.
        assert!((198..=200).contains(&out.len()), "{}", out.len());
        assert!(out.windows(2).all(|w| w[1].index == w[0].index + 1));
        let left: Vec<f32> = out
            .iter()
            .flat_map(|f| f.pcm.iter().step_by(2).copied())
            .collect();
        let r = crate::tone::analyse(&left, RATE, 1000.0);
        assert!(r.intact(), "{r:?}");
        assert!((r.amplitude - 0.1).abs() < 0.002, "{r:?}");
    }
}
