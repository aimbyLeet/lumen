//! The session's sound: a thread that captures what this
//! machine plays (or a test tone), cuts it into 10 ms frames, encodes them
//! with Opus and sends each as an `Audio` datagram with the frame before it,
//! ahead of the video queue.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use audio::framer::{Frame, FramerStats, To48k};
use audio::opus::{self, Encoder};
use audio::{Loopback, Packet};
use protocol::AudioPacket;
use transport::{NowSender, SeqCounter, wall_clock_us};

use crate::control::{SharedSealer, seal};

/// Where the session's sound comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AudioArg {
    /// What this machine plays; nothing for a client on this machine (it
    /// would hear itself, around and around).
    Auto,
    Loopback,
    /// A 1 kHz test tone, 0.1 of full scale (checks).
    Tone,
    Off,
}

/// Opus bitrate, bit/s.
const BITRATE: i32 = 128_000;
/// How often the capture is looked at.
const POLL: Duration = Duration::from_millis(5);
/// After the capture fails (no device, the device went away): try again in.
const RETRY: Duration = Duration::from_secs(1);
/// How often the default device is checked (it may change: headphones).
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// Nothing from the device for this long (it gives a packet every 10 ms while
/// anything plays): nothing plays, loopback gives no packets. From then on,
/// till a packet comes, silence is sent at every look, at the pace of the
/// clock: the client's stream goes on, its buffer does not run dry. 100 ns.
const IDLE_AFTER_100NS: u64 = 200_000;

/// What the thread did, for the log at the end of the session.
#[derive(Default, Debug)]
pub struct AudioStats {
    pub frames: u64,
    pub bytes: u64,
    /// The frame's time (the device's stamp) → sent, ms (most ahead,
    /// median, worst). Loopback stamps a packet with when it will sound from
    /// this machine's device: ~8 ms after it can be read here (measured),
    /// 5…35+ ms on the problem host's USB output (2026-11-12); negative is
    /// sent before it sounds here. The time sent is never ahead of sending.
    pub latency_min_ms: f64,
    pub latency_p50_ms: f64,
    pub latency_max_ms: f64,
    /// The device's stream: gaps, overlaps, silence while it gave nothing
    /// (at 48 kHz).
    pub framer: FramerStats,
    pub reopened: u32,
    /// Sound dropped as late for the silence put in (this thread was not
    /// woken in time), ms.
    pub late_ms: u64,
}

pub struct AudioThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<AudioStats>>,
}

impl AudioThread {
    pub fn stop(mut self) -> Option<AudioStats> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take()?.join().ok()
    }
}

impl Drop for AudioThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Where datagrams go: the session's id, numbers and key, and its sender.
pub struct Out {
    pub session: u32,
    pub seq: SeqCounter,
    pub sealer: SharedSealer,
    pub sender: NowSender,
}

/// Starts the session's sound: `Loopback` or `Tone` (`Auto` and `Off` are
/// decided by the caller); the tone as from a device at `tone_rate`.
pub fn start(
    source: AudioArg,
    out: Out,
    dump: Option<&Path>,
    tone_rate: u32,
    tone_gaps: bool,
) -> Result<AudioThread> {
    let dump = match dump {
        Some(p) => Some(crate::files::create_log(p)?),
        None => None,
    };
    let encoder = Encoder::new(BITRATE)?;
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let thread = thread::Builder::new().name("audio".into()).spawn(move || {
        audio::audio_thread_priority();
        let mut t = Streamer {
            out,
            encoder,
            base: 0,
            next_index: None,
            previous: None,
            dump,
            stats: AudioStats::default(),
            latency: Vec::new(),
        };
        match source {
            AudioArg::Tone => t.tone(&s, tone_rate, tone_gaps),
            _ => t.loopback(&s),
        }
        t.finish()
    })?;
    Ok(AudioThread {
        stop,
        thread: Some(thread),
    })
}

