//! Mouse pointer for the client: shape (sent on change and on request) and
//! hotspot position (sent every frame). The client draws it itself, so the
//! pointer never waits for the video. Positions are on the host's desktop;
//! with several monitors each one's duplication reports the pointer, and the
//! one that shows it is followed.

use capture::Pointer;
use protocol::{ControlMessage, CursorKind, CursorShape};

pub struct CursorFeed {
    /// Protocol shape id: starts at 1, increments on every shape change.
    id: u32,
    /// The last shape serial of each monitor's duplication.
    serials: [Option<u32>; protocol::MAX_STREAMS as usize],
    /// The stream (monitor) the pointer was last seen on.
    owner: Option<u8>,
    shape: Option<CursorShape>,
}

pub struct CursorUpdate {
    /// A new shape to send (fragmented, like video).
    pub new_shape: Option<CursorShape>,
    pub position: ControlMessage,
}

impl CursorFeed {
    pub fn new() -> Self {
        CursorFeed {
            id: 0,
            serials: Default::default(),
            owner: None,
            shape: None,
        }
    }

    /// The current shape, for a client that asked for `id` again.
    pub fn shape(&self, id: u32) -> Option<&CursorShape> {
        self.shape.as_ref().filter(|s| s.id == id)
    }

    /// Desktop source: the pointer the duplication of monitor `stream` (at
    /// `origin` on the desktop) reported. `None`: the pointer is on another
    /// monitor (this one only says it is not here).
    pub fn desktop(&mut self, p: &Pointer, stream: u8, origin: (i32, i32)) -> Option<CursorUpdate> {
        if p.visible {
            self.owner = Some(stream);
        } else if self.owner.is_some_and(|o| o != stream) {
            return None;
        }
        let mut new_shape = None;
        let serial = &mut self.serials[usize::from(stream)];
        if let Some(s) = &p.shape
            && *serial != Some(p.shape_serial)
        {
            *serial = Some(p.shape_serial);
            let kind = match s.kind {
                1 => CursorKind::Monochrome,
                4 => CursorKind::MaskedColor,
                _ => CursorKind::Color,
            };
            self.id += 1;
            let shape = CursorShape {
                id: self.id,
                kind,
                width: s.width as u16,
                height: s.height as u16,
                pitch: s.pitch,
                hot_x: s.hot_x as u16,
                hot_y: s.hot_y as u16,
                data: s.data.clone(),
            };
            new_shape = Some(shape.clone());
            self.shape = Some(shape);
        }
        let (hx, hy) = self
            .shape
            .as_ref()
            .map_or((0, 0), |s| (i32::from(s.hot_x), i32::from(s.hot_y)));
        Some(CursorUpdate {
            new_shape,
            position: ControlMessage::CursorPosition {
                x: origin.0 + p.x + hx,
                y: origin.1 + p.y + hy,
                visible: p.visible && self.shape.is_some(),
                shape_id: self.id,
            },
        })
    }

    /// Synthetic source: an arrow circling the centre (exercises the client's drawing).
    pub fn synthetic(&mut self, frame: u32, width: u32, height: u32) -> CursorUpdate {
        let new_shape = self.shape.is_none().then(|| {
            self.id = 1;
            let s = synthetic_arrow(self.id);
            self.shape = Some(s.clone());
            s
        });
        let a = f64::from(frame) * 0.02;
        let r = f64::from(height) / 3.0;
        CursorUpdate {
            new_shape,
            position: ControlMessage::CursorPosition {
                x: (f64::from(width) / 2.0 + r * a.cos()) as i32,
                y: (f64::from(height) / 2.0 + r * a.sin()) as i32,
                visible: true,
                shape_id: self.id,
            },
        }
    }
}

/// 20×32 colour arrow: white with a black outline, hotspot at the tip.
fn synthetic_arrow(id: u32) -> CursorShape {
    let (w, h) = (20usize, 32usize);
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        // Left edge vertical, right edge at slope ~0.6 down to row 22, then a stem.
        let right = if y < 22 { y * 6 / 10 } else { 4 + (y - 22) / 3 };
        for x in 0..=right.min(w - 1) {
            let edge = x == 0 || x == right || y == 0 || y == h - 1;
            let v = if edge { 0 } else { 255 };
            let i = (y * w + x) * 4;
            data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    CursorShape {
        id,
        kind: CursorKind::Color,
        width: w as u16,
        height: h as u16,
        pitch: (w * 4) as u32,
        hot_x: 0,
        hot_y: 0,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture::PointerShape;

    fn pointer(visible: bool, x: i32, y: i32, serial: u32) -> Pointer {
        Pointer {
            visible,
            x,
            y,
            shape_serial: serial,
            shape: Some(PointerShape {
                kind: 2,
                width: 2,
                height: 2,
                pitch: 8,
                hot_x: 1,
                hot_y: 1,
                data: vec![0; 16],
            }),
        }
    }

    fn position(u: &CursorUpdate) -> (i32, i32, bool) {
        match u.position {
            ControlMessage::CursorPosition { x, y, visible, .. } => (x, y, visible),
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_monitor_with_the_pointer_is_followed() {
        let mut c = CursorFeed::new();
        // On monitor 1 (at 0, 0): desktop position = monitor position + hotspot.
        let u = c.desktop(&pointer(true, 10, 20, 1), 0, (0, 0)).unwrap();
        assert!(u.new_shape.is_some());
        assert_eq!(position(&u), (11, 21, true));
        // Moved onto monitor 2 (at 3840, 360): monitor 1 says "not here" — ignored.
        let u = c.desktop(&pointer(true, 5, 5, 1), 1, (3840, 360)).unwrap();
        assert!(
            u.new_shape.is_some(),
            "its duplication's shape is new to us"
        );
        assert_eq!(position(&u), (3846, 366, true));
        assert!(c.desktop(&pointer(false, 0, 0, 1), 0, (0, 0)).is_none());
        // The same shape again on monitor 2: no new shape.
        let u = c.desktop(&pointer(true, 6, 5, 1), 1, (3840, 360)).unwrap();
        assert!(u.new_shape.is_none());
        // Hidden where it is (a program hid it): hidden.
        let u = c.desktop(&pointer(false, 6, 5, 1), 1, (3840, 360)).unwrap();
        assert!(!position(&u).2);
    }
}
