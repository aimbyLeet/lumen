//! The host's sound: a thread for the whole run (it
//! outlives reconnections) that plays what the network threads hand it, with
//! `audio::playout` between them and the output device.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use audio::Render;
use audio::playout::{Playout, PlayoutStats};
use protocol::AudioPacket;
use transport::wall_clock_us;

/// What plays the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AudioOut {
    /// The default output device.
    On,
    Off,
    /// No device: the sound is taken at the pace of the clock and dropped
    /// (checks without noise).
    Null,
}

/// From a network thread to the player.
pub enum AudioMsg {
    /// A new session: frame numbers start again.
    Reset,
    /// A datagram's frames, and the host's clock minus ours (µs), if known.
    Packet(AudioPacket, Option<i64>),
}

pub type AudioTx = Sender<AudioMsg>;

/// The sound so far, for the overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStatus {
    /// Frames played, gaps (the buffer ran dry), frames concealed.
    pub frames: u64,
    pub underruns: u64,
    pub concealed: u64,
    /// Host sound → played here, ms (median of the last second).
    pub lag_ms: Option<f32>,
}

/// `None`: no player (`--audio off`).
static STATUS: Mutex<Option<AudioStatus>> = Mutex::new(None);

pub fn status() -> Option<AudioStatus> {
    *STATUS.lock().unwrap()
}

/// How often the player publishes its [`AudioStatus`].
const PUBLISH: Duration = Duration::from_millis(200);
/// Lag samples the published median is over (a tick each: about a second).
const LAG_WINDOW: usize = 400;

/// How often the player looks at the device.
const TICK: Duration = Duration::from_millis(2);
/// Kept in the device ahead of what plays now (the rest waits in the playout):
/// more than this thread may sleep through on a busy machine, and at least
/// two of the device's periods (it takes a period at once).
const DEVICE_AHEAD_MS: f64 = 10.0;
/// After the device fails or is missing: try again in.
const RETRY: Duration = Duration::from_secs(1);
/// How often the default output device is checked.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// How long closing waits for the player thread (then it is left behind).
const JOIN_LIMIT: Duration = Duration::from_secs(1);

pub struct Player {
    tx: AudioTx,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// The player, or `None` with `AudioOut::Off`. `dump`: what is handed to
    /// the device, as a WAV at its rate (checks); `sim_ppm`: emulate this
    /// device's clock being off by this much.
    pub fn start(out: AudioOut, dump: Option<PathBuf>, sim_ppm: f64) -> Option<Player> {
        if out == AudioOut::Off {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        *STATUS.lock().unwrap() = Some(AudioStatus::default());
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(out, &rx, &s, dump, sim_ppm))
            .ok()?;
        Some(Player {
            tx,
            stop,
            thread: Some(thread),
        })
    }

    pub fn sender(&self) -> AudioTx {
        self.tx.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(t) = self.thread.take() else { return };
        // Not for good: a thread stuck somewhere must not keep the window
        // from closing (a 2 ms sleep that never ended, 2026-11-13).
        let until = Instant::now() + JOIN_LIMIT;
        let mut nap = desktop::nap::Nap::new();
        while !t.is_finished() && Instant::now() < until {
            nap.sleep(Duration::from_millis(5));
        }
        if t.is_finished() {
            let _ = t.join();
        } else {
            eprintln!("audio: the player thread did not stop in {JOIN_LIMIT:?}; left behind");
        }
    }
}

/// The output: a real device, or none at the pace of the clock.
enum Device {
    Real(Render),
    /// Frames not played yet, as of `at`: it plays at its rate while it has
    /// any (and silence when it runs dry), like a real device.
    Null {
        pending: f64,
        at: Instant,
    },
}

impl Device {
    fn rate(&self) -> u32 {
        match self {
            Device::Real(r) => r.format().rate,
            Device::Null { .. } => audio::opus::RATE,
        }
    }

    /// Frames written, not played yet.
    fn padding(&self) -> anyhow::Result<usize> {
        Ok(match self {
            Device::Real(r) => r.padding()? as usize,
            Device::Null { pending, at } => {
                let played = at.elapsed().as_secs_f64() * f64::from(audio::opus::RATE);
                (*pending - played).max(0.0) as usize
            }
        })
    }

    fn write(&mut self, stereo: &[f32]) -> anyhow::Result<usize> {
        Ok(match self {
            Device::Real(r) => r.write(stereo)?,
            Device::Null { pending, at } => {
                let played = at.elapsed().as_secs_f64() * f64::from(audio::opus::RATE);
                *pending = (*pending - played).max(0.0) + (stereo.len() / 2) as f64;
                *at = Instant::now();
                stereo.len() / 2
            }
        })
    }

    fn latency_us(&self) -> u64 {
        match self {
            Device::Real(r) => r.latency_us(),
            Device::Null { .. } => 0,
        }
    }