struct Streamer {
    out: Out,
    encoder: Encoder,
    /// Added to the framer's frame index: the frames go on numbered after the
    /// device is opened again (the client would take a restart for a new
    /// stream); and the index after the last frame sent, and when.
    base: u64,
    next_index: Option<(u64, Instant)>,
    /// The last frame sent (its index and Opus packet): it goes again with the next.
    previous: Option<(u64, Vec<u8>)>,
    dump: Option<BufWriter<File>>,
    stats: AudioStats,
    latency: Vec<f64>,
}

impl Streamer {
    /// What the default output device plays, until `stop`.
    fn loopback(&mut self, stop: &AtomicBool) {
        let mut opened_once = false;
        while !stop.load(Ordering::Relaxed) {
            let mut lb = match Loopback::open_default() {
                Ok(lb) => lb,
                Err(e) => {
                    if !opened_once {
                        eprintln!("audio: no capture ({e}); trying again every second");
                        opened_once = true;
                    }
                    sleep_unless(stop, RETRY);
                    continue;
                }
            };
            let f = lb.format();
            println!("audio: capturing «{}» ({f})", lb.device());
            if opened_once {
                self.stats.reopened += 1;
            }
            opened_once = true;
            // Resampled to 48 kHz if the device runs at another rate.
            let mut framer = To48k::new(f.rate, f.channels);
            // On from where the last device left off, its pause included.
            self.base = self
                .next_index
                .map_or(0, |(i, at)| i + (at.elapsed().as_millis() / 10) as u64);
            let mut pauses = Pauses::new();
            let (mut samples, mut packets, mut frames) = (Vec::new(), Vec::new(), Vec::new());
            let mut checked = Instant::now();
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                samples.clear();
                packets.clear();
                if let Err(e) = lb.read(&mut samples, &mut packets) {
                    eprintln!("audio: capture lost ({e}); opening the device again");
                    break;
                }
                pauses.feed(&packets, &samples, f, &mut framer, &mut frames);
                self.send_frames(&mut frames);
                self.stats.late_ms = pauses.late * 1000 / u64::from(f.rate);
                if checked.elapsed() >= DEVICE_CHECK {
                    checked = Instant::now();
                    if !lb.is_default() {
                        println!("audio: the default output device changed");
                        break;
                    }
                }
                thread::sleep(POLL);
            }
            self.stats.framer += framer.framer().stats;
        }
    }

    /// A 1 kHz tone at the pace of the clock, until `stop`, as a device at
    /// `rate` would give it; with `gaps`, every other 2 s nothing comes and
    /// the device's stream stands still (as loopback while nothing plays).
    fn tone(&mut self, stop: &AtomicBool, rate: u32, gaps: bool) {
        let how = if gaps { ", 2 s on, 2 s off" } else { "" };
        println!("audio: a 1 kHz test tone at {rate} Hz{how}");
        let format = audio::Format::stereo_float(rate);
        let mut framer = To48k::new(rate, 2);
        let mut pauses = Pauses::new();
        let start = Instant::now();
        let mut made: u64 = 0;
        let (mut packets, mut frames) = (Vec::new(), Vec::new());
        let w = std::f64::consts::TAU * 1000.0 / f64::from(rate);
        while !stop.load(Ordering::Relaxed) {
            // The time it has played: all of it, or the "on" halves of 4 s.
            let e = start.elapsed().as_secs_f64();
            let played = if gaps {
                (e / 4.0).floor() * 2.0 + (e % 4.0).min(2.0)
            } else {
                e
            };
            let due = (played * f64::from(rate)) as u64;
            packets.clear();
            let mut samples = Vec::new();
            if due > made {
                let n = (due - made) as u32;
                samples = (made..due)
                    .flat_map(|i| {
                        let s = 0.1 * (w * i as f64).sin() as f32;
                        [s, s]
                    })
                    .collect();
                packets.push(Packet {
                    frames: n,
                    device_position: made,
                    qpc_100ns: audio::qpc_now_100ns()
                        .saturating_sub(u64::from(n) * 10_000_000 / u64::from(rate)),
                    silent: false,
                    discontinuity: false,
                });
                made = due;
            }
            pauses.feed(&packets, &samples, format, &mut framer, &mut frames);
            self.send_frames(&mut frames);
            self.stats.late_ms = pauses.late * 1000 / u64::from(rate);
            thread::sleep(POLL);
        }
        self.stats.framer += framer.framer().stats;
    }

    fn send_frames(&mut self, frames: &mut Vec<Frame>) {
        let mut buf = [0u8; opus::MAX_PACKET];
        for mut f in frames.drain(..) {
            f.index += self.base;
            let n = match self.encoder.encode(&f.pcm, &mut buf) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("audio: {e}");
                    continue;
                }
            };
            let opus = buf[..n].to_vec();
            // The frame's time on the host's wall clock: for loopback, when
            // it sounds from this machine's device (the client measures its
            // lag from that); for the tone, when it was made. Never later
            // than now: some devices stamp their packets ahead (a USB one
            // on the problem host by ~200 ms, 2026-11-12), and the client
            // showed a negative lag. The log keeps the device's own age.
            let age_us = (audio::qpc_now_100ns() as i64 - f.qpc_100ns as i64) / 10;
            let capture_us = (wall_clock_us() as i64 - age_us.max(0)).max(0) as u64;
            let previous = self
                .previous
                .take()
                .filter(|(i, _)| *i + 1 == f.index)
                .map(|(_, p)| p);
            let packet = AudioPacket {
                frame: f.index,
                capture_us,
                opus,
                previous,
            };
            let mut d = packet.datagram(self.out.session, self.out.seq.take());
            seal(&self.out.sealer, &mut d);
            self.stats.bytes += d.len() as u64;
            self.out.sender.send(d);
            self.stats.frames += 1;
            transport::push_recent(
                &mut self.latency,
                age_us as f64 / 1000.0,
                transport::RECENT_SAMPLES,
            );
            if let Some(w) = self.dump.as_mut() {
                let _ = write_record(w, f.index, capture_us, &packet.opus);
            }
            self.next_index = Some((f.index + 1, Instant::now()));
            self.previous = Some((f.index, packet.opus));
        }
    }

    fn finish(mut self) -> AudioStats {
        if let Some(w) = self.dump.as_mut() {
            let _ = w.flush();
        }
        let v = &mut self.latency;
        v.sort_by(f64::total_cmp);
        if !v.is_empty() {
            self.stats.latency_min_ms = v[0];
            self.stats.latency_p50_ms = v[v.len() / 2];
            self.stats.latency_max_ms = v[v.len() - 1];
        }
        self.stats
    }
}

