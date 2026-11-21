//! The session's numbers for the overlay (Ctrl+Alt+Shift+O). The render thread
//! feeds them ([`Feed`]: every presented frame, and the network counters a few
//! times a second); [`Stats`] is what they come to. What they mean — words,
//! colours, the look — is the page's (web/client/src/overlay), which
//! gets them through `ui_api`, in a window of its own: never in the picture.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::{ChromaFormat, Codec, VideoFormat};
use serde::Serialize;

use crate::audio::pct;
use crate::net::Counters;

/// How far back the graph goes.
pub const SPAN: Duration = Duration::from_secs(10);
/// The numbers are over this much of the latest frames.
const RECENT: Duration = Duration::from_secs(2);

/// Shown or not (the hotkey switches it).
static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn toggle() {
    SHOWN.fetch_xor(true, Ordering::Relaxed);
}

pub fn shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

/// One presented frame, ms: capture → reassembled here (the host's encode
/// and pacing, and the network); the decoder's own time; the rest until it
/// was presented (waiting for the decoder, then for the swapchain).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub at: Instant,
    pub net_ms: f32,
    pub decode_ms: f32,
    pub show_ms: f32,
}

impl FrameSample {
    fn total(&self) -> f32 {
        self.net_ms + self.decode_ms + self.show_ms
    }
}

/// The session as the render thread sees it.
#[derive(Clone, Copy, Default)]
pub struct Snapshot {
    /// The stream's format (`None` only in tests).
    pub format: Option<VideoFormat>,
    /// Frames per second the host was asked for.
    pub fps: f64,
    pub counters: Counters,
    /// Frames presented.
    pub shown: u64,
    /// The host's sound (`None`: not played).
    pub audio: Option<crate::audio::AudioStatus>,
}

/// Snapshots over time, for rates.
#[derive(Clone, Copy)]
struct Point {
    at: Instant,
    snap: Snapshot,
}

/// How the monitors are shown, as the page needs it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ViewInfo {
    /// `all`, `one` (the circles switch), `windows`.
    pub mode: &'static str,
    /// The shown monitor, in `one`.
    pub active: usize,
    /// How many monitors the host has.
    pub count: usize,
    /// Monitor 1's place in the window, as shares: [x, y, width, height].
    pub first: [f32; 4],
}

#[derive(Default)]
struct Model {
    frames: VecDeque<FrameSample>,
    points: VecDeque<Point>,
    view: ViewInfo,
    notice: Option<crate::ui_api::Notice>,
}

/// What the render thread writes and the overlay's thread reads.
#[derive(Clone, Default)]
pub struct Feed(Arc<Mutex<Model>>);

impl Feed {
    pub fn frame(&self, f: FrameSample) {
        let mut m = self.0.lock().unwrap();
        m.frames.push_back(f);
        while m.frames.front().is_some_and(|x| f.at - x.at > SPAN) {
            m.frames.pop_front();
        }
    }

    pub fn snapshot(&self, at: Instant, snap: Snapshot) {
        let mut m = self.0.lock().unwrap();
        m.points.push_back(Point { at, snap });
        // One more second than the graph: rates over a second at its left end.
        while m
            .points
            .front()
            .is_some_and(|p| at - p.at > SPAN + Duration::from_secs(1))
        {
            m.points.pop_front();
        }
    }

    pub fn stats(&self, now: Instant) -> Option<Stats> {
        let m = self.0.lock().unwrap();
        Stats::of(&m.frames, &m.points, now)
    }

    pub fn set_view(&self, view: ViewInfo) {
        self.0.lock().unwrap().view = view;
    }

    pub fn view_info(&self) -> ViewInfo {
        self.0.lock().unwrap().view.clone()
    }

    /// Something for the page to say for a moment.
    pub fn say(&self, code: &'static str) {
        let mut m = self.0.lock().unwrap();
        let serial = m.notice.as_ref().map_or(1, |n| n.serial + 1);
        m.notice = Some(crate::ui_api::Notice { code, serial });
    }

    pub fn notice(&self) -> Option<crate::ui_api::Notice> {
        self.0.lock().unwrap().notice.clone()
    }
}

/// One column of the latency graphs, ms: the slowest frame in its time slice,
/// and the host's encode as it reported then (part of `net`). As an array in
/// JSON ([net, decode, show, lost, host]): there are 200 of them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Column(pub f32, pub f32, pub f32, pub bool, pub f32);

impl Column {
    fn total(&self) -> f32 {
        self.0 + self.1 + self.2
    }
}

