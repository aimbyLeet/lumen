//! The client's side of the sound: Opus frames in by their index, as they
//! come; sound out at the device's pace.
//!
//! - A frame lost on the way usually comes with the next datagram (each
//!   carries the frame before it); if both are lost, the decoder conceals it.
//! - Nothing plays until [`TARGET_MS`] is buffered (here and in the device);
//!   when the buffer runs dry, it is filled up again before playing on.
//! - The host's and this device's clocks differ (tens of ppm): the level
//!   drifts. Within [`DEADBAND_MS`] of the target the speed is left as it is
//!   (a clean stream plays untouched: at speed 1 the resampler passes the
//!   samples through as they are). Outside it, a PI control on how far
//!   outside (critically damped, ~10 s) brings the speed to the drift
//!   without overshooting it, and the speed stays there once the level is
//!   back inside. At most [`MAX_ADJUST`]: no audible pitch change. After a
//!   stall the backlog is dropped at once.
//! - A frame missing when it is due while later ones are here is concealed
//!   (up to [`MAX_CONCEAL`] in a row). Nothing here at all is a dry buffer,
//!   not a loss: concealing then would run ahead of the host (every frame
//!   after would come too late).

use std::collections::{BTreeMap, VecDeque};

use crate::opus::{CHANNELS, Decoder, FRAME, OpusError, RATE};
use crate::resample::Resampler;

/// The level kept: here plus in the device, ms.
pub const TARGET_MS: f64 = 50.0;
/// Above the target by this much: the backlog of a stall, dropped to the target.
pub const EXCESS_MS: f64 = 80.0;
/// The level may stray this far from the target with the speed left alone.
pub const DEADBAND_MS: f64 = 10.0;
/// Speed change per ms outside the dead band, and per ms·s of it: critically
/// damped (the level moves 1 ms/s per 1000 ppm: 1000·KP = 2·√(1000·KI)).
/// The bound: 0.5 % (8.6 cents).
const KP: f64 = 0.000_28;
const KI: f64 = 0.000_02;
pub const MAX_ADJUST: f64 = 0.005;
/// The level is smoothed over about this long before it steers the speed.
const SMOOTH_S: f64 = 0.5;
/// Frames missing in a row up to this are concealed; a longer gap is the
/// host sending nothing (silence): playing goes on from the next frame.
const MAX_CONCEAL: u64 = 3;
/// A frame this far behind the one playing: a new stream.
const NEW_STREAM: u64 = 500;
const FRAME_US: u64 = 10_000;

#[derive(Debug, Default, Clone)]
pub struct PlayoutStats {
    pub decoded: u64,
    /// Frames played from the copy in the next datagram.
    pub repeats_used: u64,
    /// Frames lost both times: concealed by the decoder.
    pub concealed: u64,
    /// Frames that came after their time.
    pub late: u64,
    /// The device ran out of sound (each an audible gap): nothing more here,
    /// and the device emptied while there was (the player was late to feed it).
    pub underruns: u64,
    pub device_dry: u64,
    /// Ran out where the host's sound stopped (silence came next): the host
    /// takes a moment to tell a pause, nothing was cut. Not in `underruns`.
    pub stops: u64,
    /// Sound dropped after stalls, ms.
    pub dropped_ms: f64,
    pub resets: u64,
    /// The smoothed level, ms, and the speed change, ppm, while playing: the
    /// recent ones ([`HISTORY`]), not the whole session's.
    pub level_ms: Vec<f32>,
    pub adjust_ppm: Vec<f32>,
}

/// How much of the level and speed history is kept for the closing report:
/// the last `HISTORY`..`2 * HISTORY` values (a session of days would keep
/// them all otherwise).
const HISTORY: usize = 360_000;

/// Drops the older values once there are twice [`HISTORY`]; the order stays.
fn keep_recent(v: &mut Vec<f32>) {
    if v.len() >= 2 * HISTORY {
        v.drain(..v.len() - HISTORY);
    }
}

impl PlayoutStats {
    /// Audible gaps: nothing here, or the device emptied.
    pub fn gaps(&self) -> u64 {
        self.underruns + self.device_dry
    }

