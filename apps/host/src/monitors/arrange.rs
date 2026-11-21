//! `displays arrange`: placing the virtual monitors and setting their scale.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use vdd::display::{self, Target};
use vdd::layout;

use super::Up;
use super::shape::Shape;

/// How long Windows may take to turn a new monitor on.
const ACTIVE_WITHIN: Duration = Duration::from_secs(10);

/// What `displays arrange` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    /// Where the monitors sit (as SYSTEM: the machine's layout).
    Place,
    /// Their scale (as the user: theirs).
    Scale,
    Both,
}

/// What `displays arrange` gets from the service.
#[derive(Serialize, Deserialize)]
pub(super) struct Plan {
    pub(super) up: Vec<Up>,
    pub(super) shape: Shape,
    /// Nobody in session: the client's monitors off, kept in the driver
    /// (another driver's display on instead, if there is one).
    #[serde(default)]
    pub(super) park: bool,
    /// None was added: ones that are off (parked) are turned on at once,
    /// not after the wait for Windows to do it.
    #[serde(default)]
    pub(super) wake: bool,
}

/// Puts the virtual monitors `up` where the plan says — among the real
/// monitors, or a client's own alone (others turned off) — and/or gives them
/// their scale. Acts on this process's session; the scale is this user's.
/// Returns notes on what did not take.
pub(super) fn arrange(plan: &Plan, part: Part) -> Result<Vec<String>> {
    let up = &plan.up;
    if up.is_empty() {
        return Ok(Vec::new());
    }
    let targets: Vec<Target> = up.iter().map(|u| u.target).collect();
    // Parked: off if another display takes over; kept on (the only display,
    // seen through Parsec too), placed and scaled as in session — Lumen Host
    // may change it then.
    let mut notes = Vec::new();
    if plan.park {
        if part == Part::Scale {
            if !display::inactive(&targets)?.is_empty() {
                return Ok(notes);
            }
        } else if display::park(&targets)? {
            return Ok(vec!["parked: off, another display on".into()]);
        } else {
            notes.push("parked: kept on (no other display)".into());
        }
    }
    if plan.wake {
        let off = display::inactive(&targets)?;
        if !off.is_empty() {
            display::activate(&off)?;
        }
    }
    display::wait_active(&targets, ACTIVE_WITHIN)?;
    if part != Part::Scale {
        let modes: Vec<_> = up
            .iter()
            .filter_map(|u| {
                let s = plan.shape.screen(u.monitor)?;
                Some((u.target, s.width, s.height, layout::REFRESH_HZ))
            })
            .collect();
        display::set_resolutions(&modes)?;
        let placed: Vec<(Target, layout::Rect)> = match &plan.shape {
            Shape::Client(c) => {
                let rects = c.rects()?;
                // Monitor 1 first: it becomes the primary one. Nothing else
                // on: a physical monitor that is there is not shown.
                display::keep_only(&targets)?;
                up.iter()
                    .filter_map(|u| Some((u.target, *rects.get(u.monitor - 1)?)))
                    .collect()
            }
            Shape::Machine(virtuals) => {
                // Without a real primary monitor there is nothing to be next to.
                display::primary(Some(targets[0].adapter))?;
                let real: Vec<layout::Rect> = display::real()?.iter().map(|a| a.rect).collect();
                let rects = layout::place(&real, virtuals)?;
                up.iter()
                    .filter_map(|u| Some((u.target, *rects.get(u.monitor.checked_sub(2)?)?)))
                    .collect()
            }
        };
        display::arrange(&placed)?;
    }
    if part != Part::Place {
        for u in up {
            let Some(want) = plan.shape.screen(u.monitor).map(|s| s.scale) else {
                continue;
            };
            match display::set_scale(u.target, want) {
                Ok(got) if got == want => {}
                Ok(got) => notes.push(format!(
                    "monitor {}: scale {want} % asked, Windows has {got} %",
                    u.monitor
                )),
                Err(e) => notes.push(format!("monitor {}: scale {want} %: {e}", u.monitor)),
            }
        }
    }
    Ok(notes)
}

/// `lumen-host displays arrange --plan <json> --part ...` (the service runs it).
pub fn arrange_plan(plan: &str, part: Part) -> Result<()> {
    let plan: Plan = serde_json::from_str(plan).context("plan")?;
    for note in arrange(&plan, part)? {
        println!("{note}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vdd::contract::Luid;
    use vdd::layout::{Align, Attach, ClientSet, Screen, Settings, Side, Virtual};

    #[test]
    fn the_plan_goes_through_the_command_line() {
        let plan = Plan {
            up: vec![Up {
                monitor: 2,
                target: Target {
                    adapter: Luid {
                        low: 0x03dc2629,
                        high: 0,
                    },
                    id: 256,
                },
                width: 2560,
                height: 1440,
            }],
            shape: Shape::Machine(vec![Virtual {
                width: 2560,
                height: 1440,
                scale: 150,
                attach: Attach {
                    to: 1,
                    side: Side::Right,
                    align: Align::Center,
                },
            }]),
            park: true,
            wake: true,
        };
        let text = serde_json::to_string(&plan).unwrap();
        assert!(!text.contains('\n'));
        let back: Plan = serde_json::from_str(&text).unwrap();
        assert_eq!(back.up[0].target, plan.up[0].target);
        assert_eq!(back.up[0].monitor, 2);
        assert_eq!(back.shape, plan.shape);
        assert_eq!(back.shape.wanted(), [(2, 2560, 1440)]);
        assert!(back.park && back.wake);
        // A plan of an older service: neither.
        let old = text.replace(",\"park\":true,\"wake\":true", "");
        let back: Plan = serde_json::from_str(&old).unwrap();
        assert!(!back.park && !back.wake);
    }

    fn set_virtual() -> Virtual {
        Virtual {
            width: 1920,
            height: 1080,
            scale: 100,
            attach: Attach {
                to: 1,
                side: Side::Right,
                align: Align::Start,
            },
        }
    }

    #[test]
    fn a_client_plan() {
        let set = ClientSet {
            primary: Screen {
                width: 3440,
                height: 1440,
                scale: 125,
            },
            virtuals: vec![Virtual {
                width: 1920,
                height: 1080,
                scale: 100,
                attach: Attach {
                    to: 1,
                    side: Side::Left,
                    align: Align::End,
                },
            }],
        };
        let shape = Shape::Client(set);
        assert_eq!(shape.wanted(), [(1, 3440, 1440), (2, 1920, 1080)]);
        let plan = Plan {
            up: Vec::new(),
            shape: shape.clone(),
            park: false,
            wake: false,
        };
        let back: Plan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        assert_eq!(back.shape, shape);
        assert_eq!(back.shape.screen(1).unwrap().scale, 125);
        assert_eq!(back.shape.screen(2).unwrap().width, 1920);
        assert_eq!(back.shape.screen(3), None);
        // The machine's, turned off: none up.
        let two = vec![set_virtual(); 2];
        let off = Settings {
            enabled: false,
            virtuals: two.clone(),
        };
        assert_eq!(Shape::machine(&off).wanted(), []);
        let on = Settings {
            enabled: true,
            virtuals: two,
        };
        assert_eq!(
            Shape::machine(&on).wanted(),
            [(2, 1920, 1080), (3, 1920, 1080)]
        );
    }
}
