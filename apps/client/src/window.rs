use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITORINFO};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
    GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE,
    VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};

use crate::layout::{Layout, layout};
use crate::memory::Place;
use crate::shortcuts::{self, WM_HOOK_KEY, WM_HOOK_SHORTCUT};

/// Per-monitor-v2 DPI awareness. Call first thing in the process.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub struct Window {
    hwnd: HWND,
    open: bool,
    /// Borderless over its monitor.
    borderless: bool,
    /// Where the window was before it went borderless.
    windowed: Option<WINDOWPLACEMENT>,
    bars: BarFilter,
    /// System shortcuts go to the host (`--shortcuts host`).
    capture: Option<shortcuts::Capture>,
}

/// What of the mouse over the black bars around the picture goes to the
/// host. Moves do (the host's pointer stops at the screen's edge: an
/// auto-hide taskbar, edge scrolling); presses and the wheel do not (a click
/// into the black would land on whatever is at the host's screen edge).
/// A button pressed over the picture is released on the host wherever it is
/// let go (a drag goes on over the bars and beyond the window).
#[derive(Default)]
struct BarFilter {
    /// Buttons whose press went to the host, `1 << button`.
    pressed: u8,
}

impl BarFilter {
    /// The button of a button message: left, right, middle, X1, X2.
    fn button(msg: u32, wparam: usize) -> Option<(u8, bool)> {
        let x = || if (wparam >> 16) & 0xFFFF == 1 { 3 } else { 4 };
        match msg {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => Some((0, true)),
            WM_LBUTTONUP => Some((0, false)),
            WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => Some((1, true)),
            WM_RBUTTONUP => Some((1, false)),
            WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => Some((2, true)),
            WM_MBUTTONUP => Some((2, false)),
            WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => Some((x(), true)),
            WM_XBUTTONUP => Some((x(), false)),
            _ => None,
        }
    }

    /// Whether mouse message `msg` goes to the host; `over_picture`: where it happened.
    fn pass(&mut self, msg: u32, wparam: usize, over_picture: bool) -> bool {
        if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
            return over_picture;
        }
        match Self::button(msg, wparam) {
            Some((b, true)) if over_picture => {
                self.pressed |= 1 << b;
                true
            }
            Some((_, true)) => false,
            Some((b, false)) => {
                let sent = self.pressed & (1 << b) != 0;
                self.pressed &= !(1 << b);
                sent
            }
            None => true,
        }
    }
}

/// The window's style with a frame: resizable, with maximise.
const FRAMED: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;
/// The smallest client area a user can drag the window to.
const MIN_CLIENT: (u32, u32) = (320, 180);

/// The video windows' state shared across threads: one video window per
/// process (plus mode 3's others, each on its own thread, see
/// `crate::extra`). It is read and written from the window procedure, from a
/// modal loop's timer and from those threads, so it cannot live in
/// [`Window`]. What the layout needs is here; the host's monitors are in
/// `crate::scene`.
struct Shared {
    /// The client area's size, `w << 32 | h` (kept while minimised).
    client: AtomicU64,
    /// The main video window (monitor 1 in mode 3).
    main: AtomicIsize,
    /// Mode 3's other windows (a monitor of the host each): their client areas.
    extra_clients: Mutex<Vec<(isize, (u32, u32))>>,
    /// Windows closed by the program (mode 3 ended): their end does not end the session.
    quiet: Mutex<Vec<isize>>,
    /// Mode 3: Ctrl+Alt+Shift+F is for every window at once ([`WM_GROUP_BORDERLESS`]).
    group: AtomicBool,
    /// The window lost the keyboard focus since the last [`Window::pump`].
    focus_lost: AtomicBool,
    /// Remote-input mode (one window per process): keys go to the host, so local
    /// shortcuts (Esc, Alt+F4, Alt menu) are swallowed and the local cursor is
    /// hidden over the window — the host's cursor is drawn instead.
    remote_input: AtomicBool,
}