    /// How much it takes at once, frames (none: it plays sample by sample).
    fn period(&self) -> usize {
        match self {
            Device::Real(r) => r.period_frames() as usize,
            Device::Null { .. } => 0,
        }
    }

    /// Frames to keep in it.
    fn ahead(&self) -> usize {
        let ms = (DEVICE_AHEAD_MS * f64::from(self.rate()) / 1000.0) as usize;
        ms.max(2 * self.period())
    }
}

fn open(out: AudioOut) -> anyhow::Result<Device> {
    Ok(match out {
        AudioOut::Null => Device::Null {
            pending: 0.0,
            at: Instant::now(),
        },
        _ => {
            let r = Render::open_default()?;
            println!("audio: playing on «{}» ({})", r.device(), r.format());
            Device::Real(r)
        }
    })
}

fn run(
    out: AudioOut,
    rx: &Receiver<AudioMsg>,
    stop: &AtomicBool,
    dump: Option<PathBuf>,
    sim_ppm: f64,
) {
    audio::audio_thread_priority();
    let mut l = PlayerLoop {
        out,
        sim_ppm,
        device: None,
        playout: None,
        offset: None,
        retry_at: Instant::now(),
        reported_error: false,
        last: Instant::now(),
        published: Instant::now(),
        checked: Instant::now(),
        fed: None,
        buf: Vec::new(),
        dump: dump.map(|p| (p, Vec::new())),
        lags: Vec::new(),
        done: PlayoutStats::default(),
    };
    // Not `thread::sleep`: its timer may never fire (`desktop::nap`).
    let mut nap = desktop::nap::Nap::new();
    while !stop.load(Ordering::Relaxed) {
        if !nap.sleep(TICK) && nap.missed() <= 3 {
            eprintln!(
                "audio: a {TICK:?} sleep's timer did not fire in {:?} (missed {}); a new timer",
                TICK + desktop::nap::GRACE,
                nap.missed()
            );
        }
        let msgs: Vec<AudioMsg> = rx.try_iter().collect();
        if l.device.is_none() && Instant::now() >= l.retry_at {
            l.open_device();
        }
        if l.device.is_none() {
            continue;
        }
        l.take(msgs);
        if let Err(e) = l.feed() {
            eprintln!("audio: the output device failed ({e}); opening it again");
            l.device = None;
            continue;
        }
        l.publish();
        l.check_default();
    }
    if nap.missed() > 0 {
        println!("audio: sleep timers missed {} times", nap.missed());
    }
    l.finish();
}

/// The player thread's state.
struct PlayerLoop {
    out: AudioOut,
    sim_ppm: f64,
    device: Option<Device>,
    /// The playout for the open device (made afresh with each: its rate).
    playout: Option<Playout>,
    /// The host's clock minus ours, µs.
    offset: Option<i64>,
    retry_at: Instant,
    reported_error: bool,
    /// The last feed, publication, look at the default device.
    last: Instant,
    published: Instant,
    checked: Instant,
    /// What the device held after the last write, and when: if it played
    /// more than that since, it ran dry.
    fed: Option<(f64, Instant)>,
    buf: Vec<f32>,
    /// `--audio-dump`: where, and what was handed to the device.
    dump: Option<(PathBuf, Vec<f32>)>,
    /// Host sound → played here, ms, every feed.
    lags: Vec<f64>,
    /// The stats of the playouts of earlier devices.
    done: PlayoutStats,
}

impl PlayerLoop {
    fn open_device(&mut self) {
        match open(self.out) {
            Ok(d) => {
                // Afresh: a new device may run at another rate.
                if let Some(p) = self.playout.take() {
                    self.done.merge(p.stats);
                }
                self.playout = Playout::new(d.rate(), self.sim_ppm).ok();
                self.device = Some(d);
                self.reported_error = false;
            }
            Err(e) => {
                if !self.reported_error {
                    eprintln!("audio: no output device ({e}); trying again every second");
                    self.reported_error = true;
                }
                self.retry_at = Instant::now() + RETRY;
            }
        }
    }

    /// The network threads' messages to the playout.
    fn take(&mut self, msgs: Vec<AudioMsg>) {
        let Some(p) = self.playout.as_mut() else {
            return;
        };
        for m in msgs {
            match m {
                AudioMsg::Reset => {
                    let _ = p.reset();
                }
                AudioMsg::Packet(a, off) => {
                    self.offset = off.or(self.offset);
                    p.on_packet(a.frame, a.capture_us, a.opus, a.previous);
                }
            }
        }
    }

