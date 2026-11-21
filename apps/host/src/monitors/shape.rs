//! The shape of the virtual monitors to be up: the machine's or a client's own.

use serde::{Deserialize, Serialize};
use vdd::layout::{ClientSet, Screen, Settings, Virtual};

/// Monitor number → resolution, as one of the layouts wants them.
pub(super) type Wanted = Vec<(usize, u32, u32)>;

/// The virtual monitors to be up: the machine's next to its real ones, or a
/// client's own set alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Shape {
    /// The machine's (Lumen Host's «Мониторы»): monitors 2 and on.
    Machine(Vec<Virtual>),
    /// A client's own: monitor 1 too.
    Client(ClientSet),
}

impl Shape {
    /// The machine's settings: none up while they are off.
    pub(super) fn machine(s: &Settings) -> Shape {
        Shape::Machine(if s.enabled {
            s.virtuals.clone()
        } else {
            Vec::new()
        })
    }

    /// Monitor `n`'s resolution and scale.
    pub(super) fn screen(&self, n: usize) -> Option<Screen> {
        match self {
            Shape::Client(c) => c.screen(n),
            Shape::Machine(virtuals) => virtuals.get(n.checked_sub(2)?).map(|v| Screen {
                width: v.width,
                height: v.height,
                scale: v.scale,
            }),
        }
    }

    pub(super) fn wanted(&self) -> Wanted {
        let (first, count) = match self {
            Shape::Client(c) => (1, 1 + c.virtuals.len()),
            Shape::Machine(v) => (2, v.len()),
        };
        (first..first + count)
            .filter_map(|n| self.screen(n).map(|s| (n, s.width, s.height)))
            .collect()
    }
}
