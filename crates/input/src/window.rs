//! Client side: window messages → input events.

use protocol::{InputEvent, MouseButton};
use windows::Win32::UI::WindowsAndMessaging::*;

fn lo_i16(v: isize) -> i32 {
    (v as u32 & 0xFFFF) as u16 as i16 as i32
}

fn hi_i16(v: isize) -> i32 {
    ((v as u32 >> 16) & 0xFFFF) as u16 as i16 as i32
}

fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

/// The client window's own message for Ctrl+Alt+Del on the host
/// (its shortcut, Ctrl+Alt+Shift+Del: Windows keeps Ctrl+Alt+Del).
pub const WM_SECURE_ATTENTION: u32 = WM_APP + 0x10;

/// Translates one window message of the client window into input events.
///
/// Mouse coordinates come from the client area (the window shows the host 1:1,
/// so client pixel == host pixel) and are signed, because during mouse capture
/// the pointer may be outside the window. Button messages produce only the
/// button event; the position is conveyed by `WM_MOUSEMOVE`. Auto-repeated
/// key-downs are forwarded as-is (injected keys do not auto-repeat on the host).
/// Returns an empty `Vec` for unrelated messages.
pub fn from_window_message(msg: u32, wparam: usize, lparam: isize) -> Vec<InputEvent> {
    let button = |button, down| vec![InputEvent::MouseButton { button, down }];
    match msg {
        WM_MOUSEMOVE => vec![InputEvent::MouseMove {
            x: lo_i16(lparam),
            y: hi_i16(lparam),
        }],
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => button(MouseButton::Left, true),
        WM_LBUTTONUP => button(MouseButton::Left, false),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => button(MouseButton::Right, true),
        WM_RBUTTONUP => button(MouseButton::Right, false),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => button(MouseButton::Middle, true),
        WM_MBUTTONUP => button(MouseButton::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_XBUTTONUP => {
            let which = match hiword(wparam) {
                1 => MouseButton::X1,
                2 => MouseButton::X2,
                _ => return Vec::new(),
            };
            button(which, msg != WM_XBUTTONUP)
        }
        WM_MOUSEWHEEL => vec![InputEvent::MouseWheel {
            dx: 0,
            dy: hiword(wparam) as i16,
        }],
        WM_MOUSEHWHEEL => vec![InputEvent::MouseWheel {
            dx: hiword(wparam) as i16,
            dy: 0,
        }],
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let l = lparam as u32;
            vec![InputEvent::Key {
                scancode: ((l >> 16) & 0xFF) as u16,
                extended: l & (1 << 24) != 0,
                down: l & (1 << 31) == 0,
            }]
        }
        WM_SECURE_ATTENTION => vec![InputEvent::SecureAttention],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xy(x: i32, y: i32) -> isize {
        (((y as u16 as u32) << 16) | (x as u16 as u32)) as i32 as isize
    }
    fn hi(v: u16) -> usize {
        (v as usize) << 16
    }
    fn key_lparam(scan: u32, ext: bool, up: bool) -> isize {
        let mut l = 1u32 | (scan << 16);
        if ext {
            l |= 1 << 24;
        }
        if up {
            l |= (1 << 30) | (1 << 31);
        }
        l as i32 as isize
    }
    fn key(scancode: u16, extended: bool, down: bool) -> [InputEvent; 1] {
        [InputEvent::Key {
            scancode,
            extended,
            down,
        }]
    }

    #[test]
    fn secure_attention_message_is_one_event() {
        assert_eq!(
            from_window_message(WM_SECURE_ATTENTION, 0, 0),
            vec![InputEvent::SecureAttention]
        );
    }

    #[test]
    fn mouse_move_positive_and_negative() {
        assert_eq!(
            from_window_message(WM_MOUSEMOVE, 0, xy(100, 2159)),
            [InputEvent::MouseMove { x: 100, y: 2159 }]
        );
        assert_eq!(
            from_window_message(WM_MOUSEMOVE, 0, xy(-5, -1)),
            [InputEvent::MouseMove { x: -5, y: -1 }]
        );
        assert_eq!(
            from_window_message(WM_MOUSEMOVE, 0, xy(-32768, 32767)),
            [InputEvent::MouseMove {
                x: -32768,
                y: 32767
            }]
        );
    }

    #[test]
    fn buttons() {
        let cases = [
            (WM_LBUTTONDOWN, MouseButton::Left, true),
            (WM_LBUTTONDBLCLK, MouseButton::Left, true),
            (WM_LBUTTONUP, MouseButton::Left, false),
            (WM_RBUTTONDOWN, MouseButton::Right, true),
            (WM_RBUTTONDBLCLK, MouseButton::Right, true),
            (WM_RBUTTONUP, MouseButton::Right, false),
            (WM_MBUTTONDOWN, MouseButton::Middle, true),
            (WM_MBUTTONDBLCLK, MouseButton::Middle, true),
            (WM_MBUTTONUP, MouseButton::Middle, false),
        ];
        for (msg, button, down) in cases {
            assert_eq!(
                from_window_message(msg, 0x0001, xy(10, 10)),
                [InputEvent::MouseButton { button, down }],
                "msg {msg:#x}"
            );
        }
    }

    #[test]
    fn x_buttons() {
        let cases = [
            (WM_XBUTTONDOWN, 1, MouseButton::X1, true),
            (WM_XBUTTONDBLCLK, 1, MouseButton::X1, true),
            (WM_XBUTTONUP, 1, MouseButton::X1, false),
            (WM_XBUTTONDOWN, 2, MouseButton::X2, true),
            (WM_XBUTTONDBLCLK, 2, MouseButton::X2, true),
            (WM_XBUTTONUP, 2, MouseButton::X2, false),
        ];
        for (msg, which, button, down) in cases {
            // LOWORD(wparam) carries MK_* key state and must be ignored.
            assert_eq!(
                from_window_message(msg, hi(which) | 0x0020, xy(1, 1)),
                [InputEvent::MouseButton { button, down }]
            );
        }
        assert!(from_window_message(WM_XBUTTONDOWN, hi(3), 0).is_empty());
    }

    #[test]
    fn wheel() {
        assert_eq!(
            from_window_message(WM_MOUSEWHEEL, hi(120), 0),
            [InputEvent::MouseWheel { dx: 0, dy: 120 }]
        );
        assert_eq!(
            from_window_message(WM_MOUSEWHEEL, hi((-240i16) as u16) | 0x0008, 0),
            [InputEvent::MouseWheel { dx: 0, dy: -240 }]
        );
        assert_eq!(
            from_window_message(WM_MOUSEHWHEEL, hi((-120i16) as u16), 0),
            [InputEvent::MouseWheel { dx: -120, dy: 0 }]
        );
        assert_eq!(
            from_window_message(WM_MOUSEHWHEEL, hi(30), 0),
            [InputEvent::MouseWheel { dx: 30, dy: 0 }]
        );
    }

    #[test]
    fn keys() {
        // 'A' = 0x1E.
        assert_eq!(
            from_window_message(WM_KEYDOWN, 0x41, key_lparam(0x1E, false, false)),
            key(0x1E, false, true)
        );
        assert_eq!(
            from_window_message(WM_KEYUP, 0x41, key_lparam(0x1E, false, true)),
            key(0x1E, false, false)
        );
        // Right Ctrl = E0 1D.
        assert_eq!(
            from_window_message(WM_KEYDOWN, 0xA3, key_lparam(0x1D, true, false)),
            key(0x1D, true, true)
        );
        // Left arrow = E0 4B, key-up.
        assert_eq!(
            from_window_message(WM_KEYUP, 0x25, key_lparam(0x4B, true, true)),
            key(0x4B, true, false)
        );
        // Alt via SYSKEY messages (Left Alt = 0x38).
        assert_eq!(
            from_window_message(WM_SYSKEYDOWN, 0x12, key_lparam(0x38, false, false)),
            key(0x38, false, true)
        );
        assert_eq!(
            from_window_message(WM_SYSKEYUP, 0x12, key_lparam(0x38, false, true)),
            key(0x38, false, false)
        );
        // Autorepeat (bit 30 set, bit 31 clear) is still a key-down.
        let repeat = (key_lparam(0x1E, false, false) as u32 | (1 << 30)) as i32 as isize;
        assert_eq!(
            from_window_message(WM_KEYDOWN, 0x41, repeat),
            key(0x1E, false, true)
        );
    }

    #[test]
    fn unrelated_messages_are_ignored() {
        assert!(from_window_message(WM_PAINT, 0, 0).is_empty());
        assert!(from_window_message(WM_CHAR, 0x41, 0).is_empty());
        assert!(from_window_message(WM_SIZE, 0, xy(10, 10)).is_empty());
    }
}