    /// Adds another playout's (an earlier device's).
    pub fn merge(&mut self, s: PlayoutStats) {
        self.decoded += s.decoded;
        self.repeats_used += s.repeats_used;
        self.concealed += s.concealed;
        self.late += s.late;
        self.underruns += s.underruns;
        self.device_dry += s.device_dry;
        self.stops += s.stops;
        self.dropped_ms += s.dropped_ms;
        self.resets += s.resets;
        self.level_ms.extend(s.level_ms);
        self.adjust_ppm.extend(s.adjust_ppm);
        keep_recent(&mut self.level_ms);
        keep_recent(&mut self.adjust_ppm);
    }
}

pub struct Playout {
    dec: Decoder,
    /// Waiting frames by index: Opus packet, its host time, came as a repeat.
    waiting: BTreeMap<u64, (Vec<u8>, u64, bool)>,
    next: Option<u64>,
    rs: Resampler,
    /// Host times of the frames in the resampler: (input position of the
    /// first sample, host µs).
    times: VecDeque<(u64, u64)>,
    pushed: u64,
    last_time: u64,
    playing: bool,
    level: Option<f64>,
    /// The speed change now (1 + it), its integral part, and frames
    /// concealed in a row.
    adjust: f64,
    integral: f64,
    concealed_run: u64,
    /// Ran dry: was it a gap, or where the host's sound stopped? The next
    /// frames tell (the first may hold the sound's tail; silence after it:
    /// a stop). Frames left to look at.
    dry_pending: u8,
    /// Input frames per device frame at speed 1 (with `sim_ppm`).
    base_step: f64,
    device_rate: u32,
    pub stats: PlayoutStats,
}

impl Playout {
    /// For a device at `device_rate`; `sim_ppm`: emulate that device's clock
    /// being off by this much (checks).
    pub fn new(device_rate: u32, sim_ppm: f64) -> Result<Playout, OpusError> {
        Ok(Playout {
            dec: Decoder::new()?,
            waiting: BTreeMap::new(),
            next: None,
            rs: Resampler::new(),
            times: VecDeque::new(),
            pushed: 0,
            last_time: 0,
            playing: false,
            level: None,
            adjust: 0.0,
            integral: 0.0,
            concealed_run: 0,
            dry_pending: 0,
            base_step: f64::from(RATE) / f64::from(device_rate) * (1.0 + sim_ppm * 1e-6),
            device_rate,
            stats: PlayoutStats::default(),
        })
    }

    /// A new stream (another session): everything waiting is dropped.
    pub fn reset(&mut self) -> Result<(), OpusError> {
        self.dec = Decoder::new()?;
        self.waiting.clear();
        self.next = None;
        self.rs = Resampler::new();
        self.times.clear();
        self.pushed = 0;
        self.playing = false;
        self.level = None;
        self.adjust = 0.0;
        self.integral = 0.0;
        self.concealed_run = 0;
        self.dry_pending = 0;
        self.stats.resets += 1;
        Ok(())
    }

    fn fresh(&self, index: u64) -> bool {
        self.next.is_none_or(|n| index >= n)
    }

    /// A datagram's frames: `index` with its host time, and the one before.
    pub fn on_packet(
        &mut self,
        index: u64,
        host_us: u64,
        opus: Vec<u8>,
        previous: Option<Vec<u8>>,
    ) {
        if self.next.is_some_and(|n| index + NEW_STREAM < n) {
            let _ = self.reset();
        }
        if self.fresh(index) {
            self.waiting.entry(index).or_insert((opus, host_us, false));
        } else {
            self.stats.late += 1;
        }
        if let Some(p) = previous
            && index > 0
            && self.fresh(index - 1)
        {
            self.waiting
                .entry(index - 1)
                .or_insert((p, host_us.saturating_sub(FRAME_US), true));
        }
    }

    /// Sound here and in the device (`padding` device frames), ms.
    fn level_ms(&self, padding: usize) -> f64 {
        let here = self.rs.buffered() + self.waiting.len() as f64 * FRAME as f64;
        here * 1000.0 / f64::from(RATE) + padding as f64 * 1000.0 / f64::from(self.device_rate)
    }

    /// The host time of the next sample to play (it plays after the device's
    /// `padding`): the client's lag is measured from it.
    pub fn next_host_us(&self) -> Option<u64> {
        let pos = self.rs.position();
        self.times
            .iter()
            .rev()
            .find(|(start, _)| (*start as f64) <= pos)
            .map(|&(start, t)| t + ((pos - start as f64) * 1e6 / f64::from(RATE)) as u64)
    }

