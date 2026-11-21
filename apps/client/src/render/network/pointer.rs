//! The live video: the host's pointer, drawn by the client into the frame
//! at the local mouse, one function for every view.

use super::*;

/// On top of the input resend timeout (about the RTT), how long after a local
/// mouse move the host's pointer reports still count as its echo: the host
/// reports the pointer when it moves and with every frame.
pub(super) const ECHO_MARGIN: std::time::Duration = std::time::Duration::from_millis(40);

/// Where to draw the pointer: at the local mouse while the host's reports are
/// only echoes of its moves (they arrive a round trip late, and drawing them
/// would pull the pointer back and forth between the two: a trail), else where
/// the host reports it (a program there moved it).
pub(super) fn pointer_position(
    local: Option<(i32, i32, std::time::Instant)>,
    (hx, hy, host_t): (i32, i32, std::time::Instant),
    echo: std::time::Duration,
) -> (i32, i32) {
    match local {
        Some((lx, ly, t)) if host_t <= t + echo => (lx, ly),
        _ => (hx, hy),
    }
}

/// The host's pointer, drawn by us, and the local mouse.
pub(super) struct Pointer {
    pub(super) cursor: CursorOverlay,
    /// Last host pointer position (hotspot), visibility, shape, and when it changed.
    pub(super) host_pointer: Option<(i32, i32, bool, u32, std::time::Instant)>,
    /// Last local mouse position over the window and when it was seen.
    pub(super) local_pointer: Option<(i32, i32, std::time::Instant)>,
    pub(super) shape_requested: Option<std::time::Instant>,
    /// Where the pointer was drawn last (hotspot) and with which shape.
    pub(super) drawn_at: Option<(i32, i32, u32)>,
    /// The local mouse moved since the last `update`.
    pub(super) local_moved: bool,
}

impl NetworkVideo {
    /// Cursor updates from the host. True if the main window should redraw.
    pub(super) fn take_pointer_news(&mut self) -> Result<bool> {
        let mut dirty = false;
        for ev in self.conn.cursor.try_iter() {
            match ev {
                CursorEvent::Shape(s) => {
                    dirty = true;
                    self.mon.dirty_streams = u8::MAX;
                    println!(
                        "cursor shape {}: {:?} {}x{}, hotspot ({}, {})",
                        s.id, s.kind, s.width, s.height, s.hot_x, s.hot_y
                    );
                    self.pointer.cursor.set_shape(&s)?;
                    self.pointer.shape_requested = None;
                }
                CursorEvent::Position {
                    x,
                    y,
                    visible,
                    shape_id,
                } => {
                    let changed = self.pointer.host_pointer.is_none_or(|(hx, hy, hv, hs, _)| {
                        (hx, hy, hv, hs) != (x, y, visible, shape_id)
                    });
                    // The host repeats the position with every frame of
                    // every monitor: only a change is redrawn.
                    if changed {
                        let mut touched = self.streams_at(x, y);
                        if let Some((hx, hy, ..)) = self.pointer.host_pointer {
                            touched |= self.streams_at(hx, hy);
                        }
                        dirty |= self.main_window_shows(touched);
                        self.mon.dirty_streams |= touched;
                        self.pointer.host_pointer =
                            Some((x, y, visible, shape_id, std::time::Instant::now()));
                    }
                }
            }
        }
        Ok(dirty)
    }

    /// The monitor under a point of the host's desktop, a bit (none: 0):
    /// whose window redraws when the pointer leaves or comes there.
    pub(super) fn streams_at(&self, x: i32, y: i32) -> u8 {
        self.mon
            .monitors
            .iter()
            .find(|m| {
                (m.x..m.x + m.width as i32).contains(&x)
                    && (m.y..m.y + m.height as i32).contains(&y)
            })
            .map_or(0, |m| 1 << m.stream)
    }

