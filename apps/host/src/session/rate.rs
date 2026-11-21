//! The session's ceiling: the encoder's rate control, the cap for the
//! monitors shown, lowered while our files go out.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use capture::qpmap::QpMapParams;
use nvenc::RateControl;
use signal::IncomingSession;
use transport::CapController;

use super::Session;
use crate::app::{Args, RcArg};
use crate::station::Station;

/// Rate-control buffer of the capped mode, in frames at the cap: a footage frame
/// (200-600 KiB at QP 16 in 4K) may exceed one frame's share of the cap while
/// the average stays under it. Owner's choice.
const VBV_FRAMES: u32 = 3;
/// The video's cap while our files go out (the owner's choice, measured): the files get the rest of the client's line.
const VIDEO_CAP_WHILE_FILES: u32 = 10_000_000;

/// The encoder's rate control and the session's ceiling.
pub(super) struct Ceiling {
    pub(super) rate_control: RateControl,
    /// The session's ceiling, bit/s; 0: none (CQP).
    pub(super) cap_bps: u32,
    /// Our files are going out (the files thread says), and whether the
    /// video's cap is lowered for them now.
    pub(super) files_sending: Arc<AtomicBool>,
    pub(super) video_low: bool,
}

/// Per-session bitrate caps, Mbit/s of video, when the server sent no table
/// (its defaults are the same: apps/server/src/db.rs). Sized so the office
/// worst case (10 x 4K + 10 x 2K + 20 x FHD, all at 60 Hz and all moving) fits
/// in 1 Gbit/s with FEC and headers.
fn default_cap_mbps(width: u32, height: u32, fps: u32) -> f64 {
    let pixels = u64::from(width) * u64::from(height);
    let (at60, at30) = if pixels > 2560 * 1440 {
        (40.0, 24.0)
    } else if pixels > 1920 * 1080 {
        (18.0, 11.0)
    } else {
        (10.0, 6.0)
    };
    if fps <= 30 { at30 } else { at60 }
}

/// The session's frame rate: the server's word (the client's choice or the
/// admin's), else the host's.
pub(super) fn session_fps(args: &Args, inc: &IncomingSession) -> u32 {
    match inc.quality.fps {
        30 | 60 => u32::from(inc.quality.fps),
        _ => args.fps,
    }
}

/// The session's ceiling, bit/s, for monitors of `pixels` together:
/// `--max-mbps`, else the client's cap, else the server's table for this size
/// and rate. One ceiling for all the monitors shown (the owner's decision,
/// 2026-10-29): 2K + 2K gets the 4K row; one 2K shown of two — the 2K row.
fn session_cap_bps(st: &Station, inc: &IncomingSession, fps: u32, pixels: u64) -> u32 {
    let (w, h) = (u32::try_from(pixels).unwrap_or(u32::MAX), 1);
    let cap = if st.args.max_mbps > 0.0 {
        st.args.max_mbps
    } else if !inc.quality.is_auto() {
        f64::from(inc.quality.mbps)
    } else {
        signal::cap_for(&st.caps, w, h, fps).map_or_else(|| default_cap_mbps(w, h, fps), f64::from)
    };
    (cap * 1e6) as u32
}

/// The pixels of the station's screens whose bit is set in `shown`.
fn pixels(st: &Station, shown: u8) -> u64 {
    st.screens
        .iter()
        .enumerate()
        .filter(|(i, _)| shown & (1 << i) != 0)
        .map(|(_, s)| u64::from(s.width) * u64::from(s.height))
        .sum()
}

/// The finest QP the encoder may use: still regions' with the QP map, else --qp.
fn floor_qp(args: &Args) -> u32 {
    args.static_qp.0.map_or(args.qp, |q| q.min(args.qp))
}

/// The QP map's settings (`--static-qp`): moving blocks at --qp, a block
/// still after half a second.
pub(super) fn static_map(args: &Args, fps: u32) -> Option<QpMapParams> {
    args.static_qp.0.map(|_| QpMapParams {
        still_qp: floor_qp(args) as u8,
        moving_qp: args.qp as u8,
        still_after: (fps / 2).max(1) as u16,
    })
}

/// The encoder's rate control, its steering and the session's ceiling
/// (bit/s; 0: none), every monitor shown.
pub(super) fn rate_control(
    st: &Station,
    inc: &IncomingSession,
    fps: u32,
) -> (RateControl, Option<CapController>, u32) {
    let args = &st.args;
    match args.rc {
        RcArg::Capped => {
            let max_bps = session_cap_bps(st, inc, fps, pixels(st, u8::MAX));
            (
                RateControl::CappedQuality {
                    min_qp: floor_qp(args),
                    max_bps,
                    vbv_frames: VBV_FRAMES,
                },
                // With the map the controller sees the encoder's base QP
                // (`StaticQpMap::observe`): it may rise while that is above
                // the floor.
                Some(CapController::new(max_bps, floor_qp(args), Instant::now())),
                max_bps,
            )
        }
        RcArg::Cqp => (RateControl::ConstQp { qp: floor_qp(args) }, None, 0),
    }
}

impl Session<'_> {
    /// The ceiling for the monitors shown now (one 2K of two: the 2K row).
    pub(super) fn follow_shown(&mut self) {
        if self.ceiling.cap_bps == 0 {
            return;
        }
        let px: u64 = self
            .monitors
            .stream_pixels
            .iter()
            .enumerate()
            .filter(|(i, _)| self.monitors.shown & (1 << i) != 0)
            .map(|(_, p)| p)
            .sum();
        let cap = session_cap_bps(self.st, self.inc, self.fps, px.max(1));
        if cap != self.ceiling.cap_bps {
            self.ceiling.cap_bps = cap;
            println!(
                "video cap {:.0} Mbit/s (monitors shown)",
                f64::from(cap) / 1e6
            );
            self.apply_cap();
        }
    }

    /// The ceiling to the encoders: lowered while our files go out.
    fn apply_cap(&self) {
        let cap = if self.ceiling.video_low {
            self.ceiling.cap_bps.min(VIDEO_CAP_WHILE_FILES)
        } else {
            self.ceiling.cap_bps
        };
        self.shared.rate.set_cap(cap);
    }

    /// The cap lowered while our files go out (and back after).
    pub(super) fn steer_files(&mut self) {
        let low = self.ceiling.files_sending.load(Ordering::Relaxed);
        if low == self.ceiling.video_low || self.ceiling.cap_bps == 0 {
            return;
        }
        self.ceiling.video_low = low;
        println!(
            "video cap {:.0} Mbit/s ({})",
            f64::from(if low {
                self.ceiling.cap_bps.min(VIDEO_CAP_WHILE_FILES)
            } else {
                self.ceiling.cap_bps
            }) / 1e6,
            if low { "files going out" } else { "files done" }
        );
        self.apply_cap();
    }
}