    /// Tops the device up to [`DEVICE_AHEAD_MS`] from the playout.
    fn feed(&mut self) -> anyhow::Result<()> {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f64();
        self.last = now;
        let (Some(d), Some(p)) = (self.device.as_mut(), self.playout.as_mut()) else {
            return Ok(());
        };
        let rate = f64::from(d.rate());
        let padding = d.padding()?;
        if let Some((held, at)) = self.fed {
            // Empty right after a take is normal; dry is having missed more
            // than a take since.
            let dry = at.elapsed().as_secs_f64() * rate - held;
            if padding == 0 && dry > d.period() as f64 {
                p.device_ran_dry(dry * 1000.0 / rate);
            }
        }
        let want = d.ahead().saturating_sub(padding);
        if want == 0 {
            return Ok(());
        }
        // The lag: when the next sample sounds here, against when it sounded
        // (or would have) at the host. Host clock = ours + offset.
        let host_next = p.next_host_us();
        self.buf.clear();
        let n = p.fill(want, padding, dt, &mut self.buf);
        if n == 0 {
            return Ok(());
        }
        if let (Some(h), Some(off)) = (host_next, self.offset) {
            let plays_at =
                wall_clock_us() as f64 + padding as f64 * 1e6 / rate + d.latency_us() as f64;
            transport::push_recent(
                &mut self.lags,
                (plays_at - (h as f64 - off as f64)) / 1000.0,
                transport::RECENT_SAMPLES,
            );
        }
        d.write(&self.buf)?;
        self.fed = Some(((padding + n) as f64, Instant::now()));
        if let Some((_, dumped)) = self.dump.as_mut() {
            dumped.extend_from_slice(&self.buf);
        }
        Ok(())
    }

    /// The [`AudioStatus`] for the overlay, every [`PUBLISH`].
    fn publish(&mut self) {
        if self.published.elapsed() < PUBLISH {
            return;
        }
        self.published = Instant::now();
        // The earlier devices' and this one's (not merged: no copies of the
        // level history, which grows all session).
        let (d, s) = (&self.done, self.playout.as_ref().map(|p| &p.stats));
        let sum = |f: fn(&PlayoutStats) -> u64| f(d) + s.map_or(0, f);
        let recent = &self.lags[self.lags.len().saturating_sub(LAG_WINDOW)..];
        let mut v: Vec<f32> = recent.iter().map(|&l| l as f32).collect();
        *STATUS.lock().unwrap() = Some(AudioStatus {
            frames: sum(|s| s.decoded),
            underruns: sum(PlayoutStats::gaps),
            concealed: sum(|s| s.concealed),
            lag_ms: (!v.is_empty()).then(|| pct(&mut v, 0.5)),
        });
    }

    /// Another device became the default: play there.
    fn check_default(&mut self) {
        if self.checked.elapsed() < DEVICE_CHECK {
            return;
        }
        self.checked = Instant::now();
        if let Some(Device::Real(r)) = &self.device
            && !r.is_default()
        {
            println!("audio: the default output device changed");
            self.device = None;
        }
    }

    /// The log at the end, and the dump.
    fn finish(mut self) {
        if let Some(p) = self.playout.take() {
            self.done.merge(p.stats);
        }
        report(&self.done, &mut self.lags);
        if let (Some((path, dumped)), Some(d)) = (self.dump, self.device.as_ref()) {
            match audio::wav::write_f32(&path, d.rate(), 2, &dumped) {
                Ok(()) => println!("audio: written {}", path.display()),
                Err(e) => eprintln!("audio: {}: {e}", path.display()),
            }
        }
    }
}

pub(crate) fn pct(v: &mut [f32], p: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn report(s: &PlayoutStats, lags: &mut [f64]) {
    let (mut level, mut adjust) = (s.level_ms.clone(), s.adjust_ppm.clone());
    let mut lag: Vec<f32> = lags.iter().map(|&l| l as f32).collect();
    println!(
        "audio: {} frames played, {} from the repeat copy, {} concealed, {} late; underruns {} (nothing here) + {} (the device emptied: fed late), {} where the host's sound stopped (not gaps), dropped {:.0} ms, resets {}",
        s.decoded,
        s.repeats_used,
        s.concealed,
        s.late,
        s.underruns,
        s.device_dry,
        s.stops,
        s.dropped_ms,
        s.resets
    );
    println!(
        "audio: level ms p50 {:.1} (min {:.1}, max {:.1}); speed ppm p50 {:+.0} (min {:+.0}, max {:+.0})",
        pct(&mut level, 0.5),
        pct(&mut level, 0.0),
        pct(&mut level, 1.0),
        pct(&mut adjust, 0.5),
        pct(&mut adjust, 0.0),
        pct(&mut adjust, 1.0),
    );
    if !lag.is_empty() {
        println!(
            "latency host sound -> played here ms: p50 {:.1}  p95 {:.1}  max {:.1}",
            pct(&mut lag, 0.5),
            pct(&mut lag, 0.95),
            pct(&mut lag, 1.0)
        );
    }
}
