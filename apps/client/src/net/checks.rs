//! Check-only parts of the network thread: simulated loss and the link
//! emulator (`--sim-*`), `--packet-log`, the test pastes (`--test-paste` and
//! the like), the other monitors' frames (`--dump-streams`).

use std::time::{Duration, Instant};

use protocol::FragmentHeader;
use transport::{LinkConfig, LinkEmulator, Payload, wall_clock_us};

use super::Options;

/// Simulated loss: true for a `loss` fraction of calls (deterministic per seed).
pub(super) fn sim_dropper(loss: f64, seed: u64) -> impl FnMut() -> bool {
    let mut rng = transport::Rng::new(seed);
    move || loss > 0.0 && rng.next_f64() < loss
}

/// The network thread's simulated downlink (`--sim-*`).
pub(super) struct Sim {
    pub(super) loss: f64,
    pub(super) rng: transport::Rng,
    pub(super) emulator: Option<LinkEmulator>,
}

impl Sim {
    /// Simulated loss (`--sim-loss`): true for that fraction of calls.
    #[inline]
    pub(super) fn drop_one(&mut self) -> bool {
        self.loss > 0.0 && self.rng.next_f64() < self.loss
    }
}

/// The downlink emulator, if any `--sim-*` asks for one.
pub(super) fn emulator(opts: &Options) -> Option<LinkEmulator> {
    (opts.sim_bandwidth_bps.is_some() || !opts.sim_delay.is_zero() || opts.sim_stall.is_some())
        .then(|| {
            LinkEmulator::new(LinkConfig {
                bandwidth_bps: opts.sim_bandwidth_bps,
                delay: opts.sim_delay,
                stall_every: opts.sim_stall.map(|s| s.0),
                stall_len: opts.sim_stall.map_or(Duration::ZERO, |s| s.1),
                ..LinkConfig::default()
            })
        })
}

/// `--packet-log`.
pub(super) fn packet_log(
    path: Option<&std::path::Path>,
) -> Option<std::io::BufWriter<std::fs::File>> {
    let p = path?;
    std::fs::File::create(p)
        .map(|f| std::io::BufWriter::with_capacity(1 << 20, f))
        .map_err(|e| eprintln!("--packet-log {}: {e}", p.display()))
        .ok()
}

/// One line of `--packet-log` for a datagram of `n` bytes that came at `t`.
#[inline]
pub(super) fn log_datagram(
    log: &mut std::io::BufWriter<std::fs::File>,
    seq: u64,
    t: u64,
    n: usize,
    video: Option<FragmentHeader>,
) {
    use std::io::Write;
    let _ = match video {
        Some(f) => writeln!(
            log,
            "V {} {t} {n} {} {} {} {} {}",
            seq, f.frame, f.block, f.index, f.data_shards, f.parity_shards
        ),
        None => writeln!(log, "C {} {t} {n}", seq),
    };
}

/// `--test-paste` and the like: when, latest first (popped from the end).
pub(super) fn test_times(seconds: &[f64]) -> Vec<Instant> {
    let now = Instant::now();
    let mut t: Vec<Instant> = seconds
        .iter()
        .map(|&s| now + Duration::from_secs_f64(s))
        .collect();
    t.sort_by(|a, b| b.cmp(a));
    t
}

/// Checks: when to paste here / in the video window (once each).
pub(super) struct TestPastes {
    pub(super) test_paste: Vec<Instant>,
    pub(super) test_host_paste: Vec<Instant>,
}

/// Frames of the other monitors: counted, and written as they came.
pub(super) struct OtherStreams {
    pub(super) other_frames: [u64; protocol::MAX_STREAMS as usize],
    /// Their capture → reassembled here, ms (host clock corrected): the
    /// recent ones (`transport::push_recent`).
    pub(super) other_latency: [Vec<f64>; protocol::MAX_STREAMS as usize],
    pub(super) dump_streams: Option<std::path::PathBuf>,
    pub(super) stream_dumps: Vec<Option<std::io::BufWriter<std::fs::File>>>,
}

impl OtherStreams {
    pub(super) fn new(dump_streams: Option<std::path::PathBuf>) -> Self {
        OtherStreams {
            other_frames: Default::default(),
            other_latency: Default::default(),
            dump_streams,
            stream_dumps: Vec::new(),
        }
    }

    /// A frame of monitor 2 or on: counted (and written, `--dump-streams`).
    pub(super) fn other_stream(&mut self, p: &Payload, clock: &crate::clock::ClockSync) {
        use std::io::Write;
        let s = usize::from(p.stream) % protocol::MAX_STREAMS as usize;
        self.other_frames[s] += 1;
        let host_now = wall_clock_us() as i64 + clock.offset_us().unwrap_or(0);
        transport::push_recent(
            &mut self.other_latency[s],
            (host_now - p.capture_time_us as i64) as f64 / 1e3,
            transport::RECENT_SAMPLES,
        );
        let Some(base) = &self.dump_streams else {
            return;
        };
        if self.stream_dumps.len() <= s {
            self.stream_dumps.resize_with(s + 1, || None);
        }
        if self.stream_dumps[s].is_none() {
            let path = base.with_extension(s.to_string());
            match std::fs::File::create(&path) {
                Ok(f) => self.stream_dumps[s] = Some(std::io::BufWriter::new(f)),
                Err(e) => eprintln!("{path:?}: {e}"),
            }
        }
        if let Some(f) = self.stream_dumps[s].as_mut() {
            let _ = f.write_all(&p.data);
        }
    }

    /// At the end of the session: what came of each, and the dumps flushed.
    pub(super) fn finish(&mut self) {
        for (s, n) in self
            .other_frames
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
        {
            let l = &mut self.other_latency[s];
            l.sort_by(f64::total_cmp);
            let at = |p: f64| transport::percentile(l, p);
            println!(
                "monitor {}: {n} frames received; capture -> reassembled ms: p50 {:.2}  p95 {:.2}  p99 {:.2}",
                s + 1,
                at(0.5),
                at(0.95),
                at(0.99)
            );
        }
        for f in self.stream_dumps.iter_mut().flatten() {
            use std::io::Write;
            let _ = f.flush();
        }
    }
}

/// The network thread's check-only state.
pub(super) struct Checks {
    pub(super) sim: Sim,
    /// "V <seq> <µs> <bytes> <frame> <block> <index> <data> <parity>" per video
    /// datagram, "C <seq> <µs> <bytes>" per other one (µs: our clock, on arrival).
    pub(super) packet_log: Option<std::io::BufWriter<std::fs::File>>,
    pub(super) others: OtherStreams,
    pub(super) pastes: TestPastes,
}
