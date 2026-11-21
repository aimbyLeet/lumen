//! Host side: input events → `SendInput`.

use anyhow::{Result, bail};
use protocol::{InputEvent, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    XBUTTON1, XBUTTON2,
};

/// Rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Where the pointer may go on the host's virtual desktop (`output`), and the
/// virtual desktop itself. Pointer positions are desktop coordinates (the
/// primary monitor at 0, 0: protocol v10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub output: Rect,
    pub virtual_desktop: Rect,
}

impl Geometry {
    /// Builds the geometry for `output`, reading the virtual desktop from
    /// `GetSystemMetrics`. The process must be per-monitor DPI aware, otherwise
    /// the metrics are virtualised.
    pub fn current_for_output(output: Rect) -> Self {
        // SAFETY: GetSystemMetrics has no preconditions.
        let virtual_desktop = unsafe {
            Rect {
                left: GetSystemMetrics(SM_XVIRTUALSCREEN),
                top: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        };
        Self {
            output,
            virtual_desktop,
        }
    }

    /// The whole virtual desktop as it is now (every monitor of the host).
    pub fn current_desktop() -> Self {
        let g = Self::current_for_output(Rect {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        });
        Self {
            output: g.virtual_desktop,
            ..g
        }
    }
}

/// Maps pixel `v` in `0..extent` to a value in `0..=65535`.
///
/// Windows turns a normalised value back into a pixel with a rule that differs
/// between versions: `floor(n * w / 65536)` or `round(n * (w - 1) / 65535)`.
/// We aim at `f = v + 0.1 + 0.8 * v / (w - 1)` in the first model's units:
/// `f - v` stays within `0.1..=0.9` (floor gives `v`), and the second model
/// sees about `f - f / w`, i.e. `v + 0.1` at the left edge down to `v - 0.1`
/// at the right edge (round gives `v`). Margin >= 0.1 px against the
/// quantisation of `n` (~0.03 px at 4K).
fn to_unit(v: i64, extent: i64) -> i32 {
    let w = extent.max(1) as f64;
    let f = v as f64 + 0.1 + 0.8 * v as f64 / (w - 1.0).max(1.0);
    ((f * 65536.0 / w).round() as i64).clamp(0, 65535) as i32
}

/// Desktop pixel → `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`
/// coordinates in `0..=65535`. Positions outside `output` are clamped to it.
pub fn normalise(geometry: &Geometry, x: i32, y: i32) -> (i32, i32) {
    let o = geometry.output;
    let v = geometry.virtual_desktop;
    let clamp = |p: i32, start: i32, len: i32| {
        i64::from(p).clamp(
            i64::from(start),
            i64::from(start) + i64::from(len - 1).max(0),
        )
    };
    let ax = clamp(x, o.left, o.width);
    let ay = clamp(y, o.top, o.height);
    (
        to_unit(ax - v.left as i64, v.width as i64),
        to_unit(ay - v.top as i64, v.height as i64),
    )
}

fn key_input(scancode: u16, extended: bool, down: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scancode,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The `INPUT` records for one event (no side effects).
pub fn to_inputs(geometry: &Geometry, event: &InputEvent) -> Vec<INPUT> {
    match *event {
        InputEvent::MouseMove { x, y } => {
            let (nx, ny) = normalise(geometry, x, y);
            vec![mouse_input(
                nx,
                ny,
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            )]
        }
        InputEvent::MouseButton { button, down } => {
            let (flags, data) = match (button, down) {
                (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
                (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
                (MouseButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
                (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
                (MouseButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
            };
            vec![mouse_input(0, 0, data.into(), flags)]
        }
        InputEvent::MouseWheel { dx, dy } => {
            let mut out = Vec::new();
            if dy != 0 {
                out.push(mouse_input(0, 0, dy as i32 as u32, MOUSEEVENTF_WHEEL));
            }
            if dx != 0 {
                out.push(mouse_input(0, 0, dx as i32 as u32, MOUSEEVENTF_HWHEEL));
            }
            out
        }
        InputEvent::Key {
            scancode,
            extended,
            down,
        } => vec![key_input(scancode, extended, down)],
        // SendInput cannot make it: the host asks the service (SendSAS).
        InputEvent::SecureAttention => Vec::new(),
        // Not input: the host puts it on the clipboard (apps/host clipboard).
        InputEvent::Clipboard(_) | InputEvent::File(_) => Vec::new(),
    }
}

type SendFn = Box<dyn FnMut(&[INPUT]) -> Result<()> + Send>;

fn send_input(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of fully initialised INPUT records.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if (sent as usize) != inputs.len() {
        // Typically UIPI: the foreground window is elevated and we are not.
        bail!(
            "SendInput injected {sent} of {} events: {}",
            inputs.len(),
            windows::core::Error::from_thread()
        );
    }
    Ok(())
}

/// Presses or releases one key on this machine by scan code (checks and tests).
pub fn send_key(scancode: u16, extended: bool, down: bool) -> Result<()> {
    send_input(&[key_input(scancode, extended, down)])
}

/// Replays [`InputEvent`]s on this machine and remembers what is held down.
pub struct Injector {
    geometry: Geometry,
    /// Look at the virtual desktop again before every pointer move (monitors
    /// come and go during a session).
    follow_desktop: bool,
    send: SendFn,
    keys: Vec<(u16, bool)>,
    buttons: Vec<MouseButton>,
}

impl Injector {
    pub fn new(geometry: Geometry) -> Self {
        Self::with_sender(geometry, Box::new(send_input))
    }

    /// Over the whole virtual desktop, as it is at each pointer move.
    pub fn for_desktop() -> Self {
        let mut inj = Self::new(Geometry::current_desktop());
        inj.follow_desktop = true;
        inj
    }

    /// Like [`new`](Self::new) but with a custom sink instead of `SendInput`
    /// (used by tests so nothing is injected).
    pub fn with_sender(geometry: Geometry, send: SendFn) -> Self {
        Self {
            geometry,
            follow_desktop: false,
            send,
            keys: Vec::new(),
            buttons: Vec::new(),
        }
    }

    /// Injects one event and updates the pressed-state bookkeeping.
    pub fn apply(&mut self, event: &InputEvent) -> Result<()> {
        if self.follow_desktop && matches!(event, InputEvent::MouseMove { .. }) {
            self.geometry = Geometry::current_desktop();
        }
        let inputs = to_inputs(&self.geometry, event);
        (self.send)(&inputs)?;
        match *event {
            InputEvent::Key {
                scancode,
                extended,
                down,
            } => {
                let k = (scancode, extended);
                self.keys.retain(|&p| p != k);
                if down {
                    self.keys.push(k);
                }
            }
            InputEvent::MouseButton { button, down } => {
                self.buttons.retain(|&b| b != button);
                if down {
                    self.buttons.push(button);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Releases everything still pressed (call on disconnect so nothing stays
    /// stuck). Attempts every release even if one fails; returns the first error.
    pub fn release_all(&mut self) -> Result<()> {
        let mut first_err = None;
        for (scancode, extended) in std::mem::take(&mut self.keys) {
            let ev = InputEvent::Key {
                scancode,
                extended,
                down: false,
            };
            if let Err(e) = (self.send)(&to_inputs(&self.geometry, &ev)) {
                first_err.get_or_insert(e);
            }
        }
        for button in std::mem::take(&mut self.buttons) {
            let ev = InputEvent::MouseButton {
                button,
                down: false,
            };
            if let Err(e) = (self.send)(&to_inputs(&self.geometry, &ev)) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Number of keys and buttons currently held down.
    pub fn pressed_count(&self) -> usize {
        self.keys.len() + self.buttons.len()
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const fn r(left: i32, top: i32, width: i32, height: i32) -> Rect {
        Rect {
            left,
            top,
            width,
            height,
        }
    }

    /// Windows' two known unit → pixel conversions; both must give the pixel back.
    fn denormalise(n: i32, extent: i32) -> [i32; 2] {
        let n = n as i64;
        let e = extent as i64;
        [
            (n * e / 65536) as i32,
            ((n * (e - 1) + 32767) / 65535) as i32,
        ]
    }

    fn check_geometry(g: &Geometry) {
        let o = g.output;
        let v = g.virtual_desktop;
        let xs = [0, 1, o.width / 3, o.width / 2, o.width - 2, o.width - 1];
        let ys = [0, 1, o.height / 3, o.height / 2, o.height - 2, o.height - 1];
        for &x in &xs {
            for &y in &ys {
                let (nx, ny) = normalise(g, o.left + x, o.top + y);
                assert!((0..=65535).contains(&nx) && (0..=65535).contains(&ny));
                for d in denormalise(nx, v.width) {
                    assert_eq!(d + v.left, o.left + x, "x of ({x},{y}) in {g:?}");
                }
                for d in denormalise(ny, v.height) {
                    assert_eq!(d + v.top, o.top + y, "y of ({x},{y}) in {g:?}");
                }
            }
        }
    }

    #[test]
    fn normalise_every_pixel_round_trips() {
        for extent in [640, 1080, 1920, 2160, 2560, 3840, 6400, 7680] {
            for px in 0..extent {
                let n = to_unit(px as i64, extent as i64);
                for d in denormalise(n, extent) {
                    assert_eq!(d, px, "extent {extent}");
                }
            }
        }
    }

    #[test]
    fn normalise_single_monitor() {
        let m = r(0, 0, 3840, 2160);
        check_geometry(&Geometry {
            output: m,
            virtual_desktop: m,
        });
    }

    #[test]
    fn normalise_negative_offset_multi_monitor() {
        // Streamed 4K monitor left of and above the primary; desktop spans both.
        check_geometry(&Geometry {
            output: r(-3840, -400, 3840, 2160),
            virtual_desktop: r(-3840, -400, 3840 + 2560, 2160),
        });
        // Streamed monitor on the right, vertically offset.
        check_geometry(&Geometry {
            output: r(1920, -200, 2560, 1440),
            virtual_desktop: r(0, -200, 1920 + 2560, 1640),
        });
    }

    #[test]
    fn normalise_clamps_to_output() {
        let g = Geometry {
            output: r(-1920, 0, 1920, 1080),
            virtual_desktop: r(-1920, 0, 3840, 1080),
        };
        // Desktop coordinates: the output spans -1920..=-1 × 0..=1079.
        assert_eq!(normalise(&g, -9999, -50), normalise(&g, -1920, 0));
        assert_eq!(normalise(&g, 9999, 9999), normalise(&g, -1, 1079));
        assert_ne!(normalise(&g, -1920, 0), normalise(&g, -1919, 0));
    }

    fn flags_of(i: &INPUT) -> (INPUT_TYPE, u32, u32, i32, i32) {
        // SAFETY: test helper reading the union member matching `r#type`.
        unsafe {
            if i.r#type == INPUT_MOUSE {
                let m = i.Anonymous.mi;
                (i.r#type, m.dwFlags.0, m.mouseData, m.dx, m.dy)
            } else {
                let k = i.Anonymous.ki;
                (i.r#type, k.dwFlags.0, k.wScan as u32, k.wVk.0 as i32, 0)
            }
        }
    }

    fn geo() -> Geometry {
        let m = r(0, 0, 1920, 1080);
        Geometry {
            output: m,
            virtual_desktop: m,
        }
    }

    #[test]
    fn to_inputs_mouse_move() {
        let v = to_inputs(&geo(), &InputEvent::MouseMove { x: 0, y: 1079 });
        assert_eq!(v.len(), 1);
        let (t, f, d, dx, dy) = flags_of(&v[0]);
        assert_eq!(t, INPUT_MOUSE);
        assert_eq!(
            f,
            (MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).0
        );
        assert_eq!(d, 0);
        assert_eq!((dx, dy), normalise(&geo(), 0, 1079));
    }

    #[test]
    fn to_inputs_buttons() {
        let cases = [
            (MouseButton::Left, true, MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false, MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true, MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false, MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true, MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false, MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::X1, true, MOUSEEVENTF_XDOWN, 1),
            (MouseButton::X1, false, MOUSEEVENTF_XUP, 1),
            (MouseButton::X2, true, MOUSEEVENTF_XDOWN, 2),
            (MouseButton::X2, false, MOUSEEVENTF_XUP, 2),
        ];
        for (button, down, flag, data) in cases {
            let v = to_inputs(&geo(), &InputEvent::MouseButton { button, down });
            assert_eq!(v.len(), 1);
            let (t, f, d, _, _) = flags_of(&v[0]);
            assert_eq!((t, f, d), (INPUT_MOUSE, flag.0, data));
        }
    }

    #[test]
    fn to_inputs_wheel() {
        let v = to_inputs(&geo(), &InputEvent::MouseWheel { dx: 0, dy: -120 });
        assert_eq!(v.len(), 1);
        let (_, f, d, _, _) = flags_of(&v[0]);
        assert_eq!((f, d), (MOUSEEVENTF_WHEEL.0, (-120i32) as u32));

        let v = to_inputs(&geo(), &InputEvent::MouseWheel { dx: 240, dy: 0 });
        assert_eq!(v.len(), 1);
        let (_, f, d, _, _) = flags_of(&v[0]);
        assert_eq!((f, d), (MOUSEEVENTF_HWHEEL.0, 240));

        let v = to_inputs(&geo(), &InputEvent::MouseWheel { dx: 120, dy: 120 });
        assert_eq!(v.len(), 2);
        assert!(to_inputs(&geo(), &InputEvent::MouseWheel { dx: 0, dy: 0 }).is_empty());
    }

    #[test]
    fn to_inputs_keys() {
        let key = |scancode, extended, down| InputEvent::Key {
            scancode,
            extended,
            down,
        };
        let sc = KEYEVENTF_SCANCODE.0;
        let ext = KEYEVENTF_EXTENDEDKEY.0;
        let up = KEYEVENTF_KEYUP.0;
        let cases = [
            (key(0x1E, false, true), sc),
            (key(0x1E, false, false), sc | up),
            (key(0x1D, true, true), sc | ext),
            (key(0x4B, true, false), sc | ext | up),
        ];
        for (ev, flags) in cases {
            let v = to_inputs(&geo(), &ev);
            assert_eq!(v.len(), 1);
            let (t, f, scan, vk, _) = flags_of(&v[0]);
            let InputEvent::Key { scancode, .. } = ev else {
                unreachable!()
            };
            assert_eq!(
                (t, f, scan, vk),
                (INPUT_KEYBOARD, flags, scancode as u32, 0)
            );
        }
    }

    #[test]
    fn release_all_bookkeeping() {
        let log: Arc<Mutex<Vec<(u32, u32)>>> = Arc::default();
        let sink = log.clone();
        let mut inj = Injector::with_sender(
            geo(),
            Box::new(move |inputs| {
                for i in inputs {
                    let (_, f, d, _, _) = flags_of(i);
                    sink.lock().unwrap().push((f, d));
                }
                Ok(())
            }),
        );
        let key = |scancode, extended, down| InputEvent::Key {
            scancode,
            extended,
            down,
        };
        inj.apply(&key(0x1D, false, true)).unwrap(); // Ctrl
        inj.apply(&key(0x1D, true, true)).unwrap(); // Right Ctrl (distinct)
        inj.apply(&key(0x1D, true, true)).unwrap(); // repeat, not double counted
        inj.apply(&key(0x1E, false, true)).unwrap();
        inj.apply(&key(0x1E, false, false)).unwrap();
        inj.apply(&InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        })
        .unwrap();
        inj.apply(&InputEvent::MouseButton {
            button: MouseButton::X2,
            down: true,
        })
        .unwrap();
        inj.apply(&InputEvent::MouseMove { x: 5, y: 5 }).unwrap();
        assert_eq!(inj.pressed_count(), 4);

        log.lock().unwrap().clear();
        inj.release_all().unwrap();
        assert_eq!(inj.pressed_count(), 0);
        let sent = log.lock().unwrap().clone();
        assert_eq!(sent.len(), 4);
        let up = KEYEVENTF_KEYUP.0;
        assert_eq!(sent.iter().filter(|(f, _)| f & up != 0).count(), 2);
        assert!(sent.contains(&(MOUSEEVENTF_LEFTUP.0, 0)));
        assert!(sent.contains(&(MOUSEEVENTF_XUP.0, 2)));

        // Second call is a no-op.
        log.lock().unwrap().clear();
        inj.release_all().unwrap();
        assert!(log.lock().unwrap().is_empty());
    }
}
