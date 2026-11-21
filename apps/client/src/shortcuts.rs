//! System shortcuts to the host (`--shortcuts host`, the default in a session).
//!
//! Windows handles Win, Alt+Tab, Alt+Esc, Ctrl+Esc, Ctrl+Shift+Esc and the
//! like before any window sees them. A low-level keyboard hook takes every key
//! while the video window is in the foreground: Windows does not see it, the
//! window gets it as a posted [`WM_HOOK_KEY`] and sends it to the host like any
//! other key. All keys take this path, so their order is kept. The client's own
//! shortcuts (Ctrl+Alt+Shift+Q / F) come as [`WM_HOOK_SHORTCUT`]. Ctrl+Alt+Del
//! and Win+L stay local: Windows handles them before any hook.
//!
//! The same hook counts the pastes made in other programs (Ctrl+V,
//! Shift+Insert while the video window is not in front, [`pastes`]): files
//! copied on the host start coming then. It runs with
//! `--shortcuts local` too, forwarding nothing.
//!
//! The hook runs on a thread of its own: every key of the whole system waits
//! for it, and the render thread may be decoding for ~13 ms.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A key taken by the hook: `wparam` the virtual key, `lparam` as of
/// `WM_KEYDOWN` / `WM_KEYUP` (scan code, extended, transition bits).
pub const WM_HOOK_KEY: u32 = WM_APP + 1;
/// One of the client's own shortcuts, Ctrl+Alt+Shift+`wparam` (a virtual key).
pub const WM_HOOK_SHORTCUT: u32 = WM_APP + 2;

/// The video window while the hook runs; 0: off.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Keys typed in the video window go to the host through the hook.
static FORWARD: AtomicBool = AtomicBool::new(false);
/// Pastes typed in other programs so far.
static PASTES: AtomicU64 = AtomicU64::new(0);

/// How many pastes (Ctrl+V, Shift+Insert) were typed in other programs
/// than the video window since the program started.
pub fn pastes() -> u64 {
    PASTES.load(Ordering::Relaxed)
}

/// What the hook does with one key.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// To the host, in order with the others.
    Forward,
    /// A client shortcut was pressed.
    Shortcut,
    /// Swallowed: the release of a shortcut key.
    Drop,
}

/// Ctrl, Alt and Shift held down, as the hook saw them.
#[derive(Default, Clone, Copy)]
struct Mods {
    ctrl: [bool; 2],
    alt: [bool; 2],
    shift: [bool; 2],
}

impl Mods {
    fn track(&mut self, vk: u16, down: bool) {
        let slot = match VIRTUAL_KEY(vk) {
            VK_LCONTROL | VK_CONTROL => &mut self.ctrl[0],
            VK_RCONTROL => &mut self.ctrl[1],
            VK_LMENU | VK_MENU => &mut self.alt[0],
            VK_RMENU => &mut self.alt[1],
            VK_LSHIFT | VK_SHIFT => &mut self.shift[0],
            VK_RSHIFT => &mut self.shift[1],
            _ => return,
        };
        *slot = down;
    }

    fn all_three(&self) -> bool {
        self.ctrl.contains(&true) && self.alt.contains(&true) && self.shift.contains(&true)
    }

    /// `vk` pressed with these makes a paste.
    fn paste(&self, vk: u16) -> bool {
        let (ctrl, alt, shift) = (
            self.ctrl.contains(&true),
            self.alt.contains(&true),
            self.shift.contains(&true),
        );
        !alt && ((ctrl && vk == u16::from(b'V'))
            || (shift && !ctrl && VIRTUAL_KEY(vk) == VK_INSERT))
    }
}

/// `shortcut_keys`: the letters that make the client's shortcuts with Ctrl+Alt+Shift.
fn decide(mods: Mods, vk: u16, down: bool, shortcut_keys: &[u16]) -> Action {
    if mods.all_three() && shortcut_keys.contains(&vk) {
        return if down { Action::Shortcut } else { Action::Drop };
    }
    Action::Forward
}

/// `lparam` of `WM_KEYDOWN` / `WM_KEYUP` for a key the hook saw.
fn key_lparam(scancode: u32, extended: bool, down: bool) -> isize {
    let mut l = 1 | ((scancode & 0xFF) << 16);
    if extended {
        l |= 1 << 24;
    }
    if !down {
        l |= (1 << 30) | (1 << 31);
    }
    l as i32 as isize
}

/// Window message the window hands on for a [`WM_HOOK_KEY`] (`lparam` tells up from down).
pub fn key_message(lparam: isize) -> u32 {
    if lparam as u32 & (1 << 31) == 0 {
        WM_KEYDOWN
    } else {
        WM_KEYUP
    }
}

thread_local! {
    static MODS: std::cell::Cell<Mods> = std::cell::Cell::new(Mods::default());
    static SHORTCUT_KEYS: std::cell::RefCell<Vec<u16>> = const { std::cell::RefCell::new(Vec::new()) };
}

unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let vk = k.vkCode as u16;
        let mut mods = MODS.get();
        let before = mods;
        mods.track(vk, down);
        MODS.set(mods);
        let target = TARGET.load(Ordering::Relaxed);
        let front = unsafe { GetForegroundWindow() }.0 as isize;
        // Mode 3: any of the video windows in front takes the keys.
        let in_front = target != 0
            && (front == target || EXTRA.iter().any(|e| e.load(Ordering::Relaxed) == front));
        if target != 0 && !in_front && down && before.paste(vk) {
            PASTES.fetch_add(1, Ordering::Relaxed);
        }
        if in_front && FORWARD.load(Ordering::Relaxed) {
            let hwnd = HWND(front as *mut _);
            let action = SHORTCUT_KEYS.with_borrow(|keys| decide(before, vk, down, keys));
            let extended = k.flags.0 & LLKHF_EXTENDED.0 != 0;
            let _ = match action {
                Action::Forward => unsafe {
                    PostMessageW(
                        Some(hwnd),
                        WM_HOOK_KEY,
                        WPARAM(vk as usize),
                        LPARAM(key_lparam(k.scanCode, extended, down)),
                    )
                },
                Action::Shortcut => unsafe {
                    PostMessageW(Some(hwnd), WM_HOOK_SHORTCUT, WPARAM(vk as usize), LPARAM(0))
                },
                Action::Drop => Ok(()),
            };
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// Mode 3's other video windows: in front, they take the keys like the main one.
static EXTRA: [AtomicIsize; 4] = [const { AtomicIsize::new(0) }; 4];

/// Window `hwnd` takes the keys too while in front.
pub fn also_target(hwnd: HWND) {
    let h = hwnd.0 as isize;
    if let Some(slot) = EXTRA.iter().find(|e| e.load(Ordering::Relaxed) == 0) {
        slot.store(h, Ordering::Relaxed);
    }
}

pub fn forget_target(hwnd: HWND) {
    let h = hwnd.0 as isize;
    EXTRA
        .iter()
        .filter(|e| e.load(Ordering::Relaxed) == h)
        .for_each(|e| e.store(0, Ordering::Relaxed));
}

/// The hook and its thread; dropping it gives the keys back to Windows.
pub struct Capture {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl Capture {
    /// With `forward`, keys go to the host while `hwnd` is in the foreground;
    /// Ctrl+Alt+Shift with one of `shortcut_keys` comes as [`WM_HOOK_SHORTCUT`].
    /// Pastes in other programs are counted either way.
    pub fn start(hwnd: HWND, shortcut_keys: &[u16], forward: bool) -> Result<Self> {
        let keys = shortcut_keys.to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("keyboard hook".into())
            .spawn(move || {
                SHORTCUT_KEYS.set(keys);
                let installed = unsafe {
                    let module = GetModuleHandleW(None).unwrap_or_default();
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(module.into()), 0)
                };
                let hook = match installed {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
                // The hook is called from this loop.
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {}
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            })?;
        let thread_id = rx
            .recv()
            .context("keyboard hook thread")?
            .context("SetWindowsHookExW(WH_KEYBOARD_LL)")?;
        FORWARD.store(forward, Ordering::Relaxed);
        TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
        Ok(Capture {
            thread: Some(thread),
            thread_id,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        TARGET.store(0, Ordering::Relaxed);
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What the host should log for a key `--test-shortcuts` sends: the key
/// itself, except Del (with Ctrl+Alt+Shift held): the host gets Ctrl+Alt+Del
/// instead, once, on press.
fn expected_event(scancode: u16, extended: bool, down: bool) -> Option<protocol::InputEvent> {
    use protocol::InputEvent;
    match (scancode, down) {
        (SCAN_DEL, true) => Some(InputEvent::SecureAttention),
        (SCAN_DEL, false) => None,
        _ => Some(InputEvent::Key {
            scancode,
            extended,
            down,
        }),
    }
}

const SCAN_DEL: u16 = 0x53;

/// `--test-shortcuts`: from a helper thread, presses system shortcuts with
/// `SendInput` (the hook sees injected keys too) and checks that `hwnd` stays
/// in the foreground. Writes the keys it pressed to `log` as the host logs
/// them (`Key { .. }` per line) and reports how many shortcuts escaped.
/// `harmless_only`: just Win and Alt+Tab (a control run without the hook).
pub fn test_shortcuts(hwnd: HWND, log: std::path::PathBuf, harmless_only: bool) {
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        use std::io::Write;
        const WIN: u16 = 0x5B;
        const ALT: u16 = 0x38;
        const CTRL: u16 = 0x1D;
        const SHIFT: u16 = 0x2A;
        const TAB: u16 = 0x0F;
        const ESC: u16 = 0x01;
        const SPACE: u16 = 0x39;
        const F4: u16 = 0x3E;
        const D: u16 = 0x20;
        const R: u16 = 0x13;
        const E: u16 = 0x12;
        const DEL: u16 = SCAN_DEL;
        // Never Win+L (locks the machine, no hook can stop it).
        let combos: &[(&str, &[u16])] = &[
            ("Win", &[WIN]),
            ("Win+D", &[WIN, D]),
            ("Win+R", &[WIN, R]),
            ("Win+E", &[WIN, E]),
            ("Win+Tab", &[WIN, TAB]),
            ("Alt+Tab", &[ALT, TAB]),
            ("Alt+Shift+Tab", &[ALT, SHIFT, TAB]),
            ("Alt+Esc", &[ALT, ESC]),
            ("Ctrl+Esc", &[CTRL, ESC]),
            ("Ctrl+Shift+Esc", &[CTRL, SHIFT, ESC]),
            ("Alt+Space", &[ALT, SPACE]),
            ("Alt+F4", &[ALT, F4]),
            // The client's shortcut for Ctrl+Alt+Del on the host (a real
            // Ctrl+Alt+Del cannot be pressed with SendInput).
            ("Ctrl+Alt+Shift+Del", &[CTRL, ALT, SHIFT, DEL]),
        ];
        let combos = if harmless_only {
            &[combos[0], combos[5]][..]
        } else {
            combos
        };
        let mut file = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("--test-shortcuts {}: {e}", log.display());
                return;
            }
        };
        std::thread::sleep(std::time::Duration::from_secs(2));
        let foreground = || unsafe { GetForegroundWindow() }.0 as isize == hwnd;
        if !foreground() {
            eprintln!("--test-shortcuts: the window is not in the foreground, not testing");
            return;
        }
        let mut escaped = 0;
        for (name, keys) in combos {
            let mut press = |scancode: u16, down: bool| {
                let extended = scancode == WIN || scancode == DEL;
                if let Some(ev) = expected_event(scancode, extended, down) {
                    let _ = writeln!(file, "{ev:?}");
                }
                let _ = input::send_key(scancode, extended, down);
                std::thread::sleep(std::time::Duration::from_millis(30));
            };
            for &k in keys.iter() {
                press(k, true);
            }
            for &k in keys.iter().rev() {
                press(k, false);
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
            if foreground() {
                println!("shortcut {name}: kept");
            } else {
                escaped += 1;
                println!("shortcut {name}: ESCAPED (the window lost the foreground)");
                unsafe {
                    let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        let _ = file.flush();
        println!("shortcuts: {} tested, {escaped} escaped", combos.len());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: u16 = b'Q' as u16;
    const F: u16 = b'F' as u16;
    const KEYS: &[u16] = &[Q, F];

    fn held(vks: &[VIRTUAL_KEY]) -> Mods {
        let mut m = Mods::default();
        for vk in vks {
            m.track(vk.0, true);
        }
        m
    }

    #[test]
    fn test_shortcuts_expects_secure_attention_for_del() {
        use protocol::InputEvent;
        assert_eq!(
            expected_event(SCAN_DEL, true, true),
            Some(InputEvent::SecureAttention)
        );
        assert_eq!(expected_event(SCAN_DEL, true, false), None);
        assert_eq!(
            expected_event(0x38, false, true),
            Some(InputEvent::Key {
                scancode: 0x38,
                extended: false,
                down: true
            })
        );
    }

    #[test]
    fn everything_goes_to_the_host_but_the_clients_shortcuts() {
        let none = Mods::default();
        assert_eq!(decide(none, 0x5B, true, KEYS), Action::Forward); // VK_LWIN
        assert_eq!(decide(none, Q, true, KEYS), Action::Forward);
        let alt_tab = held(&[VK_LMENU]);
        assert_eq!(decide(alt_tab, 0x09, true, KEYS), Action::Forward);
        let two = held(&[VK_LCONTROL, VK_RMENU]);
        assert_eq!(decide(two, Q, true, KEYS), Action::Forward);
        let three = held(&[VK_RCONTROL, VK_LMENU, VK_RSHIFT]);
        assert_eq!(decide(three, Q, true, KEYS), Action::Shortcut);
        assert_eq!(decide(three, F, true, KEYS), Action::Shortcut);
        assert_eq!(decide(three, Q, false, KEYS), Action::Drop);
        assert_eq!(decide(three, b'W' as u16, true, KEYS), Action::Forward);
    }

    #[test]
    fn modifiers_are_tracked_left_and_right() {
        let mut m = held(&[VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_LSHIFT]);
        assert!(m.all_three());
        m.track(VK_LCONTROL.0, false);
        assert!(m.all_three(), "the right Ctrl is still down");
        m.track(VK_RCONTROL.0, false);
        assert!(!m.all_three());
    }

    #[test]
    fn key_lparam_reads_back_as_the_window_does() {
        for (scancode, extended, down) in [
            (0x5B, true, true),
            (0x0F, false, false),
            (0x38, false, true),
        ] {
            let l = key_lparam(scancode, extended, down);
            let msg = key_message(l);
            assert_eq!(msg, if down { WM_KEYDOWN } else { WM_KEYUP });
            assert_eq!(
                input::from_window_message(msg, 0, l),
                vec![protocol::InputEvent::Key {
                    scancode: scancode as u16,
                    extended,
                    down
                }]
            );
        }
    }
}