    /// The main window shows one of `streams` (a bit each): the pointer there
    /// is its news. Only mode 3 has monitors shown elsewhere; the others
    /// redraw as ever.
    pub(super) fn main_window_shows(&self, streams: u8) -> bool {
        self.mon.mode != Mode::Windows || streams & (1 << self.shown_stream()) != 0
    }

    /// A desktop point (the host's pointer) → where it is drawn in the render
    /// target of `monitor`'s own window (mode 3), or of the main window: the
    /// picture (one monitor) or the window (several). On nothing shown there: none.
    pub(super) fn to_target(
        &self,
        x: i32,
        y: i32,
        monitor: Option<&protocol::Monitor>,
    ) -> Option<(i32, i32)> {
        if let Some(m) = monitor {
            let (px, py) = (x - m.x, y - m.y);
            return ((0..m.width as i32).contains(&px) && (0..m.height as i32).contains(&py))
                .then_some((px, py));
        }
        let (cx, cy) = self.mon.scene.to_canvas(x, y)?;
        Some(if self.mon.scene.is_multi() {
            self.fit().picture_to_client(cx, cy)
        } else {
            (cx, cy)
        })
    }

    /// Draws the pointer, in every view: at the local mouse position while
    /// the user moves it (no round trip), otherwise where the host reports it
    /// (see [`pointer_position`]). A shape that did not come is asked for.
    pub(super) fn draw_pointer(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        monitor: Option<&protocol::Monitor>,
    ) {
        let Some((hx, hy, visible, shape_id, host_t)) = self.pointer.host_pointer else {
            return;
        };
        if self.pointer.cursor.shape_id() != Some(shape_id) {
            if self
                .pointer
                .shape_requested
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(200))
            {
                self.conn
                    .request(ControlMessage::RequestCursorShape { id: shape_id });
                self.pointer.shape_requested = Some(std::time::Instant::now());
            }
            return;
        }
        if !visible {
            return;
        }
        let echo = self.conn.counters.lock().unwrap().input_rto + ECHO_MARGIN;
        let (x, y) = pointer_position(self.pointer.local_pointer, (hx, hy, host_t), echo);
        let Some((x, y)) = self.to_target(x, y, monitor) else {
            return;
        };
        self.pointer.cursor.draw(ctx, rtv, x, y);
        self.pointer.drawn_at = Some((x, y, shape_id));
    }

    /// A picture pixel whose exact codes are known after `render` (cursor
    /// check): the synthetic host arrow (`lumen-host --source synthetic`) is
    /// opaque white two pixels right and ten below its tip.
    pub fn probe(&self) -> Option<(i32, i32, [u16; 3])> {
        let (x, y, _) = self.pointer.drawn_at?;
        Some((x + 2, y + 10, [1023, 1023, 1023]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const ECHO: Duration = Duration::from_millis(90);

    #[test]
    fn host_echoes_of_local_moves_do_not_pull_the_pointer_back() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        // Moving: each host report is the position from a round trip ago.
        let mut local = None;
        for i in 0..20u64 {
            local = Some((100 + i as i32 * 10, 100, ms(i * 8)));
            let host = (100 + i.saturating_sub(6) as i32 * 10, 100, ms(i * 8 + 1));
            assert_eq!(
                pointer_position(local, host, ECHO),
                (100 + i as i32 * 10, 100)
            );
        }
        // The last echo, one round trip after the mouse stopped: still local.
        assert_eq!(
            pointer_position(local, (290, 100, ms(152 + 50)), ECHO),
            (290, 100)
        );
    }

    #[test]
    fn host_moves_win_once_the_local_mouse_is_still() {
        let t0 = Instant::now();
        let local = Some((290, 100, t0));
        // A program on the host moved the pointer well after the user stopped.
        let host = (500, 300, t0 + Duration::from_millis(400));
        assert_eq!(pointer_position(local, host, ECHO), (500, 300));
        // No local mouse over the window yet: the host's position.
        assert_eq!(pointer_position(None, (7, 8, t0), ECHO), (7, 8));
    }
}
