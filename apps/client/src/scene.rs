//! The host's monitors as the video window shows them where each one is on the host's desktop, and where on the canvas
//! the window shows — the same layout with thin gaps between the monitors.
//!
//! Pointer positions are desktop coordinates both ways (protocol v10); the
//! window turns a mouse position into one ([`Scene::to_desktop`]), the
//! renderer turns the host's pointer into a place on the canvas
//! ([`Scene::to_canvas`]). One monitor: the canvas is that monitor, no gaps —
//! the picture of before.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use protocol::Monitor;

/// A rectangle: left, top, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        (self.x..self.x + self.w).contains(&x) && (self.y..self.y + self.h).contains(&y)
    }

    /// The point of the rectangle nearest to (x, y), and how far it is (squared).
    fn nearest(&self, x: i32, y: i32) -> ((i32, i32), i64) {
        let cx = x.clamp(self.x, self.x + self.w - 1);
        let cy = y.clamp(self.y, self.y + self.h - 1);
        let (dx, dy) = (i64::from(x - cx), i64::from(y - cy));
        ((cx, cy), dx * dx + dy * dy)
    }
}

/// One monitor of the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub stream: u8,
    /// On the host's desktop.
    pub desktop: Rect,
    /// On the canvas (its top-left corner at 0, 0).
    pub canvas: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Scene {
    /// Monitor 1 first.
    pub monitors: Vec<Placed>,
    pub canvas: (u32, u32),
}

impl Scene {
    /// A single picture of `w` × `h` at the desktop's (0, 0): before the
    /// host says where its monitors are.
    pub fn single(w: u32, h: u32) -> Scene {
        Self::of(&[Monitor {
            stream: 0,
            x: 0,
            y: 0,
            width: w,
            height: h,
        }])
    }

    /// The host's monitors (`ControlMessage::Monitors`), laid out with gaps:
    /// every distinct left edge after the first moves right by one more gap,
    /// every top edge down — monitors that touch get a gap between them and
    /// keep their places relative to each other.
    pub fn of(monitors: &[Monitor]) -> Scene {
        if monitors.is_empty() {
            return Scene::default();
        }
        let rect = |m: &Monitor| Rect {
            x: m.x,
            y: m.y,
            w: m.width as i32,
            h: m.height as i32,
        };
        let left = monitors.iter().map(|m| m.x).min().unwrap_or(0);
        let top = monitors.iter().map(|m| m.y).min().unwrap_or(0);
        let right = monitors
            .iter()
            .map(|m| m.x + m.width as i32)
            .max()
            .unwrap_or(0);
        let bottom = monitors
            .iter()
            .map(|m| m.y + m.height as i32)
            .max()
            .unwrap_or(0);
        let gap = if monitors.len() > 1 {
            ((right - left).max(bottom - top) / GAP_PARTS).max(2)
        } else {
            0
        };
        let edges = |f: fn(&Monitor) -> i32| {
            let mut e: Vec<i32> = monitors.iter().map(f).collect();
            e.sort_unstable();
            e.dedup();
            e
        };
        let (xs, ys) = (edges(|m| m.x), edges(|m| m.y));
        let shift = |edges: &[i32], v: i32| gap * edges.iter().filter(|&&e| e < v).count() as i32;
        let placed: Vec<Placed> = monitors
            .iter()
            .map(|m| Placed {
                stream: m.stream,
                desktop: rect(m),
                canvas: Rect {
                    x: m.x - left + shift(&xs, m.x),
                    y: m.y - top + shift(&ys, m.y),
                    w: m.width as i32,
                    h: m.height as i32,
                },
            })
            .collect();
        let w = placed
            .iter()
            .map(|p| p.canvas.x + p.canvas.w)
            .max()
            .unwrap_or(0);
        let h = placed
            .iter()
            .map(|p| p.canvas.y + p.canvas.h)
            .max()
            .unwrap_or(0);
        Scene {
            monitors: placed,
            canvas: (w as u32, h as u32),
        }
    }

    /// More than one monitor: the composite view.
    pub fn is_multi(&self) -> bool {
        self.monitors.len() > 1
    }

    /// A canvas point → the desktop point under it, and whether it is on a
    /// monitor. Over a gap or beyond the monitors: the nearest monitor's
    /// edge (the host's pointer stops there).
    pub fn to_desktop(&self, x: i32, y: i32) -> ((i32, i32), bool) {
        let Some((m, (cx, cy), d)) = self
            .monitors
            .iter()
            .map(|m| {
                let (p, d) = m.canvas.nearest(x, y);
                (m, p, d)
            })
            .min_by_key(|(_, _, d)| *d)
        else {
            return ((x, y), false);
        };
        (
            (m.desktop.x + cx - m.canvas.x, m.desktop.y + cy - m.canvas.y),
            d == 0,
        )
    }

    /// A desktop point → its place on the canvas (`None`: on no monitor shown).
    pub fn to_canvas(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.monitors
            .iter()
            .find(|m| m.desktop.contains(x, y))
            .map(|m| (m.canvas.x + x - m.desktop.x, m.canvas.y + y - m.desktop.y))
    }
}

