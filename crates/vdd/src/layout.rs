//! The virtual monitors as Lumen Host's «Мониторы» sets them:
//! up to two, a resolution and a scale from fixed lists, 60 Hz, each one next
//! to another along an edge. The real monitors come first in the numbering
//! (monitor 1 is the primary, at (0, 0)), then the virtual ones; each of those
//! sits next to a monitor before it.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::contract::{MAX_MODES, Mode};

/// Virtual monitors at most.
pub const MAX_VIRTUAL: usize = 2;
/// Monitors a session shows at most (the owner's decision 2026-10-30): the
/// real ones first, so virtual ones beyond it would never be seen.
pub const MAX_MONITORS: usize = 3;
pub const REFRESH_HZ: u32 = 60;
/// Scales offered, %.
pub const SCALES: &[u16] = &[100, 125, 150, 175, 200];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    /// "16:9", "16:10", "21:9".
    pub aspect: &'static str,
}

const fn res(width: u32, height: u32, aspect: &'static str) -> Resolution {
    Resolution {
        width,
        height,
        aspect,
    }
}

/// Resolutions offered, by aspect, smallest first.
pub const RESOLUTIONS: &[Resolution] = &[
    res(1920, 1080, "16:9"),
    res(2560, 1440, "16:9"),
    res(3200, 1800, "16:9"),
    res(3840, 2160, "16:9"),
    res(1920, 1200, "16:10"),
    res(2560, 1600, "16:10"),
    res(3840, 2400, "16:10"),
    res(2560, 1080, "21:9"),
    res(3440, 1440, "21:9"),
    res(3840, 1600, "21:9"),
    res(5120, 2160, "21:9"),
];

pub fn resolution(width: u32, height: u32) -> Option<&'static Resolution> {
    RESOLUTIONS
        .iter()
        .find(|r| r.width == width && r.height == height)
}

/// Which edge of the monitor it attaches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// Where along that edge: its start (left or top), centre or end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attach {
    /// The monitor it is next to: a real one (1..) or a virtual one before it.
    pub to: u8,
    pub side: Side,
    pub align: Align,
}

/// One virtual monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Virtual {
    pub width: u32,
    pub height: u32,
    /// %, one of [`SCALES`].
    pub scale: u16,
    pub attach: Attach,
}

/// The machine's virtual monitors.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The monitors are up (off: «Отключить» — the settings stay).
    pub enabled: bool,
    /// The virtual monitors, in order.
    pub virtuals: Vec<Virtual>,
}