    /// Up to `want` device frames of sound (interleaved stereo) to `out`, for
    /// a device with `padding` frames still to play, `dt_s` after the last
    /// call. Returns how many; fewer than `want`: nothing more to play now.
    pub fn fill(&mut self, want: usize, padding: usize, dt_s: f64, out: &mut Vec<f32>) -> usize {
        let level = self.level_ms(padding);
        if !self.playing {
            if level < TARGET_MS {
                return 0;
            }
            self.playing = true;
            self.level = Some(TARGET_MS);
        }
        if level > TARGET_MS + EXCESS_MS {
            self.drop_ms(level - TARGET_MS);
        }
        let level = self.level_ms(padding);
        let a = dt_s / (SMOOTH_S + dt_s);
        let smooth = self.level.map_or(level, |l| l + a * (level - l));
        self.level = Some(smooth);
        let err = smooth - TARGET_MS;
        let outside = err.signum() * (err.abs() - DEADBAND_MS).max(0.0);
        self.integral = (self.integral + KI * outside * dt_s).clamp(-MAX_ADJUST, MAX_ADJUST);
        self.adjust = (KP * outside + self.integral).clamp(-MAX_ADJUST, MAX_ADJUST);
        keep_recent(&mut self.stats.level_ms);
        keep_recent(&mut self.stats.adjust_ppm);
        self.stats.level_ms.push(smooth as f32);
        self.stats.adjust_ppm.push((self.adjust * 1e6) as f32);
        let step = self.base_step * (1.0 + self.adjust);
        while self.rs.buffered() < want as f64 * step + 4.0 {
            if !self.decode_next() {
                break;
            }
        }
        let n = self.rs.pull(step, want, out);
        let pos = self.rs.position() as u64;
        while self.times.len() > 1 && self.times[1].0 <= pos {
            self.times.pop_front();
        }
        if n < want {
            // Dry: fill up to the target again before playing on. The few
            // samples the resampler holds go, with their times: after a
            // pause they would be the next sample's time.
            self.stats.underruns += 1;
            self.dry_pending = 2;
            self.playing = false;
            self.rs = Resampler::new();
            self.times.clear();
            self.pushed = 0;
        }
        n
    }

    /// The device played `ms` of silence (it ran dry while sound was here: the
    /// player was not woken in time). As much of what is here goes, so the
    /// sound stays as late after the host as before.
    pub fn device_ran_dry(&mut self, ms: f64) {
        if self.playing {
            self.stats.device_dry += 1;
            self.drop_ms(ms);
        }
    }

    /// The oldest `ms` of what is buffered here.
    fn drop_ms(&mut self, ms: f64) {
        let mut frames = ms * f64::from(RATE) / 1000.0;
        let from_rs = frames.min((self.rs.buffered() - 4.0).max(0.0));
        self.rs.skip(from_rs as usize);
        frames -= from_rs;
        while frames >= FRAME as f64 {
            let Some((&i, _)) = self.waiting.iter().next() else {
                break;
            };
            self.waiting.remove(&i);
            self.next = Some(i + 1);
            frames -= FRAME as f64;
        }
        self.stats.dropped_ms += ms;
    }