static SHARED: Shared = Shared {
    client: AtomicU64::new(0),
    main: AtomicIsize::new(0),
    extra_clients: Mutex::new(Vec::new()),
    quiet: Mutex::new(Vec::new()),
    group: AtomicBool::new(false),
    focus_lost: AtomicBool::new(false),
    remote_input: AtomicBool::new(false),
};

/// What Ctrl+Alt+Shift+F gives `on_input` in mode 3 (wparam, lparam 0).
pub const WM_GROUP_BORDERLESS: u32 = WM_APP + 0x21;

/// Mode 3 or not: see [`Shared::group`].
pub fn set_group(on: bool) {
    SHARED.group.store(on, Ordering::Relaxed);
}

/// The client area of `hwnd` (one of the video windows).
fn client_of(hwnd: isize) -> (u32, u32) {
    if hwnd == SHARED.main.load(Ordering::Relaxed) {
        return unpack(SHARED.client.load(Ordering::Relaxed));
    }
    SHARED
        .extra_clients
        .lock()
        .unwrap()
        .iter()
        .find(|(h, _)| *h == hwnd)
        .map_or((1, 1), |(_, c)| *c)
}

/// Where the picture goes in window `hwnd` (mode 3's windows show one
/// monitor each, as the main window does in mode 2).
pub fn layout_of(hwnd: HWND) -> Layout {
    let h = hwnd.0 as isize;
    let scene = crate::scene::for_window(h);
    let client = client_of(h);
    if scene.is_multi() {
        layout(client, client)
    } else {
        layout(scene.canvas, client)
    }
}

