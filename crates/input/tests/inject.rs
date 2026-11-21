//! Real injection against the live desktop. Only cursor moves and an F24
//! key press are injected: no clicks, no wheel, nothing typed into windows.
#![cfg(windows)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use input::{Geometry, Injector, Rect};
use protocol::InputEvent;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F24;
use windows::Win32::UI::WindowsAndMessaging::*;

const F24_SCAN: u32 = 0x76;

// Hook events packed as: bit 0 = seen down, bit 1 = seen up, bit 2 = injected flag on every event.
static HOOK_SEEN: AtomicU32 = AtomicU32::new(0);
static HOOK_BAD: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL lparam points to a KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if k.scanCode == F24_SCAN {
            if k.flags.0 & LLKHF_INJECTED.0 == 0 {
                HOOK_BAD.fetch_or(1, Ordering::SeqCst);
            }
            if k.vkCode != VK_F24.0 as u32 {
                HOOK_BAD.fetch_or(2, Ordering::SeqCst);
            }
            match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => HOOK_SEEN.fetch_or(1, Ordering::SeqCst),
                WM_KEYUP | WM_SYSKEYUP => HOOK_SEEN.fetch_or(2, Ordering::SeqCst),
                _ => 0,
            };
        }
    }
    // SAFETY: standard hook chaining.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let mut msg = MSG::default();
        // SAFETY: plain message pump on the test thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn cursor() -> POINT {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    unsafe { GetCursorPos(&mut p).unwrap() };
    p
}

fn set_cursor(p: POINT) {
    // SAFETY: no preconditions.
    unsafe { SetCursorPos(p.x, p.y).unwrap() };
}

#[test]
fn inject_cursor_and_key() {
    // SAFETY: called once, before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap() };

    // Primary monitor: origin (0,0), size from the primary-screen metrics.
    // SAFETY: no preconditions.
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let output = Rect {
        left: 0,
        top: 0,
        width: w,
        height: h,
    };
    let geometry = Geometry::current_for_output(output);
    eprintln!("primary {w}x{h}, geometry {geometry:?}");
    let mut injector = Injector::new(geometry);

    let original = cursor();

    // (a) cursor positions.
    let points = [
        (1, 1),
        (w - 2, 1),
        (1, h - 2),
        (w - 2, h - 2),
        (w / 2, h / 2),
        (w / 3, h / 5),
    ];
    let mut failures = Vec::new();
    for (x, y) in points {
        injector.apply(&InputEvent::MouseMove { x, y }).unwrap();
        // Absolute moves are processed asynchronously by the input thread.
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut got = cursor();
        while (got.x, got.y) != (x, y) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            got = cursor();
        }
        eprintln!("moved to ({x},{y}), GetCursorPos = ({},{})", got.x, got.y);
        if (got.x, got.y) != (x, y) {
            failures.push(((x, y), (got.x, got.y)));
        }
    }
    set_cursor(original);

    // (b) keyboard via a low-level hook.
    // SAFETY: the hook procedure is a valid extern "system" fn; the thread pumps messages.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            Some(HINSTANCE::default()),
            0,
        )
        .expect("install WH_KEYBOARD_LL")
    };
    for down in [true, false] {
        injector
            .apply(&InputEvent::Key {
                scancode: F24_SCAN as u16,
                extended: false,
                down,
            })
            .unwrap();
        pump(100);
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while HOOK_SEEN.load(Ordering::SeqCst) != 3 && Instant::now() < deadline {
        pump(10);
    }
    // SAFETY: hook handle from SetWindowsHookExW.
    unsafe { UnhookWindowsHookEx(hook).unwrap() };
    let seen = HOOK_SEEN.load(Ordering::SeqCst);
    let bad = HOOK_BAD.load(Ordering::SeqCst);
    eprintln!("hook: seen mask {seen:#b} (1=down, 2=up), anomalies {bad:#b}");

    assert_eq!(injector.pressed_count(), 0);
    assert!(failures.is_empty(), "cursor mismatches: {failures:?}");
    assert_eq!(seen, 3, "hook must see F24 down and up");
    assert_eq!(bad, 0, "F24 must carry LLKHF_INJECTED and VK_F24");
}