/// The gap between monitors: this part of the canvas's larger side (16 px on
/// a 6400-wide canvas; about half that on screen).
const GAP_PARTS: i32 = 400;

/// The scene of the (one) video window: set by the render thread, read by the
/// window procedure (mouse) and the render thread (drawing).
static SCENE: Mutex<Option<Scene>> = Mutex::new(None);
/// Bumped whenever the host's monitors change (not the view of them): a
/// framed window then takes the new layout's aspect.
static HOST_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn host_changed() {
    HOST_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn host_generation() -> u64 {
    HOST_GENERATION.load(Ordering::Relaxed)
}

/// Mode 3's other windows: the scene each one shows (one monitor), by window.
static EXTRA: Mutex<Vec<(isize, Scene)>> = Mutex::new(Vec::new());

pub fn set(scene: Scene) {
    *SCENE.lock().unwrap() = Some(scene);
}

/// The scene of mode 3's window `hwnd` (`None`: it is gone).
pub fn set_for(hwnd: isize, scene: Option<Scene>) {
    let mut extra = EXTRA.lock().unwrap();
    extra.retain(|(h, _)| *h != hwnd);
    if let Some(s) = scene {
        extra.push((hwnd, s));
    }
}

/// The scene window `hwnd` shows: its own (mode 3), else the main one's.
pub fn for_window(hwnd: isize) -> Scene {
    EXTRA
        .lock()
        .unwrap()
        .iter()
        .find(|(h, _)| *h == hwnd)
        .map_or_else(current, |(_, s)| s.clone())
}

pub fn current() -> Scene {
    SCENE.lock().unwrap().clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(stream: u8, x: i32, y: i32, width: u32, height: u32) -> Monitor {
        Monitor {
            stream,
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn one_monitor_is_the_picture_of_before() {
        let s = Scene::of(&[m(0, 0, 0, 3840, 2160)]);
        assert!(!s.is_multi());
        assert_eq!(s.canvas, (3840, 2160));
        assert_eq!(s.to_desktop(10, 20), ((10, 20), true));
        assert_eq!(s.to_canvas(3839, 2159), Some((3839, 2159)));
        // Beyond the picture: its edge, not on it.
        assert_eq!(s.to_desktop(-5, 3000), ((0, 2159), false));
    }

    #[test]
    fn touching_monitors_get_a_gap() {
        // 4K, 2K right of it centred, FHD above the 2K (Lumen Host's two.json).
        let s = Scene::of(&[
            m(0, 0, 0, 3840, 2160),
            m(1, 3840, 360, 2560, 1440),
            m(2, 3840, -720, 1920, 1080),
        ]);
        let gap = 6400 / GAP_PARTS;
        // Top edges -720, 0, 360: monitor 1 one gap down, monitor 2 two.
        let r = |x, y, w, h| Rect { x, y, w, h };
        assert_eq!(s.monitors[0].canvas, r(0, 720 + gap, 3840, 2160));
        assert_eq!(
            s.monitors[1].canvas,
            r(3840 + gap, 1080 + 2 * gap, 2560, 1440)
        );
        assert_eq!(s.monitors[2].canvas, r(3840 + gap, 0, 1920, 1080));
        assert_eq!(s.canvas, (6400 + gap as u32, 2880 + gap as u32));
        // No two monitors overlap or touch on the canvas.
        for a in &s.monitors {
            for b in &s.monitors {
                if a != b {
                    let (ra, rb) = (a.canvas, b.canvas);
                    let apart = ra.x + ra.w < rb.x
                        || rb.x + rb.w < ra.x
                        || ra.y + ra.h < rb.y
                        || rb.y + rb.h < ra.y;
                    assert!(apart, "{ra:?} {rb:?}");
                }
            }
        }
    }

    #[test]
    fn points_go_both_ways() {
        let s = Scene::of(&[m(0, 0, 0, 3840, 2160), m(1, -3440, 0, 3440, 1440)]);
        for (x, y) in [(0, 0), (3839, 2159), (-3440, 0), (-1, 1439), (100, 700)] {
            let c = s.to_canvas(x, y).unwrap();
            assert_eq!(s.to_desktop(c.0, c.1), ((x, y), true));
        }
        // Below the left monitor: no monitor there on the desktop.
        assert_eq!(s.to_canvas(-100, 2000), None);
    }

    #[test]
    fn a_gap_takes_the_pointer_to_the_nearest_edge() {
        let s = Scene::of(&[m(0, 0, 0, 1920, 1080), m(1, 1920, 0, 1920, 1080)]);
        let right = s.monitors[1].canvas;
        // In the gap, nearer the right monitor: its left edge, not on a monitor.
        let ((x, y), on) = s.to_desktop(right.x - 1, 500);
        assert_eq!(((x, y), on), ((1920, 500), false));
        let ((x, _), _) = s.to_desktop(1920, 500);
        assert_eq!(x, 1919, "the gap's left part is monitor 1's edge");
    }
}