    /// Decodes the next frame into the resampler; false: nothing to decode.
    fn decode_next(&mut self) -> bool {
        let first = self.waiting.keys().next().copied();
        let Some(n) = self.next.or(first) else {
            return false;
        };
        let mut pcm = [0f32; FRAME * CHANNELS];
        let host_us = match self.waiting.remove(&n) {
            Some((packet, host_us, repeat)) => {
                if self.dec.decode(Some(&packet), &mut pcm).is_err() {
                    let _ = self.dec.decode(None, &mut pcm);
                }
                self.stats.repeats_used += u64::from(repeat);
                self.concealed_run = 0;
                if self.dry_pending > 0 {
                    self.dry_pending -= 1;
                    if pcm.iter().all(|s| s.abs() < 1e-4) {
                        self.dry_pending = 0;
                        self.stats.underruns -= 1;
                        self.stats.stops += 1;
                    }
                }
                host_us
            }
            // Nothing here: dry, not lost.
            None if first.is_none() => return false,
            // Due now, later ones here: lost both times, concealed.
            None if first.is_some_and(|f| f - n <= MAX_CONCEAL) => {
                if self.concealed_run >= MAX_CONCEAL {
                    return false;
                }
                let _ = self.dec.decode(None, &mut pcm);
                self.stats.concealed += 1;
                self.concealed_run += 1;
                self.last_time + FRAME_US
            }
            None => {
                // The host had nothing to send: on from the next frame.
                self.next = first;
                return true;
            }
        };
        self.stats.decoded += 1;
        self.rs.push(&pcm);
        self.times.push_back((self.pushed, host_us));
        self.pushed += FRAME as u64;
        self.last_time = host_us;
        self.next = Some(n + 1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_is_bounded() {
        let mut v: Vec<f32> = (0..2 * HISTORY - 1).map(|i| i as f32).collect();
        keep_recent(&mut v);
        assert_eq!(v.len(), 2 * HISTORY - 1);
        v.push(0.0);
        keep_recent(&mut v);
        // The newer half stays, in order.
        assert_eq!(v.len(), HISTORY);
        assert_eq!(v[0], HISTORY as f32);
        assert_eq!(v[HISTORY - 1], 0.0);
    }
    use crate::opus::Encoder;
    use crate::tone;

    /// A tiny reproducible random source.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    struct Sim {
        /// The device runs this much faster than the host (ppm).
        drift_ppm: f64,
        loss: f64,
        /// Datagrams sent in this window (s) arrive at its end.
        stall: Option<(f64, f64)>,
        seconds: f64,
        /// Level and speed are looked at from here on (settled).
        settle: f64,
    }

    struct Outcome {
        stats: PlayoutStats,
        played: Vec<f32>,
        /// Smoothed level and speed change once settled (after 5 s, outside the stall).
        level: (f32, f32),
        adjust: (f32, f32),
    }

    /// Host: a 1 kHz tone, a datagram every 10 ms (its clock); network: up
    /// to 4 ms of jitter, losses, a stall; device: 48 kHz at its own clock,
    /// filled every 5 ms to 10 ms ahead.
    fn simulate(s: Sim) -> Outcome {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut enc = Encoder::new(128_000).unwrap();
        let mono = tone::sine(RATE, 1000.0, 0.1, s.seconds + 1.0);
        let mut p = Playout::new(RATE, 0.0).unwrap();
        // Arrival time, frame index, its packet, the previous frame's.
        type Datagram = (f64, u64, Vec<u8>, Option<Vec<u8>>);
        let mut in_flight: Vec<Datagram> = Vec::new();
        let mut prev: Option<Vec<u8>> = None;
        let mut played = Vec::new();
        let (mut level, mut adjust) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
        let tick = 0.005;
        let device_rate = f64::from(RATE) * (1.0 + s.drift_ppm * 1e-6);
        let (mut t, mut sent, mut padding) = (0.0f64, 0u64, 0.0f64);
        let mut buf = [0u8; 1275];
        while t < s.seconds {
            // The host's frames due by now.
            while (sent + 1) as f64 * 0.010 <= t {
                let i = sent as usize * FRAME;
                let stereo: Vec<f32> = mono[i..i + FRAME].iter().flat_map(|&x| [x, x]).collect();
                let len = enc.encode(&stereo, &mut buf).unwrap();
                let opus = buf[..len].to_vec();
                let mut arrive = t + rng.next() * 0.004;
                if let Some((a, b)) = s.stall
                    && (a..b).contains(&t)
                {
                    arrive = b;
                }
                if rng.next() >= s.loss {
                    in_flight.push((arrive, sent, opus.clone(), prev.clone()));
                }
                prev = Some(opus);
                sent += 1;
            }
            in_flight.sort_by(|a, b| a.0.total_cmp(&b.0));
            while in_flight.first().is_some_and(|d| d.0 <= t) {
                let (_, i, o, pr) = in_flight.remove(0);
                p.on_packet(i, i * FRAME_US, o, pr);
            }
            // The device played `tick` worth since the last look.
            padding = (padding - tick * device_rate).max(0.0);
            let want = (0.010 * f64::from(RATE) - padding).max(0.0) as usize;
            let n = p.fill(want, padding as usize, tick, &mut played);
            padding += n as f64;
            let in_stall = s
                .stall
                .is_some_and(|(a, b)| (a - 0.1..b + 1.5).contains(&t));
            if t > s.settle && !in_stall {
                let (l, a) = (
                    p.level.unwrap_or(0.0) as f32,
                    *p.stats.adjust_ppm.last().unwrap(),
                );
                level = (level.0.min(l), level.1.max(l));
                adjust = (adjust.0.min(a), adjust.1.max(a));
            }
            t += tick;
        }
        Outcome {
            stats: p.stats,
            played,
            level,
            adjust,
        }
    }

    #[test]
    fn a_clean_stream_plays_whole() {
        let o = simulate(Sim {
            drift_ppm: 0.0,
            loss: 0.0,
            stall: None,
            seconds: 30.0,
            settle: 15.0,
        });
        assert_eq!(o.stats.underruns, 0, "{:?}", o.stats.underruns);
        assert_eq!(o.stats.concealed, 0);
        let left: Vec<f32> = o.played.iter().step_by(2).copied().collect();
        let r = tone::analyse(&left, RATE, 1000.0);
        assert!(r.dropouts == 0 && r.phase_jumps == 0, "{r:?}");
        eprintln!("clean: level {:?} ms, adjust {:?} ppm", o.level, o.adjust);
        let t = TARGET_MS as f32;
        assert!(
            o.level.0 > t - 10.0 && o.level.1 < t + 10.0,
            "level {:?}",
            o.level
        );
        assert_eq!(o.adjust, (0.0, 0.0), "a clean stream plays untouched");
    }

    /// 90 s at 1000 ppm move 90 ms: as much as 15 minutes at 100 ppm, or 15
    /// hours at 1.7. The level stays put, the speed follows.
    #[test]
    fn the_level_holds_against_drift() {
        for drift in [1000.0, -1000.0] {
            let o = simulate(Sim {
                drift_ppm: drift,
                loss: 0.0,
                stall: None,
                seconds: 90.0,
                settle: 60.0,
            });
            assert_eq!(o.stats.underruns, 0, "drift {drift}");
            assert_eq!(o.stats.dropped_ms, 0.0, "drift {drift}");
            eprintln!(
                "drift {drift} ppm: level {:?} ms, adjust {:?} ppm",
                o.level, o.adjust
            );
            let t = TARGET_MS as f32;
            assert!(
                o.level.0 > t - 20.0 && o.level.1 < t + 20.0,
                "drift {drift}: level {:?}",
                o.level
            );
            // The device consumes faster: the stream is played slower, and the other way.
            let want = (-drift) as f32;
            assert!(
                (o.adjust.0 - want).abs() < 600.0 && (o.adjust.1 - want).abs() < 600.0,
                "drift {drift}: adjust {:?}",
                o.adjust
            );
        }
    }

    #[test]
    fn losses_are_covered_by_the_repeats() {
        let o = simulate(Sim {
            drift_ppm: 0.0,
            loss: 0.05,
            stall: None,
            seconds: 30.0,
            settle: 15.0,
        });
        eprintln!(
            "5 % loss: repeats used {}, concealed {}, late {}",
            o.stats.repeats_used, o.stats.concealed, o.stats.late
        );
        // Two in a row lost (0.25 % of frames at 5 %): the next one's copy
        // comes 20 ms after the first was due — once in a while too late.
        assert!(o.stats.underruns <= 1, "{}", o.stats.underruns);
        assert!(o.stats.repeats_used > 100, "{}", o.stats.repeats_used);
        // Both lost: 0.25 % of 3000 frames, give or take.
        assert!(o.stats.concealed < 25, "{}", o.stats.concealed);
    }

    #[test]
    fn a_stall_is_a_gap_and_its_backlog_is_dropped() {
        let o = simulate(Sim {
            drift_ppm: 0.0,
            loss: 0.0,
            stall: Some((10.0, 10.3)),
            seconds: 20.0,
            settle: 5.0,
        });
        assert_eq!(o.stats.underruns, 1, "the stall itself");
        eprintln!(
            "stall 300 ms: underruns {}, dropped {:.0} ms, level after {:?} ms",
            o.stats.underruns, o.stats.dropped_ms, o.level
        );
        assert!(o.stats.dropped_ms > 200.0, "{}", o.stats.dropped_ms);
        // Back near the target afterwards.
        let t = TARGET_MS as f32;
        assert!(
            o.level.0 > t - 20.0 && o.level.1 < t + 20.0,
            "level {:?}",
            o.level
        );
    }
}
