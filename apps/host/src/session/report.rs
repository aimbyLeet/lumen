//! The session's numbers: a line per monitor every second, the host's
//! numbers for the client's overlay, the sound at the end.

use std::time::Instant;

use protocol::{ControlMessage, HostStats};

use super::Session;
use crate::streamer::{SessionRate, Stream, Window};

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// The session's sound, as it went: one line for the log.
pub(super) fn report_audio(a: &crate::audio::AudioStats) {
    let f = &a.framer;
    let ms = |frames: u64| frames * 1000 / u64::from(audio::opus::RATE);
    println!(
        "audio: {} frames, {:.0} kbit/s sent, frame time -> sent min {:+.1} / p50 {:+.1} / max {:+.1} ms; device stream: gaps {} (filled {} ms), overlaps {} ({} frames dropped), restarts {}; silence while the device gave nothing {} ms (sound late for it, dropped: {} ms); device reopened {} times",
        a.frames,
        a.bytes as f64 * 8.0 / (a.frames.max(1) as f64 * 10.0),
        a.latency_min_ms,
        a.latency_p50_ms,
        a.latency_max_ms,
        f.gaps,
        ms(f.filled),
        f.overlaps,
        f.skipped,
        f.restarts,
        ms(f.idled),
        a.late_ms,
        a.reopened
    );
}

impl Session<'_> {
    /// The last second: a line per monitor here (monitor 1's as before, with
    /// the sender's numbers), the numbers to the client (its overlay).
    pub(super) fn report(&mut self) {
        if self.streams.is_empty() {
            self.win_start = Instant::now();
            return;
        }
        let secs = self.win_start.elapsed().as_secs_f64();
        let s = self.sender.stats();
        let mut wins: Vec<Window> = self.streams.iter().map(Stream::take_window).collect();
        let one_shown = self.monitors.shown.count_ones() == 1;
        let stats = host_stats(
            &mut wins,
            secs,
            one_shown,
            self.ceiling.cap_bps,
            &self.shared.rate,
            s.queued,
        );
        self.send_control(&ControlMessage::HostStats(stats));
        let nominal = self.shared.rate.nominal_bps();
        for (i, w) in wins.iter_mut().enumerate() {
            if i > 0 && w.frames == 0 {
                continue;
            }
            let line = format!(
                "{:.1} fps ({} new images), video {:.1} Mbit/s (encoder set to {:.1}, QP {:.1}, max frame {} KiB), encode p50 {:.2} p99 {:.2} ms, {} IDR, {} recoveries ({} repeat requests ignored)",
                f64::from(w.frames) / secs,
                w.captured,
                w.bytes as f64 * 8.0 / secs / 1e6,
                f64::from(nominal) / 1e6,
                w.qp_sum as f64 / f64::from(w.frames.max(1)),
                w.max_frame / 1024,
                pct(&mut w.encode_ms, 0.5),
                pct(&mut w.encode_ms, 0.99),
                w.keyframes,
                w.recoveries,
                w.ignored_requests,
            );
            let line = match w.map_delta {
                Some(d) => format!(
                    "{line}, QP map: moving +{d} on {:.1} % of blocks",
                    100.0 * w.moving_share_sum / f64::from(w.frames.max(1))
                ),
                None => line,
            };
            if i == 0 {
                println!(
                    "{line}, sent {} pkts ({} queued, {} errors)",
                    s.datagrams, s.queued, s.send_errors
                );
            } else {
                println!("monitor {}: {line}", i + 1);
            }
        }
        self.win_start = Instant::now();
    }
}

/// The host's numbers of the last second for the client's overlay: the
/// monitors together (frames per second: the busiest monitor's; with one
/// shown at a time, all of theirs: a switch splits the second between two,
/// and the overlay said the host gave fewer frames, 2026-10-30).
fn host_stats(
    wins: &mut [Window],
    secs: f64,
    one_shown: bool,
    cap_bps: u32,
    rate: &SessionRate,
    queued: usize,
) -> HostStats {
    let kbps = |bps: f64| (bps / 1e3).round() as u32;
    let frames: u32 = wins.iter().map(|w| w.frames).sum();
    let max_frames = if one_shown {
        frames
    } else {
        wins.iter().map(|w| w.frames).max().unwrap_or(0)
    };
    let bytes: u64 = wins.iter().map(|w| w.bytes).sum();
    let qp_sum: u64 = wins.iter().map(|w| w.qp_sum).sum();
    let mut encode_ms: Vec<f64> = wins.iter().flat_map(|w| w.encode_ms.clone()).collect();
    HostStats {
        fps_x10: (f64::from(max_frames) / secs * 10.0).round() as u16,
        video_kbps: kbps(bytes as f64 * 8.0 / secs),
        encoder_kbps: kbps(f64::from(rate.nominal_bps())),
        cap_kbps: kbps(f64::from(cap_bps)),
        qp_x10: (qp_sum as f64 / f64::from(frames.max(1)) * 10.0).round() as u16,
        encode_p50_us: (pct(&mut encode_ms, 0.5) * 1e3) as u32,
        encode_max_us: (encode_ms.iter().copied().fold(0.0, f64::max) * 1e3) as u32,
        queued: queued as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(frames: u32) -> Window {
        Window {
            frames,
            ..Window::default()
        }
    }

    #[test]
    fn a_switch_between_monitors_does_not_halve_the_frame_rate() {
        let rate = SessionRate::new(None);
        // Mode 2: monitor 1 for half the second, then monitor 2.
        let mut wins = [win(30), win(30), win(0)];
        assert_eq!(host_stats(&mut wins, 1.0, true, 0, &rate, 0).fps_x10, 600);
        // Mode 1: every monitor at once, the busiest one counts.
        let mut wins = [win(60), win(58), win(59)];
        assert_eq!(host_stats(&mut wins, 1.0, false, 0, &rate, 0).fps_x10, 600);
    }
}
