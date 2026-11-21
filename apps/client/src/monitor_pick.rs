//! A host without monitors asks for ours: what Lumen
//! Client offers (the resolutions and scales Lumen VDD has, the closest to
//! the monitor the video window opens on first), and the user's answer.

use std::sync::Mutex;

use protocol::ControlMessage;
use serde::Deserialize;
use serde_json::{Value, json};
use vdd::layout::{RESOLUTIONS, SCALES};

/// A monitor for a host without one: `3440x1440@125` (resolution, scale %).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct MonitorPick {
    pub width: u32,
    pub height: u32,
    pub scale: u16,
}

impl std::str::FromStr for MonitorPick {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("{s:?}: expected WIDTHxHEIGHT@SCALE, like 3440x1440@125");
        let (size, scale) = s.split_once('@').ok_or_else(bad)?;
        let (w, h) = size.split_once('x').ok_or_else(bad)?;
        Ok(MonitorPick {
            width: w.trim().parse().map_err(|_| bad())?,
            height: h.trim().parse().map_err(|_| bad())?,
            scale: scale.trim().parse().map_err(|_| bad())?,
        })
    }
}

impl MonitorPick {
    pub fn answer(&self, ask: u32) -> ControlMessage {
        ControlMessage::MonitorChoice {
            ask,
            width: self.width,
            height: self.height,
            scale: self.scale,
        }
    }
}

/// What the host says while it has no monitor (`MonitorWanted`,
/// `NoMonitors`): the hub is told once per change, the question answered
/// each time it comes, once there is an answer.
#[derive(Default)]
pub struct HostWaits {
    last: Option<ControlMessage>,
}

impl HostWaits {
    /// `m` heard (`mid`: in session, not while connecting); the answer to
    /// send, if any.
    pub fn heard(&mut self, m: &ControlMessage, mid: bool) -> Option<ControlMessage> {
        if self.last.as_ref() != Some(m) {
            let now = if mid { " now" } else { "" };
            match m {
                ControlMessage::MonitorWanted { ask } => {
                    println!("the host has no monitor{now}: it asks for ours (question {ask})");
                    crate::status::monitor_wanted(*ask);
                }
                _ => {
                    println!("the host has no monitor{now}, and none can be made (no Lumen VDD)");
                    crate::status::no_monitors();
                }
            }
            self.last = Some(m.clone());
        }
        match m {
            ControlMessage::MonitorWanted { ask } => picked().map(|p| p.answer(*ask)),
            _ => None,
        }
    }
}

/// The user's answer (or `--monitor-choice`): sent for each question of the
/// host's, until its monitors come.
static PICKED: Mutex<Option<MonitorPick>> = Mutex::new(None);

pub fn set(p: MonitorPick) {
    *PICKED.lock().unwrap() = Some(p);
}

pub fn picked() -> Option<MonitorPick> {
    *PICKED.lock().unwrap()
}

/// The list's resolution closest to `width` × `height`: the same one, else
/// the closest of the same shape, else the closest in size.
fn closest(width: u32, height: u32) -> (u32, u32) {
    let shape = |w: u32, h: u32| f64::from(w) / f64::from(h.max(1));
    let size = |w: u32, h: u32| f64::from(w) * f64::from(h);
    let want = (shape(width, height), size(width, height));
    RESOLUTIONS
        .iter()
        .min_by(|a, b| {
            let d = |r: &&vdd::layout::Resolution| {
                let shape_off = (shape(r.width, r.height) - want.0).abs() > 0.02;
                (shape_off, (size(r.width, r.height) / want.1).ln().abs())
            };
            let (da, db) = (d(a), d(b));
            da.0.cmp(&db.0).then(da.1.total_cmp(&db.1))
        })
        .map_or((1920, 1080), |r| (r.width, r.height))
}

/// The list's scale closest to `percent`.
fn closest_scale(percent: u16) -> u16 {
    SCALES
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(percent))
        .unwrap_or(100)
}

/// What the hub needs to ask: the choices and the suggestion for the
/// monitor `width` × `height` at `scale` %.
pub fn question(ask: u32, (width, height, scale): (u32, u32, u16)) -> Value {
    let (w, h) = closest(width, height);
    json!({
        "event": "monitor_wanted",
        "ask": ask,
        "suggest": { "width": w, "height": h, "scale": closest_scale(scale) },
        "resolutions": RESOLUTIONS
            .iter()
            .map(|r| json!({ "width": r.width, "height": r.height, "aspect": r.aspect }))
            .collect::<Vec<_>>(),
        "scales": SCALES,
    })
}

/// The monitor the video window opens on first (the primary one): its size,
/// pixels, and scale, %.
pub fn this_monitor() -> (u32, u32, u16) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let r = crate::window::monitor_info(mon).rcMonitor;
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (
            (r.right - r.left).max(0) as u32,
            (r.bottom - r.top).max(0) as u32,
            (dx * 100 / 96) as u16,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_pick_reads() {
        let p: MonitorPick = "3440x1440@125".parse().unwrap();
        assert_eq!((p.width, p.height, p.scale), (3440, 1440, 125));
        for bad in ["3440x1440", "3440@125", "x1440@125", "3440x1440@", ""] {
            assert!(bad.parse::<MonitorPick>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_suggestion_is_the_closest_of_the_list() {
        assert_eq!(closest(3440, 1440), (3440, 1440));
        assert_eq!(closest(1920, 1080), (1920, 1080));
        // Not in the list: the same shape first.
        assert_eq!(closest(2880, 1800), (2560, 1600));
        assert_eq!(closest(1366, 768), (1920, 1080));
        assert_eq!(closest(2560, 1080), (2560, 1080));
        // A shape the list lacks: the closest in size.
        assert_eq!(closest(1280, 1024), (1920, 1080));
        assert_eq!(closest_scale(125), 125);
        assert_eq!(closest_scale(110), 100);
        assert_eq!(closest_scale(300), 200);
    }

    #[test]
    fn the_question_has_the_choices() {
        let q = question(3, (3440, 1440, 125));
        assert_eq!(q["ask"], 3);
        assert_eq!(
            q["suggest"],
            json!({ "width": 3440, "height": 1440, "scale": 125 })
        );
        assert_eq!(
            q["resolutions"].as_array().unwrap().len(),
            RESOLUTIONS.len()
        );
        assert_eq!(q["scales"].as_array().unwrap().len(), SCALES.len());
    }
}