/// A monitor on the desktop, in pixels; monitor 1 is at (0, 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// What is wrong with the settings, for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    TooMany,
    Resolution { monitor: usize },
    Scale { monitor: usize },
    Attach { monitor: usize },
    Overlap { a: usize, b: usize },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::TooMany => write!(f, "err_layout_too_many|{MAX_VIRTUAL}"),
            LayoutError::Resolution { monitor } => {
                write!(f, "err_layout_resolution|{monitor}")
            }
            LayoutError::Scale { monitor } => write!(f, "err_layout_scale|{monitor}"),
            LayoutError::Attach { monitor } => {
                write!(f, "err_layout_attach|{monitor}")
            }
            LayoutError::Overlap { a, b } => write!(f, "err_layout_overlap|{a}|{b}"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// How many virtual monitors can be shown next to `real` real ones that are.
pub fn virtuals_allowed(real: usize) -> usize {
    MAX_MONITORS.saturating_sub(real).min(MAX_VIRTUAL)
}

/// The settings' own checks (what is next to what, and overlaps, need the
/// real monitors: [`place`]).
pub fn check(s: &Settings) -> Result<(), LayoutError> {
    if s.virtuals.len() > MAX_VIRTUAL {
        return Err(LayoutError::TooMany);
    }
    for (i, v) in s.virtuals.iter().enumerate() {
        let monitor = i + 2;
        if resolution(v.width, v.height).is_none() {
            return Err(LayoutError::Resolution { monitor });
        }
        if !SCALES.contains(&v.scale) {
            return Err(LayoutError::Scale { monitor });
        }
        if v.attach.to < 1 {
            return Err(LayoutError::Attach { monitor });
        }
    }
    Ok(())
}

/// Where the virtual monitors sit next to the `real` ones (primary first, as
/// numbered): none on top of another.
pub fn place(real: &[Rect], virtuals: &[Virtual]) -> Result<Vec<Rect>, LayoutError> {
    check(&Settings {
        enabled: true,
        virtuals: virtuals.to_vec(),
    })?;
    let mut rects = real.to_vec();
    for v in virtuals {
        let Some(&to) = rects.get(v.attach.to as usize - 1) else {
            return Err(LayoutError::Attach {
                monitor: rects.len() + 1,
            });
        };
        let (w, h) = (v.width as i32, v.height as i32);
        // Along the edge: from its start to its end.
        let along = |start: i32, len: i32, own: i32| match v.attach.align {
            Align::Start => start,
            Align::Center => start + (len - own) / 2,
            Align::End => start + len - own,
        };
        let (x, y) = match v.attach.side {
            Side::Left => (to.x - w, along(to.y, to.height as i32, h)),
            Side::Right => (to.right(), along(to.y, to.height as i32, h)),
            Side::Above => (along(to.x, to.width as i32, w), to.y - h),
            Side::Below => (along(to.x, to.width as i32, w), to.bottom()),
        };
        let rect = Rect {
            x,
            y,
            width: v.width,
            height: v.height,
        };
        if let Some(i) = rects.iter().position(|r| r.overlaps(&rect)) {
            return Err(LayoutError::Overlap {
                a: i + 1,
                b: rects.len() + 1,
            });
        }
        rects.push(rect);
    }
    Ok(rects.split_off(real.len()))
}

/// The modes a virtual monitor offers Windows: its resolution first, then
/// every other one of [`RESOLUTIONS`], all at [`REFRESH_HZ`]. Another one of
/// them is then set in place, without plugging the monitor out and in
///.
pub fn modes(width: u32, height: u32) -> Vec<Mode> {
    let mode = |width, height| Mode {
        width,
        height,
        refresh_hz: REFRESH_HZ,
    };
    std::iter::once(mode(width, height))
        .chain(
            RESOLUTIONS
                .iter()
                .filter(|r| (r.width, r.height) != (width, height))
                .map(|r| mode(r.width, r.height)),
        )
        .take(MAX_MODES)
        .collect()
}

/// Monitor 1 of a client's set: virtual too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    /// %, one of [`SCALES`].
    pub scale: u16,
}

/// A client's own monitors on a host that has none:
/// monitor 1, then up to two more, each next to one before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSet {
    pub primary: Screen,
    #[serde(default)]
    pub virtuals: Vec<Virtual>,
}

impl ClientSet {
    /// Just monitor 1: what the client chose on its first visit.
    pub fn of(primary: Screen) -> ClientSet {
        ClientSet {
            primary,
            virtuals: Vec::new(),
        }
    }

    /// Where all of them sit, monitor 1 at (0, 0); or what is wrong.
    pub fn rects(&self) -> Result<Vec<Rect>, LayoutError> {
        let p = self.primary;
        if resolution(p.width, p.height).is_none() {
            return Err(LayoutError::Resolution { monitor: 1 });
        }
        if !SCALES.contains(&p.scale) {
            return Err(LayoutError::Scale { monitor: 1 });
        }
        let first = Rect {
            x: 0,
            y: 0,
            width: p.width,
            height: p.height,
        };
        let mut all = vec![first];
        all.extend(place(&[first], &self.virtuals)?);
        Ok(all)
    }