/// The picture's format, as the stream carries it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FormatInfo {
    pub width: u16,
    pub height: u16,
    pub codec: &'static str,
    pub bit_depth: u8,
    pub chroma: &'static str,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

/// The host's sound.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioStats {
    /// Anything played lately.
    pub playing: bool,
    pub lag_ms: Option<f32>,
    /// Gaps over the last 2 s, and over the graph's span.
    pub gaps_recent: u64,
    pub gaps_span: u64,
}

/// What the numbers come to: medians over the latest frames, unless named.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stats {
    /// Frames over the latest 2 s (0: nothing comes).
    pub frames_recent: usize,
    /// Capture → presented.
    pub latency_ms: f32,
    /// Capture → reassembled here: the host's encode and pacing, the network.
    pub net_ms: f32,
    pub net_p95_ms: f32,
    /// The decoder's own time.
    pub decode_ms: f32,
    pub decode_p95_ms: f32,
    /// Waiting for the decoder and the swapchain.
    pub show_ms: f32,
    /// The host's encode, as it last reported (lately).
    pub encode_ms: Option<f32>,
    pub host_fps: Option<f64>,
    /// Asked of the host, and shown here over the last second.
    pub fps: f64,
    pub shown_fps: f64,
    pub rtt_ms: Option<f64>,
    /// Over the latest 2 s.
    pub loss_percent: f64,
    pub frames_lost_recent: u32,
    pub video_mbps: f64,
    /// The host's cap; 0: not known.
    pub cap_mbps: f64,
    pub audio: Option<AudioStats>,
    pub format: Option<FormatInfo>,
    /// The frame interval.
    pub budget_ms: f32,
    /// Oldest first, over [`SPAN`]; `null`: no frame in the slice.
    pub columns: Vec<Option<Column>>,
}

/// Columns in the latency graph.
pub const COLUMNS: usize = 200;

impl Stats {
    fn of(frames: &VecDeque<FrameSample>, points: &VecDeque<Point>, now: Instant) -> Option<Stats> {
        let last = points.back()?;
        let s = last.snap;
        let c = s.counters;
        // The counters a second ago (or the oldest), for rates.
        let ago = |d: Duration| {
            points
                .iter()
                .rev()
                .find(|p| last.at - p.at >= d)
                .or(points.front())
                .copied()
                .unwrap_or(*last)
        };
        let sec = ago(Duration::from_secs(1));
        let dt = (last.at - sec.at).as_secs_f64().max(1e-3);
        let mbps = (c.bytes - sec.snap.counters.bytes) as f64 * 8.0 / dt / 1e6;
        let shown_fps = (s.shown - sec.snap.shown) as f64 / dt;

        let recent: Vec<&FrameSample> = frames
            .iter()
            .filter(|f| now.saturating_duration_since(f.at) <= RECENT)
            .collect();
        let mut net: Vec<f32> = recent.iter().map(|f| f.net_ms).collect();
        let mut dec: Vec<f32> = recent.iter().map(|f| f.decode_ms).collect();
        let mut show: Vec<f32> = recent.iter().map(|f| f.show_ms).collect();
        let mut total: Vec<f32> = recent.iter().map(|f| f.total()).collect();
        let (net_ms, net_p95_ms) = (pct(&mut net, 0.5), pct(&mut net, 0.95));
        let (decode_ms, decode_p95_ms) = (pct(&mut dec, 0.5), pct(&mut dec, 0.95));
        let show_ms = pct(&mut show, 0.5);
        let latency_ms = pct(&mut total, 0.5);

        let host = c
            .host
            .filter(|(_, at)| now.saturating_duration_since(*at) < Duration::from_secs(3))
            .map(|(h, _)| h);
        let host_fps = host.map(|h| f64::from(h.fps_x10) / 10.0);

        // Over the last two seconds: datagrams lost, frames lost.
        let two = ago(RECENT);
        let got = c
            .stats
            .packets_received
            .saturating_sub(two.snap.counters.stats.packets_received);
        let lost = c
            .stats
            .packets_lost
            .saturating_sub(two.snap.counters.stats.packets_lost);
        let loss = 100.0 * f64::from(lost) / f64::from((got + lost).max(1));
        let frames_lost_recent = c.frames_lost.saturating_sub(two.snap.counters.frames_lost);
        // The sound: gaps lately, and whether any is playing.
        let audio_since = |from: &Point| match (s.audio, from.snap.audio) {
            (Some(a), Some(b)) => (
                a.underruns.saturating_sub(b.underruns),
                a.frames.saturating_sub(b.frames),
            ),
            (Some(a), None) => (a.underruns, a.frames),
            _ => (0, 0),
        };
        let (gaps_recent, played) = audio_since(&two);
        let (gaps_span, _) = audio_since(&ago(SPAN));
        let (video_mbps, cap_mbps) = match host {
            Some(h) => (f64::from(h.video_kbps) / 1e3, f64::from(h.cap_kbps) / 1e3),
            None => (mbps, 0.0),
        };

        Some(Stats {
            frames_recent: recent.len(),
            latency_ms,
            net_ms,
            net_p95_ms,
            decode_ms,
            decode_p95_ms,
            show_ms,
            encode_ms: host.map(|h| h.encode_p50_us as f32 / 1e3),
            host_fps,
            fps: s.fps,
            shown_fps,
            rtt_ms: c.clock.last_rtt_ms(),
            loss_percent: loss,
            frames_lost_recent,
            video_mbps,
            cap_mbps,
            audio: s.audio.map(|a| AudioStats {
                playing: played > 0,
                lag_ms: a.lag_ms,
                gaps_recent,
                gaps_span,
            }),
            format: s.format.as_ref().map(format_info),
            budget_ms: (1000.0 / s.fps.max(1.0)) as f32,
            columns: columns(frames, points, now),
        })
    }
}