/// Pauses in what the device gives: while anything plays, a packet every
/// 10 ms; while nothing does, loopback gives none and its stream stands still.
/// Then silence goes to the framer at every look, at the pace of the clock
/// and as far ahead as the device stamps its packets (loopback: when they
/// sound here, ~8 ms on), so the stream keeps its timing across pauses.
///
/// A pause may be this thread not woken in time (the machine is busy): the
/// packets then come late, for a time already given as silence. Their part
/// before the silence's end is dropped (by their time stamps): else the
/// stream would hold that time twice, and grow.
struct Pauses {
    /// Till when sound or silence has been given to the framer (100 ns).
    heard_until: u64,
    /// The end of the silence put in: what a packet has before it is late.
    silent_until: u64,
    /// How far ahead of being read the device stamps its packets.
    lead: u64,
    idle: bool,
    /// Device frames dropped as late.
    late: u64,
}

impl Pauses {
    fn new() -> Pauses {
        Pauses {
            heard_until: audio::qpc_now_100ns(),
            silent_until: 0,
            lead: 0,
            idle: false,
            late: 0,
        }
    }

    /// One read: its packets (and their samples, in the device's `format`)
    /// to the framer, or silence if none came for a while.
    fn feed(
        &mut self,
        packets: &[Packet],
        samples: &[f32],
        format: audio::Format,
        framer: &mut To48k,
        frames: &mut Vec<Frame>,
    ) {
        let read_at = audio::qpc_now_100ns();
        if let Some(p) = packets.last() {
            self.lead = p.qpc_100ns.saturating_sub(read_at);
        }
        let ch = usize::from(format.channels);
        let rate = u64::from(format.rate);
        let mut at = 0;
        for p in packets {
            let n = p.frames as usize * ch;
            let mut q = *p;
            // The first sound after a pause: silence up to where it starts
            // (else that bit of time would be missing from the stream).
            if std::mem::take(&mut self.idle) && q.qpc_100ns > self.silent_until {
                let gap = (q.qpc_100ns - self.silent_until) * u64::from(opus::RATE) / 10_000_000;
                if gap <= u64::from(opus::RATE) / 10 {
                    framer.idle(gap, q.qpc_100ns, frames);
                }
            }
            if q.qpc_100ns < self.silent_until {
                let late = ((self.silent_until - q.qpc_100ns) * rate / 10_000_000)
                    .min(u64::from(q.frames)) as u32;
                q.frames -= late;
                q.device_position += u64::from(late);
                q.qpc_100ns += u64::from(late) * 10_000_000 / rate;
                self.late += u64::from(late);
            }
            if q.frames > 0 {
                let skip = (p.frames - q.frames) as usize * ch;
                framer.push(&q, &samples[at + skip..at + n], frames);
            }
            at += n;
            let end = p.qpc_100ns + u64::from(p.frames) * 10_000_000 / rate;
            self.heard_until = self.heard_until.max(end);
        }
        let now = read_at + self.lead;
        self.idle = packets.is_empty() && (self.idle || now > self.heard_until + IDLE_AFTER_100NS);
        if self.idle && now > self.heard_until {
            // Whole samples: the time moves on by just as much (what is left
            // goes with the next look, else every look would lose a bit).
            let silence = (now - self.heard_until) * u64::from(opus::RATE) / 10_000_000;
            let end = self.heard_until + silence * 10_000_000 / u64::from(opus::RATE);
            framer.idle(silence, end, frames);
            self.heard_until = end;
            self.silent_until = end;
        }
    }
}