    /// Monitor `n` (1..): resolution and scale.
    pub fn screen(&self, n: usize) -> Option<Screen> {
        if n == 1 {
            return Some(self.primary);
        }
        let v = self.virtuals.get(n.checked_sub(2)?)?;
        Some(Screen {
            width: v.width,
            height: v.height,
            scale: v.scale,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(width: u32, height: u32, to: u8, side: Side, align: Align) -> Virtual {
        Virtual {
            width,
            height,
            scale: 100,
            attach: Attach { to, side, align },
        }
    }

    fn r(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    const K4: &[Rect] = &[Rect {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
    }];

    #[test]
    fn every_side_and_alignment() {
        use Align::*;
        use Side::*;
        let cases = [
            (Right, Start, r(3840, 0, 2560, 1440)),
            (Right, Center, r(3840, 360, 2560, 1440)),
            (Right, End, r(3840, 720, 2560, 1440)),
            (Left, Start, r(-2560, 0, 2560, 1440)),
            (Left, End, r(-2560, 720, 2560, 1440)),
            (Above, Start, r(0, -1440, 2560, 1440)),
            (Above, Center, r(640, -1440, 2560, 1440)),
            (Above, End, r(1280, -1440, 2560, 1440)),
            (Below, Center, r(640, 2160, 2560, 1440)),
        ];
        for (side, align, want) in cases {
            let got = place(K4, &[v(2560, 1440, 1, side, align)]).unwrap();
            assert_eq!(got, [want], "{side:?} {align:?}");
        }
    }

    #[test]
    fn a_bigger_monitor_centres_beyond_the_edge() {
        let fhd = [r(0, 0, 1920, 1080)];
        let got = place(&fhd, &[v(3840, 2160, 1, Side::Left, Align::Center)]).unwrap();
        assert_eq!(got, [r(-3840, -540, 3840, 2160)]);
    }

    #[test]
    fn monitor_3_next_to_monitor_2() {
        let got = place(
            K4,
            &[
                v(2560, 1440, 1, Side::Right, Align::Center),
                v(1920, 1080, 2, Side::Above, Align::Start),
            ],
        )
        .unwrap();
        assert_eq!(got, [r(3840, 360, 2560, 1440), r(3840, -720, 1920, 1080)]);
    }

    #[test]
    fn real_monitors_are_in_the_way_and_can_be_attached_to() {
        // A second real monitor to the right of the primary.
        let real = [r(0, 0, 3840, 2160), r(3840, 0, 2560, 1440)];
        assert_eq!(
            place(&real, &[v(1920, 1080, 1, Side::Right, Align::Start)]).unwrap_err(),
            LayoutError::Overlap { a: 2, b: 3 }
        );
        assert_eq!(
            place(&real, &[v(1920, 1080, 2, Side::Right, Align::Start)]).unwrap(),
            [r(6400, 0, 1920, 1080)]
        );
        // The one after it may be next to it: number 3.
        assert_eq!(
            place(
                &real,
                &[
                    v(1920, 1080, 2, Side::Right, Align::Start),
                    v(1920, 1080, 3, Side::Below, Align::Start),
                ]
            )
            .unwrap()[1],
            r(6400, 1080, 1920, 1080)
        );
        // Not to itself, nor to a monitor that is not there.
        assert_eq!(
            place(&real, &[v(1920, 1080, 3, Side::Right, Align::Start)]).unwrap_err(),
            LayoutError::Attach { monitor: 3 }
        );
    }

    #[test]
    fn three_monitors_shown_at_most() {
        assert_eq!(virtuals_allowed(0), 2);
        assert_eq!(virtuals_allowed(1), 2);
        assert_eq!(virtuals_allowed(2), 1);
        assert_eq!(virtuals_allowed(3), 0);
        assert_eq!(virtuals_allowed(5), 0);
    }

    #[test]
    fn two_on_one_place_are_refused() {
        let same = [
            v(2560, 1440, 1, Side::Right, Align::Start),
            v(1920, 1080, 1, Side::Right, Align::End),
        ];
        assert_eq!(
            place(K4, &same).unwrap_err(),
            LayoutError::Overlap { a: 2, b: 3 }
        );
        // Below 2, which hangs below monitor 1's bottom: no overlap with 1.
        let ok = [
            v(2560, 1440, 1, Side::Right, Align::End),
            v(1920, 1080, 2, Side::Below, Align::Start),
        ];
        assert!(place(K4, &ok).is_ok());
        // Left of 2, back into monitor 1.
        let back = [
            v(2560, 1440, 1, Side::Right, Align::End),
            v(1920, 1080, 2, Side::Left, Align::Start),
        ];
        assert_eq!(
            place(K4, &back).unwrap_err(),
            LayoutError::Overlap { a: 1, b: 3 }
        );
    }

    #[test]
    fn checks() {
        let ok = v(2560, 1440, 1, Side::Right, Align::Start);
        let s = |virtuals: Vec<Virtual>| Settings {
            enabled: true,
            virtuals,
        };
        assert!(check(&s(vec![ok, ok])).is_ok());
        assert_eq!(check(&s(vec![ok; 3])), Err(LayoutError::TooMany));
        let odd = Virtual {
            width: 1366,
            height: 768,
            ..ok
        };
        assert_eq!(
            check(&s(vec![odd])),
            Err(LayoutError::Resolution { monitor: 2 })
        );
        let scale = Virtual { scale: 110, ..ok };
        assert_eq!(
            check(&s(vec![ok, scale])),
            Err(LayoutError::Scale { monitor: 3 })
        );
        let to_self = v(2560, 1440, 2, Side::Right, Align::Start);
        assert_eq!(
            place(K4, &[to_self]),
            Err(LayoutError::Attach { monitor: 2 })
        );
        let to_zero = v(2560, 1440, 0, Side::Right, Align::Start);
        assert_eq!(
            check(&s(vec![ok, to_zero])),
            Err(LayoutError::Attach { monitor: 3 })
        );
        assert_eq!(
            LayoutError::Overlap { a: 2, b: 3 }.to_string(),
            "err_layout_overlap|2|3"
        );
    }

    #[test]
    fn modes_start_with_the_chosen_one() {
        let m = modes(3440, 1440);
        assert_eq!((m[0].width, m[0].height), (3440, 1440));
        assert_eq!(m.len(), RESOLUTIONS.len());
        assert!(m.iter().all(|m| m.refresh_hz == 60));
        for r in RESOLUTIONS {
            assert_eq!(
                m.iter()
                    .filter(|m| (m.width, m.height) == (r.width, r.height))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn a_client_set() {
        let p = Screen {
            width: 3440,
            height: 1440,
            scale: 100,
        };
        let one = ClientSet::of(p);
        assert_eq!(one.rects().unwrap(), [r(0, 0, 3440, 1440)]);
        let two = ClientSet {
            virtuals: vec![v(1920, 1080, 1, Side::Right, Align::Start)],
            ..one.clone()
        };
        assert_eq!(
            two.rects().unwrap(),
            [r(0, 0, 3440, 1440), r(3440, 0, 1920, 1080)]
        );
        assert_eq!(two.screen(2).unwrap().width, 1920);
        assert_eq!(two.screen(3), None);
        let odd = ClientSet::of(Screen { width: 3000, ..p });
        assert_eq!(odd.rects(), Err(LayoutError::Resolution { monitor: 1 }));
        let scale = ClientSet::of(Screen { scale: 110, ..p });
        assert_eq!(scale.rects(), Err(LayoutError::Scale { monitor: 1 }));
        let three = ClientSet {
            virtuals: vec![v(1920, 1080, 1, Side::Right, Align::Start); 3],
            ..one
        };
        assert_eq!(three.rects(), Err(LayoutError::TooMany));
    }

    #[test]
    fn settings_are_json() {
        let s = Settings {
            enabled: true,
            virtuals: vec![v(2560, 1440, 1, Side::Above, Align::Center)],
        };
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(
            text,
            r#"{"enabled":true,"virtuals":[{"width":2560,"height":1440,"scale":100,"attach":{"to":1,"side":"above","align":"center"}}]}"#
        );
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }
}