fn format_info(f: &VideoFormat) -> FormatInfo {
    FormatInfo {
        width: f.width,
        height: f.height,
        codec: match f.codec {
            Codec::HevcMain10 => "HEVC",
            Codec::Av1 => "AV1",
        },
        bit_depth: f.bit_depth,
        chroma: match f.chroma {
            ChromaFormat::Yuv420 => "4:2:0",
            ChromaFormat::Yuv444 => "4:4:4",
        },
        primaries: f.primaries,
        transfer: f.transfer,
        matrix: f.matrix,
        full_range: f.full_range,
    }
}

/// A tenth of a millisecond is plenty, and keeps the JSON short.
fn round(ms: f32) -> f32 {
    (ms * 10.0).round() / 10.0
}

/// The latency graph: [`COLUMNS`] slices of [`SPAN`] up to `now`.
fn columns(
    frames: &VecDeque<FrameSample>,
    points: &VecDeque<Point>,
    now: Instant,
) -> Vec<Option<Column>> {
    let slice = SPAN / COLUMNS as u32;
    let index = |at: Instant| {
        let back = now.saturating_duration_since(at);
        (back < SPAN).then(|| COLUMNS - 1 - (back.as_nanos() / slice.as_nanos()) as usize)
    };
    let mut out: Vec<Option<Column>> = vec![None; COLUMNS];
    for f in frames {
        let Some(i) = index(f.at) else { continue };
        let slot = &mut out[i];
        if slot.is_none_or(|c| c.total() < f.total()) {
            let lost = slot.is_some_and(|c| c.3);
            *slot = Some(Column(
                round(f.net_ms),
                round(f.decode_ms),
                round(f.show_ms),
                lost,
                0.0,
            ));
        }
    }
    // Lost frames, as the counters grew between snapshots.
    for w in points.iter().collect::<Vec<_>>().windows(2) {
        if w[1].snap.counters.frames_lost > w[0].snap.counters.frames_lost
            && let Some(i) = index(w[1].at)
        {
            out[i].get_or_insert(Column(0.0, 0.0, 0.0, true, 0.0)).3 = true;
        }
    }
    // The host reports once a second: each slice has the report last known then.
    let mut reports: Vec<Option<f32>> = vec![None; COLUMNS];
    for p in points {
        if let (Some(i), Some((h, _))) = (index(p.at), p.snap.counters.host) {
            reports[i] = Some(round(h.encode_p50_us as f32 / 1e3));
        }
    }
    let mut host = reports.iter().flatten().next().copied().unwrap_or(0.0);
    for (slot, report) in out.iter_mut().zip(reports) {
        host = report.unwrap_or(host);
        if let Some(c) = slot {
            c.4 = host;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::HostStats;

    /// Two steady seconds: 60 frames a second, 3 MB/s received; at the end,
    /// `lost_frames` frames and `lost_packets` datagrams lost.
    fn feed(t0: Instant, decode_ms: f32, lost_frames: u32, lost_packets: u32) -> Feed {
        let feed = Feed::default();
        for i in 0..=10u32 {
            let at = t0 + Duration::from_millis(u64::from(i) * 200);
            let mut s = Snapshot {
                fps: 60.0,
                shown: u64::from(i) * 12,
                ..Snapshot::default()
            };
            s.counters.bytes = u64::from(i) * 600_000;
            s.counters.stats.packets_received = i * 100;
            if i == 10 {
                s.counters.frames_lost = lost_frames;
                s.counters.stats.packets_lost = lost_packets;
            }
            s.counters.host = Some((
                HostStats {
                    fps_x10: 600,
                    video_kbps: 21_000,
                    cap_kbps: 40_000,
                    qp_x10: 162,
                    encode_p50_us: 2_500,
                    encode_max_us: 3_000,
                    ..HostStats::default()
                },
                at,
            ));
            feed.snapshot(at, s);
        }
        for i in 0..120u64 {
            feed.frame(FrameSample {
                at: t0 + Duration::from_millis(i * 2000 / 120),
                net_ms: 20.0,
                decode_ms,
                show_ms: 2.0,
            });
        }
        feed
    }

    #[test]
    fn numbers_of_a_healthy_stream() {
        let t0 = Instant::now();
        let v = feed(t0, 5.0, 0, 0)
            .stats(t0 + Duration::from_secs(2))
            .unwrap();
        assert_eq!(v.latency_ms, 27.0);
        assert_eq!((v.net_ms, v.decode_ms, v.show_ms), (20.0, 5.0, 2.0));
        assert_eq!((v.fps, v.shown_fps.round()), (60.0, 60.0));
        assert_eq!((v.video_mbps, v.cap_mbps), (21.0, 40.0));
        assert_eq!((v.loss_percent, v.frames_lost_recent), (0.0, 0));
        assert!((v.encode_ms.unwrap() - 2.5).abs() < 1e-3);
        assert!((v.budget_ms - 16.67).abs() < 0.01);
        assert_eq!(v.audio, None);
    }

    #[test]
    fn losses_are_counted_and_marked() {
        let t0 = Instant::now();
        let now = t0 + Duration::from_secs(2);
        let saved = feed(t0, 5.0, 0, 20).stats(now).unwrap();
        assert!(saved.loss_percent >= 1.0 && saved.frames_lost_recent == 0);
        let lossy = feed(t0, 5.0, 2, 20).stats(now).unwrap();
        assert_eq!(lossy.frames_lost_recent, 2);
        // The lost frames are marked at the right end of the graph.
        assert!(lossy.columns.last().unwrap().unwrap().3);
    }

    #[test]
    fn the_format_as_the_stream_carries_it() {
        let f = format_info(&VideoFormat::lumen_default(3840, 2160, 60));
        assert_eq!((f.width, f.height, f.codec), (3840, 2160, "HEVC"));
        assert_eq!((f.bit_depth, f.chroma, f.full_range), (10, "4:2:0", true));
    }

    #[test]
    fn the_sound_and_its_gaps() {
        let t0 = Instant::now();
        let f = feed(t0, 5.0, 0, 0);
        let snap = |frames, underruns| Snapshot {
            fps: 60.0,
            shown: 120,
            audio: Some(crate::audio::AudioStatus {
                frames,
                underruns,
                concealed: 0,
                lag_ms: Some(42.4),
            }),
            ..Snapshot::default()
        };
        // Playing, no gaps.
        f.snapshot(t0 + Duration::from_millis(2100), snap(200, 0));
        f.snapshot(t0 + Duration::from_millis(4100), snap(400, 0));
        let a = f
            .stats(t0 + Duration::from_millis(4100))
            .unwrap()
            .audio
            .unwrap();
        assert!(a.playing && a.gaps_recent == 0 && a.lag_ms == Some(42.4));
        // A gap just now.
        f.snapshot(t0 + Duration::from_millis(4300), snap(420, 1));
        let a = f
            .stats(t0 + Duration::from_millis(4300))
            .unwrap()
            .audio
            .unwrap();
        assert_eq!((a.gaps_recent, a.gaps_span), (1, 1));
        // Nothing comes any more.
        f.snapshot(t0 + Duration::from_millis(6500), snap(420, 1));
        let a = f
            .stats(t0 + Duration::from_millis(6500))
            .unwrap()
            .audio
            .unwrap();
        assert!(!a.playing);
    }

    #[test]
    fn no_frames_lately_is_said_so() {
        let t0 = Instant::now();
        let v = feed(t0, 5.0, 0, 0)
            .stats(t0 + Duration::from_secs(10))
            .unwrap();
        assert_eq!(v.frames_recent, 0);
    }

    #[test]
    fn the_json_is_short_and_the_columns_are_arrays() {
        let t0 = Instant::now();
        let v = feed(t0, 5.0, 0, 0)
            .stats(t0 + Duration::from_secs(2))
            .unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.len() < 8000, "{} bytes", json.len());
        assert!(json.contains("[20.0,5.0,2.0,false,2.5]"), "{json}");
    }

    #[test]
    fn each_column_has_the_host_report_of_its_time() {
        let t0 = Instant::now();
        let mut frames = VecDeque::new();
        let mut points = VecDeque::new();
        for ms in 0..4u64 {
            let at = t0 + Duration::from_millis(ms * 1000);
            frames.push_back(FrameSample {
                at,
                net_ms: 20.0,
                decode_ms: 5.0,
                show_ms: 1.0,
            });
            if ms == 1 || ms == 3 {
                let mut snap = Snapshot::default();
                let stats = HostStats {
                    encode_p50_us: if ms == 1 { 2_000 } else { 7_000 },
                    ..HostStats::default()
                };
                snap.counters.host = Some((stats, at));
                points.push_back(Point { at, snap });
            }
        }
        let now = t0 + Duration::from_millis(3010);
        let hosts: Vec<f32> = columns(&frames, &points, now)
            .iter()
            .flatten()
            .map(|c| c.4)
            .collect();
        // Before the first report: its value; then each report from its time on.
        assert_eq!(hosts, [2.0, 2.0, 2.0, 7.0]);
    }

    /// What the page gets: web/client/src/overlay/api.ts has the same (signal::contract).
    #[test]
    fn the_page_has_the_same() {
        use crate::ui_api::{Notice, OverlayState, TransferState};
        let t0 = Instant::now();
        let mut stats = feed(t0, 5.0, 2, 3)
            .stats(t0 + Duration::from_secs(2))
            .unwrap();
        // The last slices: enough for their shape.
        stats.columns = stats.columns.split_off(COLUMNS - 3);
        stats.columns[0] = None;
        stats.format = Some(FormatInfo {
            width: 3840,
            height: 2160,
            codec: "HEVC",
            bit_depth: 10,
            chroma: "4:2:0",
            primaries: 1,
            transfer: 1,
            matrix: 1,
            full_range: true,
        });
        stats.audio = Some(AudioStats {
            playing: true,
            lag_ms: Some(48.0),
            gaps_recent: 0,
            gaps_span: 0,
        });
        let transfer = |stage, reason, set: Option<u32>, ended_ago_ms| TransferState {
            outgoing: set.is_some(),
            download: set.is_none(),
            set,
            batch: set.is_none().then_some(3),
            image: false,
            transfer: 1,
            name: "a.mov".into(),
            len: 100,
            done: 50,
            stage,
            reason,
            speed: 1.5,
            started_ago_ms: 1000,
            ended_ago_ms,
        };
        let view = ViewInfo {
            mode: "one",
            active: 1,
            count: 3,
            first: [0.0, 0.0, 0.5, 0.75],
        };
        signal::contract::check_example(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../web/client/src/contract/overlay.example.ts"
            ),
            "OverlayState",
            "../overlay/api",
            &[
                OverlayState {
                    picture_scale: 0.5,
                    view: view.clone(),
                    stats_shown: true,
                    stats: Some(stats),
                    transfers: vec![
                        transfer("running", None, None, None),
                        transfer("cancelled", Some("locked"), Some(4), Some(10)),
                    ],
                    transfers_key: 1,
                    notice: Some(Notice {
                        code: "two_windows_on_one_monitor",
                        serial: 1,
                    }),
                },
                OverlayState {
                    picture_scale: 1.0,
                    view,
                    stats_shown: false,
                    stats: None,
                    transfers: Vec::new(),
                    transfers_key: 0,
                    notice: None,
                },
            ],
        );
    }

    #[test]
    fn graph_columns_keep_the_slowest_frame_of_a_slice() {
        let t0 = Instant::now();
        let mut frames = VecDeque::new();
        for (ms, total) in [(0, 10.0), (10, 30.0), (20, 15.0)] {
            frames.push_back(FrameSample {
                at: t0 + Duration::from_millis(ms),
                net_ms: total,
                decode_ms: 0.0,
                show_ms: 0.0,
            });
        }
        let now = t0 + Duration::from_millis(20);
        let cols = columns(&frames, &VecDeque::new(), now);
        assert_eq!(cols.len(), COLUMNS);
        // A slice is 50 ms: all three in the last one.
        assert_eq!(cols[COLUMNS - 1].unwrap().0, 30.0);
        assert!(cols[..COLUMNS - 1].iter().all(Option::is_none));
    }

    #[test]
    fn toggled_by_the_hotkey() {
        assert!(!shown());
        toggle();
        assert!(shown());
        toggle();
        assert!(!shown());
    }
}