fn sleep_unless(stop: &AtomicBool, d: Duration) {
    let until = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
        thread::sleep(Duration::from_millis(50));
    }
}

/// `--dump-audio`: one record per frame — index u64, capture_us u64, length
/// u16, the Opus packet (little-endian). `lumen-host audio-probe --decode` reads it.
fn write_record(
    w: &mut impl Write,
    index: u64,
    capture_us: u64,
    opus: &[u8],
) -> std::io::Result<()> {
    w.write_all(&index.to_le_bytes())?;
    w.write_all(&capture_us.to_le_bytes())?;
    w.write_all(&(opus.len() as u16).to_le_bytes())?;
    w.write_all(opus)
}

/// Reads a `--dump-audio` file: (index, capture_us, Opus packet) per frame.
pub fn read_dump(path: &Path) -> Result<Vec<(u64, u64, Vec<u8>)>> {
    let data = std::fs::read(path).with_context(|| format!("read {path:?}"))?;
    let mut out = Vec::new();
    let mut at = 0;
    while at + 18 <= data.len() {
        let index = u64::from_le_bytes(data[at..at + 8].try_into()?);
        let capture = u64::from_le_bytes(data[at + 8..at + 16].try_into()?);
        let n = usize::from(u16::from_le_bytes(data[at + 16..at + 18].try_into()?));
        at += 18;
        anyhow::ensure!(at + n <= data.len(), "{path:?}: cut short");
        out.push((index, capture, data[at..at + n].to_vec()));
        at += n;
    }
    Ok(out)
}