fn pack(w: u32, h: u32) -> u64 {
    (u64::from(w) << 32) | u64::from(h)
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// Where the picture goes in the window as it is now. One monitor of the
/// host: the picture, fitted (1:1 when it fits). Several: a buffer of the
/// window's size, the composite view scales the monitors itself.
pub fn current_layout() -> Layout {
    let client = unpack(SHARED.client.load(Ordering::Relaxed));
    let scene = crate::scene::current();
    if scene.is_multi() {
        layout(client, client)
    } else {
        layout(scene.canvas, client)
    }
}

/// A client-area point of window `hwnd` → the host's desktop point under it,
/// and whether it is on one of the host's monitors (not the black around and
/// between them).
fn to_desktop(hwnd: isize, x: i32, y: i32) -> ((i32, i32), bool) {
    let scene = crate::scene::for_window(hwnd);
    let fit = layout(scene.canvas, client_of(hwnd));
    let (cx, cy) = fit.to_picture(x, y);
    scene.to_desktop(cx, cy)
}

/// Switches borderless mode: Ctrl+Alt+Shift+F.
const BORDERLESS_KEY: u16 = b'F' as u16;

/// Leaves remote-input mode and closes the client: Ctrl+Alt+Shift+Q.
const EXIT_KEY: u16 = b'Q' as u16;

/// Shows or hides the session's overlay: Ctrl+Alt+Shift+O.
const OVERLAY_KEY: u16 = b'O' as u16;
/// The next way to show several monitors of the host: Ctrl+Alt+Shift+L.
const LAYOUT_KEY: u16 = b'L' as u16;
/// What Ctrl+Alt+Shift+L gives `on_input` (wparam, lparam 0).
pub const WM_NEXT_LAYOUT: u32 = WM_APP + 0x20;
/// Mode 2: show this monitor (wparam: its index). The overlay page asks.
pub const WM_SHOW_MONITOR: u32 = WM_APP + 0x22;

/// Shows or hides the file transfers: Ctrl+Alt+Shift+D.
const FILES_KEY: u16 = b'D' as u16;

/// Ctrl+Alt+Del on the host: Ctrl+Alt+Shift+Del (VK_DELETE).
const SECURE_ATTENTION_KEY: u16 = 0x2E;

/// Timer that keeps the picture going while Windows runs a modal loop of the
/// window (dragging it, its system menu): the loop does not return to
/// [`Window::pump`] until it ends, so without it the picture freezes.
const MODAL_TIMER: usize = 1;

thread_local! {
    /// The `on_modal` callback of the [`Window::pump_with`] call in progress.
    static MODAL_TICK: Cell<Option<*mut (dyn FnMut() + 'static)>> = const { Cell::new(None) };
}

/// Runs the modal callback once. It is taken out of the cell while it runs, so
/// a nested modal tick cannot alias it.
fn modal_tick() {
    if let Some(f) = MODAL_TICK.take() {
        // SAFETY: set by `pump_with` from a live `&mut dyn FnMut()` that is
        // not used elsewhere until it clears the cell (also on unwind).
        unsafe { (*f)() };
        MODAL_TICK.set(Some(f));
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let remote = SHARED.remote_input.load(Ordering::Relaxed);
    unsafe {
        match msg {
            // Any video window closed: the session ends (owner's decision
            // 2026-10-29); unless the program closed it (mode 3 ended).
            WM_DESTROY => {
                let id = hwnd.0 as isize;
                let mut quiet = SHARED.quiet.lock().unwrap();
                match quiet.iter().position(|&h| h == id) {
                    Some(i) => {
                        quiet.remove(i);
                    }
                    None => PostQuitMessage(0),
                }
                SHARED
                    .extra_clients
                    .lock()
                    .unwrap()
                    .retain(|(h, _)| *h != id);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_KILLFOCUS => {
                SHARED.focus_lost.store(true, Ordering::Relaxed);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SIZE => {
                let (w, h) = ((lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                if wp.0 != SIZE_MINIMIZED as usize && w > 0 && h > 0 {
                    let id = hwnd.0 as isize;
                    if id == SHARED.main.load(Ordering::Relaxed) {
                        SHARED.client.store(pack(w, h), Ordering::Relaxed);
                    } else if let Some(c) = SHARED
                        .extra_clients
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .find(|(h, _)| *h == id)
                    {
                        c.1 = (w, h);
                    }
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
                if let Ok((w, h)) =
                    outer_size(style, MIN_CLIENT.0, MIN_CLIENT.1, GetDpiForWindow(hwnd))
                {
                    info.ptMinTrackSize.x = w;
                    info.ptMinTrackSize.y = h;
                }
                LRESULT(0)
            }
            // Another monitor's DPI: take the size Windows suggests (per-monitor v2).
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            // Over the picture the host's pointer is drawn instead of ours; over
            // the black bars around it ours stays.
            WM_SETCURSOR if remote && (lp.0 & 0xFFFF) as u32 == HTCLIENT && over_picture(hwnd) => {
                SetCursor(None);
                LRESULT(1)
            }
            WM_SYSKEYDOWN | WM_SYSKEYUP | WM_SYSCHAR if remote => LRESULT(0),
            WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
                SetTimer(Some(hwnd), MODAL_TIMER, USER_TIMER_MINIMUM, None);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
                let _ = KillTimer(Some(hwnd), MODAL_TIMER);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_TIMER if wp.0 == MODAL_TIMER => {
                modal_tick();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The mouse is over the picture (not the bars around it).
fn over_picture(hwnd: HWND) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    let mut p = POINT::default();
    unsafe {
        if GetCursorPos(&mut p).is_err() || !ScreenToClient(hwnd, &mut p).as_bool() {
            return true;
        }
    }
    to_desktop(hwnd.0 as isize, p.x, p.y).1
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

fn outer_size(style: WINDOW_STYLE, width: u32, height: u32, dpi: u32) -> Result<(i32, i32)> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    unsafe { AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE(0), dpi)? };
    Ok((r.right - r.left, r.bottom - r.top))
}

/// What Windows knows of a monitor; zeroed if it does not answer.
pub(crate) fn monitor_info(mon: HMONITOR) -> MONITORINFO {
    use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = unsafe { GetMonitorInfoW(mon, &mut info) };
    info
}

/// A monitor's whole area and its work area (minus the taskbar): the one
/// `hwnd` is on, or the primary one.
fn monitor_rects(hwnd: Option<HWND>) -> (RECT, RECT) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, MonitorFromWindow,
    };
    unsafe {
        let mon = match hwnd {
            Some(h) => MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST),
            None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
        };
        let info = monitor_info(mon);
        (info.rcMonitor, info.rcWork)
    }
}

/// The whole area and the work area of the monitor nearest to a screen point.
fn monitor_at((x, y): (i32, i32)) -> (RECT, RECT) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let info = monitor_info(mon);
        (info.rcMonitor, info.rcWork)
    }
}

/// A framed window for the picture on the primary monitor: the picture's size
/// if it fits in the work area (1:1), else as large as fits, centred.
/// `(x, y, w, h)` of the window, and its client area.
type Placed = ((i32, i32, i32, i32), (u32, u32));

fn framed_rect(width: u32, height: u32, dpi: u32) -> Result<Placed> {
    let area = monitor_rects(None).1;
    let (frame_w, frame_h) = outer_size(FRAMED, 0, 0, dpi)?;
    let max_w = (area.right - area.left - frame_w).max(1) as u32;
    let max_h = (area.bottom - area.top - frame_h).max(1) as u32;
    let client = fit(width, height, max_w, max_h);
    let (w, h) = outer_size(FRAMED, client.0, client.1, dpi)?;
    let (x, y) = if client == (width, height) {
        (CW_USEDEFAULT, CW_USEDEFAULT)
    } else {
        (
            area.left + (area.right - area.left - w) / 2,
            area.top + (area.bottom - area.top - h) / 2,
        )
    };
    Ok(((x, y, w, h), client))
}

/// The largest size of the picture's aspect that fits in `max_w` x `max_h`.
fn fit(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    let s = (f64::from(max_w) / f64::from(width)).min(f64::from(max_h) / f64::from(height));
    (
        ((f64::from(width) * s).floor() as u32).max(1),
        ((f64::from(height) * s).floor() as u32).max(1),
    )
}

impl Window {
    /// Creates the window for a `width` x `height` picture. Framed: the picture's
    /// size if it fits in the primary monitor's work area, else as large as
    /// fits; the user may resize and maximise it, the picture keeps its aspect
    /// (see [`crate::layout`]). Borderless: over the whole primary monitor.
    pub fn new(title: &str, width: u32, height: u32, borderless: bool) -> Result<Window> {
        crate::scene::set(crate::scene::Scene::single(width, height));
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let class = w!("LumenClientWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                // The app icon (lumen.rc, resource 1): the taskbar's.
                // MAKEINTRESOURCE(1): an id, not an address.
                hIcon: LoadIconW(Some(hinst.into()), PCWSTR(std::ptr::without_provenance(1)))
                    .unwrap_or_default(),
                lpszClassName: class,
                ..Default::default()
            };
            // Registering twice fails harmlessly with ERROR_CLASS_ALREADY_EXISTS.
            RegisterClassW(&wc);

            let (style, (x, y, w, h), client) = if borderless {
                let m = monitor_rects(None).0;
                let size = ((m.right - m.left) as u32, (m.bottom - m.top) as u32);
                (
                    WS_POPUP,
                    (m.left, m.top, size.0 as i32, size.1 as i32),
                    size,
                )
            } else {
                let (rect, client) = framed_rect(width, height, GetDpiForSystem())?;
                (FRAMED, rect, client)
            };
            let title_w: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR(title_w.as_ptr()),
                style,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;
            SHARED.main.store(hwnd.0 as isize, Ordering::Relaxed);
            let win = Window {
                hwnd,
                open: true,
                borderless,
                windowed: None,
                bars: BarFilter::default(),
                capture: None,
            };
            // The window may have landed on a monitor with a different DPI than the
            // system one; fix the outer size so the client area is as planned.
            if !borderless && win.client_size() != client {
                let (w, h) = outer_size(style, client.0, client.1, GetDpiForWindow(hwnd))?;
                SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER)?;
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let (cw, ch) = win.client_size();
            SHARED.client.store(pack(cw, ch), Ordering::Relaxed);
            Ok(win)
        }
    }

    /// Mode 3: another window, framed, for a picture of `width` × `height`
    /// (one of the host's monitors), at (`x`, `y`) on the screen, as large as
    /// fits in the primary monitor's work area. Closing it ends the session.
    pub fn new_extra(title: &str, width: u32, height: u32, x: i32, y: i32) -> Result<Window> {
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let ((_, _, w, h), client) = framed_rect(width, height, GetDpiForSystem())?;
            let title_w: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("LumenClientWindow"),
                PCWSTR(title_w.as_ptr()),
                FRAMED,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;
            SHARED
                .extra_clients
                .lock()
                .unwrap()
                .push((hwnd.0 as isize, client));
            let win = Window {
                hwnd,
                open: true,
                borderless: false,
                windowed: None,
                bars: BarFilter::default(),
                capture: None,
            };
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let (cw, ch) = win.client_size();
            if let Some(c) = SHARED
                .extra_clients
                .lock()
                .unwrap()
                .iter_mut()
                .find(|(h, _)| *h == hwnd.0 as isize)
            {
                c.1 = (cw.max(1), ch.max(1));
            }
            crate::shortcuts::also_target(hwnd);
            Ok(win)
        }
    }

    /// Closes this window without ending the session (mode 3 ended).
    pub fn close_quietly(&mut self) {
        SHARED.quiet.lock().unwrap().push(self.hwnd.0 as isize);
        crate::shortcuts::forget_target(self.hwnd);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        self.open = false;
    }

    pub fn is_borderless(&self) -> bool {
        self.borderless
    }

    /// The monitor (of this machine) under the window's centre.
    pub fn monitor(&self) -> isize {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
            let centre = POINT {
                x: (r.left + r.right) / 2,
                y: (r.top + r.bottom) / 2,
            };
            MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST).0 as isize
        }
    }

    /// Where the window is: its top-left corner on the screen.
    pub fn position(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        (r.left, r.top)
    }

    /// Borderless over the window's monitor, or back to the framed window
    /// where it was (Ctrl+Alt+Shift+F). The stream goes on.
    pub fn set_borderless(&mut self, on: bool) {
        self.set_borderless_at(on, None);
    }

    /// [`Self::set_borderless`] over the monitor around `at` (a screen point)
    /// instead of the window's.
    fn set_borderless_at(&mut self, on: bool, at: Option<(i32, i32)>) {
        if on == self.borderless {
            return;
        }
        unsafe {
            if on {
                let mut p = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                if GetWindowPlacement(self.hwnd, &mut p).is_ok() {
                    self.windowed = Some(p);
                }
                let m = match at {
                    Some(pt) => monitor_at(pt).0,
                    None => monitor_rects(Some(self.hwnd)).0,
                };
                self.borderless = true;
                SetWindowLongW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as i32);
                // Owned windows (the overlay) are raised with it: with
                // SWP_NOOWNERZORDER they stayed behind the picture.
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOP),
                    m.left,
                    m.top,
                    m.right - m.left,
                    m.bottom - m.top,
                    SWP_FRAMECHANGED,
                );
            } else {
                self.borderless = false;
                SetWindowLongW(self.hwnd, GWL_STYLE, (FRAMED | WS_VISIBLE).0 as i32);
                match &self.windowed {
                    Some(p) => {
                        let _ = SetWindowPlacement(self.hwnd, p);
                    }
                    // Started borderless: where a framed window would have opened.
                    None => {
                        let (pw, ph) = crate::scene::for_window(self.hwnd.0 as isize).canvas;
                        if let Ok(((x, y, w, h), _)) =
                            framed_rect(pw, ph, GetDpiForWindow(self.hwnd))
                        {
                            let (x, y) = if x == CW_USEDEFAULT {
                                (100, 100)
                            } else {
                                (x, y)
                            };
                            let _ = SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER);
                        }
                    }
                }
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            }
        }
    }

    /// Where the window is, to be remembered (`crate::memory`).
    pub fn place(&self) -> Option<Place> {
        let mut p = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        let mut r = RECT::default();
        unsafe {
            GetWindowPlacement(self.hwnd, &mut p).ok()?;
            GetWindowRect(self.hwnd, &mut r).ok()?;
        }
        // Borderless: the framed place it goes back to (none when it started
        // borderless: the screen it covers).
        let framed = if self.borderless { self.windowed } else { None };
        let framed = framed.unwrap_or(p);
        let n = framed.rcNormalPosition;
        Some(Place {
            normal: (n.left, n.top, n.right, n.bottom),
            maximized: framed.showCmd == SW_SHOWMAXIMIZED.0 as u32,
            borderless: self
                .borderless
                .then_some(((r.left + r.right) / 2, (r.top + r.bottom) / 2)),
        })
    }

    /// Puts the window where it was (`place`). Windows keeps a framed window
    /// on a monitor of this machine; a borderless one covers the monitor
    /// nearest to where it was.
    pub fn restore(&mut self, place: &Place) {
        self.set_borderless(false);
        let (l, t, r, b) = place.normal;
        let p = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            showCmd: if place.maximized {
                SW_SHOWMAXIMIZED.0 as u32
            } else {
                SW_SHOWNORMAL.0 as u32
            },
            rcNormalPosition: RECT {
                left: l,
                top: t,
                right: r,
                bottom: b,
            },
            ..Default::default()
        };
        unsafe {
            let _ = SetWindowPlacement(self.hwnd, &p);
        }
        if let Some(at) = place.borderless {
            self.set_borderless_at(true, Some(at));
        }
    }

    /// Mouse `lparam` in client coordinates -> on the host's desktop, and
    /// whether that is on one of its monitors.
    fn to_desktop(&self, hwnd: HWND, lparam: isize) -> (isize, bool) {
        let x = i32::from(lparam as u16 as i16);
        let y = i32::from((lparam >> 16) as u16 as i16);
        let ((dx, dy), on) = to_desktop(hwnd.0 as isize, x, y);
        let lim = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16 as u16;
        (
            ((u32::from(lim(dy)) << 16) | u32::from(lim(dx))) as i32 as isize,
            on,
        )
    }

    /// The host's monitors changed (`crate::scene`): a framed window takes
    /// the new layout's aspect, as large as it opens (borderless and
    /// maximised windows stay as they are; the picture fits in them).
    pub fn fit_scene(&mut self) {
        if self.borderless {
            return;
        }
        unsafe {
            if IsZoomed(self.hwnd).as_bool() {
                return;
            }
            let (cw, ch) = crate::scene::current().canvas;
            let Ok(((x, y, w, h), _)) = framed_rect(cw, ch, GetDpiForWindow(self.hwnd)) else {
                return;
            };
            let flags = if x == CW_USEDEFAULT {
                SWP_NOMOVE | SWP_NOZORDER
            } else {
                SWP_NOZORDER
            };
            let _ = SetWindowPos(self.hwnd, None, x, y, w, h, flags);
        }
    }

    /// Remote-input mode, see [`Shared::remote_input`].
    pub fn set_remote_input(&mut self, on: bool) {
        SHARED.remote_input.store(on, Ordering::Relaxed);
    }

    /// In remote-input mode, system shortcuts (Win, Alt+Tab, ...) go to the
    /// host while the window is in the foreground (see `shortcuts`).
    /// The keyboard hook: system shortcuts to the host (`forward`), pastes
    /// in other programs counted.
    pub fn capture_shortcuts(&mut self, forward: bool) -> Result<()> {
        if self.capture.is_none() {
            self.capture = Some(shortcuts::Capture::start(
                self.hwnd,
                &[
                    EXIT_KEY,
                    BORDERLESS_KEY,
                    OVERLAY_KEY,
                    FILES_KEY,
                    SECURE_ATTENTION_KEY,
                    LAYOUT_KEY,
                ],
                forward,
            )?);
        }
        Ok(())
    }

    /// Processes pending messages without blocking; mouse and keyboard
    /// messages are also passed to `on_input(msg, wparam, lparam)`, and so is
    /// `WM_KILLFOCUS` (wparam, lparam 0) when the window lost the focus: what
    /// is held down will not see its release here. Returns
    /// false once the window was closed (Esc, or Ctrl+Alt+Shift+Q in remote mode).
    pub fn pump(&mut self, on_input: impl FnMut(u32, usize, isize)) -> bool {
        self.pump_with(on_input, &mut || {})
    }

    /// [`Window::pump`] that also calls `on_modal` about every 16 ms (the
    /// system timer tick) while a modal loop of the window runs inside it.
    /// `on_modal` must not touch the window.
    pub fn pump_with(
        &mut self,
        on_input: impl FnMut(u32, usize, isize),
        on_modal: &mut dyn FnMut(),
    ) -> bool {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                MODAL_TICK.set(None);
            }
        }
        let f: *mut (dyn FnMut() + '_) = on_modal;
        // SAFETY: only the lifetime is erased; the guard clears the pointer
        // before `on_modal` goes out of scope.
        let f: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(f) };
        MODAL_TICK.set(Some(f));
        let _clear = Clear;
        self.dispatch(on_input)
    }

    fn dispatch(&mut self, mut on_input: impl FnMut(u32, usize, isize)) -> bool {
        let remote = SHARED.remote_input.load(Ordering::Relaxed);
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let m = msg.message;
                if m == WM_QUIT {
                    self.open = false;
                    continue;
                }
                let is_key = (WM_KEYFIRST..=WM_KEYLAST).contains(&m);
                let is_mouse = (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m);
                let key_pressed = m == WM_KEYDOWN || m == WM_SYSKEYDOWN;
                // Keys the hook took from Windows (see `shortcuts`).
                let hooked = m == WM_HOOK_SHORTCUT && remote;
                let vk = msg.wParam.0 as u16;
                // Ctrl+Alt+Shift+<key>: pressed here, or from the hook.
                let chord = |key: u16| {
                    (key_pressed
                        && vk == key
                        && key_down(VK_CONTROL)
                        && key_down(VK_MENU)
                        && key_down(VK_SHIFT))
                        || (hooked && vk == key)
                };
                let exit = if remote {
                    chord(EXIT_KEY)
                } else {
                    m == WM_KEYDOWN && msg.wParam.0 == usize::from(VK_ESCAPE.0)
                };
                if m == WM_SHOW_MONITOR {
                    on_input(m, msg.wParam.0, 0);
                    continue;
                }
                if m == WM_HOOK_KEY {
                    on_input(
                        shortcuts::key_message(msg.lParam.0),
                        msg.wParam.0,
                        msg.lParam.0,
                    );
                    continue;
                }
                if exit {
                    self.open = false;
                    let _ = DestroyWindow(self.hwnd);
                    continue;
                }
                if remote && chord(SECURE_ATTENTION_KEY) {
                    on_input(input::WM_SECURE_ATTENTION, 0, 0);
                    continue;
                }
                if chord(LAYOUT_KEY) {
                    on_input(WM_NEXT_LAYOUT, 0, 0);
                    continue;
                }
                if chord(OVERLAY_KEY) {
                    crate::overlay::toggle();
                    continue;
                }
                if chord(FILES_KEY) {
                    crate::ui_api::press_transfers_key();
                    continue;
                }
                if chord(BORDERLESS_KEY) {
                    if SHARED.group.load(Ordering::Relaxed) {
                        on_input(WM_GROUP_BORDERLESS, 0, 0);
                    } else {
                        self.set_borderless(!self.borderless);
                    }
                    continue;
                }
                if is_key {
                    on_input(m, msg.wParam.0, msg.lParam.0);
                } else if is_mouse {
                    // Wheel messages carry screen coordinates (not used), the rest client ones.
                    let (lp, over) = if m == WM_MOUSEWHEEL || m == WM_MOUSEHWHEEL {
                        (msg.lParam.0, over_picture(self.hwnd))
                    } else {
                        self.to_desktop(msg.hwnd, msg.lParam.0)
                    };
                    if !remote || self.bars.pass(m, msg.wParam.0, over) {
                        on_input(m, msg.wParam.0, lp);
                    }
                }
                // Keep receiving mouse events while a button is held outside the window.
                if remote && is_mouse {
                    let buttons = msg.wParam.0 & 0x73; // MK_LBUTTON|RBUTTON|MBUTTON|XBUTTON1|2
                    if buttons != 0 {
                        SetCapture(msg.hwnd);
                    } else if GetCapture() == msg.hwnd {
                        let _ = ReleaseCapture();
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if SHARED.focus_lost.swap(false, Ordering::Relaxed) {
                self.bars = BarFilter::default();
                on_input(WM_KILLFOCUS, 0, 0);
            }
        }
        self.open
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn client_size(&self) -> (u32, u32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
    }
}

/// `--test-move-loop`: from a helper thread, every 3 s puts the window into
/// the keyboard move loop (`SC_MOVE`) and ends it 1.5 s later — the same
/// modal loop as dragging the title bar.
pub fn test_move_loop(win: &Window) {
    let hwnd = win.hwnd.0 as isize;
    std::thread::spawn(move || {
        let hwnd = HWND(hwnd as *mut _);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            unsafe {
                if PostMessageW(
                    Some(hwnd),
                    WM_SYSCOMMAND,
                    WPARAM(SC_MOVE as usize),
                    LPARAM(0),
                )
                .is_err()
                {
                    return; // the window is gone
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1500));
            unsafe {
                SendMessageW(hwnd, WM_CANCELMODE, None, None);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_the_bars_moves_pass_presses_and_the_wheel_do_not() {
        let mut f = BarFilter::default();
        assert!(f.pass(WM_MOUSEMOVE, 0, false));
        assert!(!f.pass(WM_LBUTTONDOWN, 0, false));
        assert!(!f.pass(WM_LBUTTONUP, 0, false));
        assert!(!f.pass(WM_MOUSEWHEEL, 0, false));
        assert!(f.pass(WM_MOUSEWHEEL, 0, true));
    }

    #[test]
    fn a_drag_from_the_picture_is_released_over_the_bars() {
        let mut f = BarFilter::default();
        assert!(f.pass(WM_LBUTTONDOWN, 0, true));
        assert!(f.pass(WM_MOUSEMOVE, 0, false));
        // Another button pressed over the bar meanwhile is not sent, nor its release.
        assert!(!f.pass(WM_RBUTTONDOWN, 0, false));
        assert!(!f.pass(WM_RBUTTONUP, 0, true));
        assert!(f.pass(WM_LBUTTONUP, 0, false));
        // X buttons are told apart.
        assert!(f.pass(WM_XBUTTONDOWN, 1 << 16, true));
        assert!(!f.pass(WM_XBUTTONUP, 2 << 16, true));
        assert!(f.pass(WM_XBUTTONUP, 1 << 16, false));
    }

    #[test]
    fn picture_fits_the_work_area() {
        assert_eq!(fit(1920, 1080, 3824, 2072), (1920, 1080));
        assert_eq!(fit(3840, 2160, 3824, 2072), (3683, 2072));
        assert_eq!(fit(3840, 2160, 3824, 2160), (3824, 2151));
    }
}
