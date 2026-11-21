//! The API of what is drawn over the video. The overlays
//! themselves are not here: they are a web page (web/client/src/overlay) in a
//! window of Lumen Client's. This side keeps the core's half:
//!
//! * state: one `@overlay {json}` line on stdout ([`OverlayState`]) when
//!   something changed, a few times a second while something is going;
//! * commands: one JSON line per command on stdin ([`Command`]);
//! * the window: it stays over the picture of the main video window (the one
//!   with the host's main monitor; other windows have no overlays), moves with
//!   it, shows when the page has something to show, and takes clicks only where
//!   the page asks for them.
//!
//! The window is another process's: no owner relationship between the two
//! windows on purpose. It would attach their input queues, and a busy hub
//! would hold up the input of the video window. So the z-order is kept here.
//! Without a hub (the check scripts) nothing of this runs, and there are no
//! overlays. The colour path never sees any of it: the window is not in the
//! picture.

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use desktop::files::{Cmd, Stage, View as Transfer};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDPREV, GWL_EXSTYLE, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsIconic, IsWindow, IsWindowVisible,
    LWA_ALPHA, PostMessageW, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

use crate::layout::layout;
use crate::overlay::{self, Feed, Stats, ViewInfo};
use crate::window::{WM_SHOW_MONITOR, current_layout};

/// How often the state is looked at: a key or a mode change shows at once.
const LOOK: Duration = Duration::from_millis(50);
/// While the numbers or a transfer are on the move.
const BUSY: Duration = Duration::from_millis(200);
/// Otherwise: enough for a page that has just opened.
const IDLE: Duration = Duration::from_secs(1);
/// How often the window follows the picture and the mouse, shown / hidden.
const FOLLOW: Duration = Duration::from_millis(16);
const FOLLOW_HIDDEN: Duration = Duration::from_millis(50);

/// Ctrl+Alt+Shift+D, counted: what it does is the page's business.
static TRANSFERS_KEY: AtomicU32 = AtomicU32::new(0);

pub fn press_transfers_key() {
    TRANSFERS_KEY.fetch_add(1, Ordering::Relaxed);
}

/// A transfer: what the core knows, times as ages.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferState {
    pub outgoing: bool,
    /// Into `Downloads\Lumen` (not a clipboard set's).
    pub download: bool,
    /// The clipboard set it belongs to (numbered per direction).
    pub set: Option<u32>,
    /// A download's batch: the files of one paste, numbered by the sender.
    pub batch: Option<u32>,
    /// A format of a copied picture, not a file.
    pub image: bool,
    pub transfer: u32,
    pub name: String,
    pub len: u64,
    pub done: u64,
    /// `waiting`, `running`, `checking`, `saved`, `cancelled`.
    pub stage: &'static str,
    /// `user`, `disk`, `read`, `write`, `hash`, `not_allowed`, `no_user`, `locked`.
    pub reason: Option<&'static str>,
    /// Bytes per second lately.
    pub speed: f64,
    pub started_ago_ms: u64,
    pub ended_ago_ms: Option<u64>,
}

fn transfer_state(v: &Transfer, now: Instant) -> TransferState {
    use protocol::FileCancel as C;
    let (stage, reason) = match v.stage {
        Stage::Waiting => ("waiting", None),
        Stage::Running => ("running", None),
        Stage::Checking => ("checking", None),
        Stage::Saved => ("saved", None),
        Stage::Cancelled(r) => (
            "cancelled",
            Some(match r {
                C::User => "user",
                C::Disk => "disk",
                C::Read => "read",
                C::Write => "write",
                C::Hash => "hash",
                C::NotAllowed => "not_allowed",
                C::NoUser => "no_user",
                C::Locked => "locked",
            }),
        ),
    };
    let ago = |t: Instant| now.saturating_duration_since(t).as_millis() as u64;
    TransferState {
        outgoing: v.outgoing,
        download: v.download,
        set: v.set,
        batch: v.batch,
        image: v.image,
        transfer: v.transfer,
        name: v.name.clone(),
        len: v.len,
        done: v.done,
        stage,
        reason,
        speed: v.speed,
        started_ago_ms: ago(v.started),
        ended_ago_ms: v.ended.map(ago),
    }
}

/// Something the page should say for a moment (`serial` tells them apart).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notice {
    /// `two_windows_on_one_monitor`.
    pub code: &'static str,
    pub serial: u32,
}

/// What the page gets (web/client/src/overlay/api.ts has the same).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OverlayState {
    /// Screen pixels per picture pixel.
    pub picture_scale: f32,
    pub view: ViewInfo,
    /// Ctrl+Alt+Shift+O, or --overlay.
    pub stats_shown: bool,
    /// Only while shown.
    pub stats: Option<Stats>,
    /// Of this session, oldest first.
    pub transfers: Vec<TransferState>,
    pub transfers_key: u32,
    pub notice: Option<Notice>,
}

/// What the page or the hub asks for.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// The page's window, once it exists.
    OverlayWindow {
        hwnd: isize,
    },
    /// Whether the page has anything to show, and where it takes clicks:
    /// [x, y, width, height], pixels in its window.
    OverlayUi {
        visible: bool,
        interactive: Vec<[i32; 4]>,
    },
    CancelTransfer {
        outgoing: bool,
        transfer: u32,
    },
    ShowMonitor {
        index: usize,
    },
    /// The hub's answer to a host without monitors (it may come before the
    /// window: [`listen`]).
    MonitorChoice(crate::monitor_pick::MonitorPick),
}

/// The hub's commands, read from stdin from the start (after the token), so
/// that its answer to a host without monitors gets through before there is
/// a window; the rest wait for [`start`].
static COMMANDS: Mutex<Option<std::sync::mpsc::Receiver<Command>>> = Mutex::new(None);

pub fn listen() {
    let (tx, rx) = std::sync::mpsc::channel();
    *COMMANDS.lock().unwrap() = Some(rx);
    let _ = std::thread::Builder::new()
        .name("ui stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                match serde_json::from_str::<Command>(&line) {
                    Ok(Command::MonitorChoice(p)) => crate::monitor_pick::set(p),
                    Ok(c) => {
                        if tx.send(c).is_err() {
                            return;
                        }
                    }
                    Err(e) => eprintln!("ui: {e}: {line}"),
                }
            }
        });
}

#[derive(Default)]
struct Page {
    visible: bool,
    interactive: Vec<[i32; 4]>,
}

/// Starts the API's threads for the video window `owner`, while `--status-json`
/// says a hub listens. They end with the process.
pub fn start(owner: HWND, feed: Feed, transfers: Arc<Mutex<Vec<Transfer>>>, commands: Sender<Cmd>) {
    let owner = owner.0 as isize;
    let window = Arc::new(AtomicIsize::new(0));
    let page = Arc::new(Mutex::new(Page::default()));
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        std::thread::Builder::new()
            .name(name.into())
            .spawn(f)
            .is_ok()
    };
    {
        let (window, page) = (window.clone(), page.clone());
        spawn(
            "ui commands",
            Box::new(move || read_commands(owner, &window, &page, &commands)),
        );
    }
    spawn(
        "ui state",
        Box::new(move || send_state(owner, &feed, &transfers)),
    );
    spawn(
        "ui window",
        Box::new(move || follow_picture(owner, &window, &page)),
    );
}

fn read_commands(owner: isize, window: &AtomicIsize, page: &Mutex<Page>, files: &Sender<Cmd>) {
    let Some(commands) = COMMANDS.lock().unwrap().take() else {
        return;
    };
    for c in commands {
        match c {
            Command::OverlayWindow { hwnd } => {
                prepare_window(HWND(hwnd as *mut c_void));
                window.store(hwnd, Ordering::Relaxed);
            }
            Command::OverlayUi {
                visible,
                interactive,
            } => {
                *page.lock().unwrap() = Page {
                    visible,
                    interactive,
                };
            }
            Command::CancelTransfer { outgoing, transfer } => {
                let _ = files.send(Cmd::Cancel { outgoing, transfer });
            }
            Command::ShowMonitor { index } => unsafe {
                let _ = PostMessageW(
                    Some(HWND(owner as *mut c_void)),
                    WM_SHOW_MONITOR,
                    WPARAM(index),
                    LPARAM(0),
                );
            },
            Command::MonitorChoice(_) => {}
        }
    }
}

/// No taskbar button, no focus, and clicks go through (until asked for).
fn prepare_window(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & !(WS_EX_APPWINDOW.0 as isize);
        let on = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | on as isize);
        let _ = SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA);
    }
}

/// Clicks through the window or not.
fn take_clicks(hwnd: HWND, takes: bool) {
    let through = WS_EX_TRANSPARENT.0 as isize;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = if takes {
            style & !through
        } else {
            style | through
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    }
}

fn send_state(owner: isize, feed: &Feed, transfers: &Mutex<Vec<Transfer>>) {
    let owner = HWND(owner as *mut c_void);
    let mut said: Option<(Instant, String)> = None;
    loop {
        std::thread::sleep(LOOK);
        if !unsafe { IsWindow(Some(owner)).as_bool() } {
            return;
        }
        let now = Instant::now();
        let stats_shown = overlay::shown();
        let list: Vec<TransferState> = transfers
            .lock()
            .unwrap()
            .iter()
            .map(|v| transfer_state(v, now))
            .collect();
        let view = feed.view_info();
        let notice = feed.notice();
        let picture_scale = picture_on_screen(owner).map_or(1.0, |(_, s)| s);
        let key = TRANSFERS_KEY.load(Ordering::Relaxed);
        // What must show at once, whatever the pace.
        let sign = format!(
            "{stats_shown} {key} {notice:?} {view:?} {picture_scale} {}",
            list.len()
        );
        let going = list
            .iter()
            .any(|t| matches!(t.stage, "waiting" | "running" | "checking"));
        let pace = if stats_shown || going { BUSY } else { IDLE };
        if said
            .as_ref()
            .is_some_and(|(at, s)| *s == sign && now - *at < pace)
        {
            continue;
        }
        let state = OverlayState {
            picture_scale,
            view,
            stats_shown,
            stats: if stats_shown { feed.stats(now) } else { None },
            transfers: list,
            transfers_key: key,
            notice,
        };
        let Ok(json) = serde_json::to_string(&state) else {
            continue;
        };
        // In one piece: other lines go to the same pipe.
        let mut out = std::io::stdout().lock();
        if writeln!(out, "@overlay {json}").is_err() {
            return;
        }
        let _ = out.flush();
        said = Some((now, sign));
    }
}

/// The picture on the screen, and screen pixels per picture pixel; `None`
/// if the window is empty.
fn picture_on_screen(owner: HWND) -> Option<(RECT, f32)> {
    let mut client = RECT::default();
    unsafe { GetClientRect(owner, &mut client).ok()? };
    let mut origin = POINT::default();
    unsafe { ClientToScreen(owner, &mut origin).ok().ok()? };
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    let l = layout(current_layout().picture, (w as u32, h as u32));
    let (x, y, pw, ph) = l.picture_rect();
    (pw > 0 && ph > 0).then(|| {
        (
            RECT {
                left: origin.x + x,
                top: origin.y + y,
                right: origin.x + x + pw,
                bottom: origin.y + y + ph,
            },
            l.picture_scale(),
        )
    })
}

fn inside(rects: &[[i32; 4]], x: i32, y: i32) -> bool {
    rects
        .iter()
        .any(|&[rx, ry, w, h]| x >= rx && x < rx + w && y >= ry && y < ry + h)
}

/// The window over the picture, for as long as the video window lives.
fn follow_picture(owner: isize, window: &AtomicIsize, page: &Mutex<Page>) {
    let owner = HWND(owner as *mut c_void);
    let mut shown_at: Option<RECT> = None;
    // Hidden, the window still lies over the picture: the page lays itself
    // out at the right size before it is shown (else a block showed small for
    // a moment, laid out for the window's first size).
    let mut placed_at: Option<RECT> = None;
    let mut takes_clicks = false;
    loop {
        std::thread::sleep(if shown_at.is_some() {
            FOLLOW
        } else {
            FOLLOW_HIDDEN
        });
        if !unsafe { IsWindow(Some(owner)).as_bool() } {
            return;
        }
        let hwnd = window.load(Ordering::Relaxed);
        if hwnd == 0 {
            continue;
        }
        let hwnd = HWND(hwnd as *mut c_void);
        let (visible, clickable) = {
            let p = page.lock().unwrap();
            (p.visible, p.interactive.clone())
        };
        let on_screen = unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() };
        let picture = picture_on_screen(owner).map(|(r, _)| r);
        let place = |rect: RECT, show: bool| unsafe {
            let flags = SWP_NOACTIVATE | SWP_NOZORDER;
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                if show { flags | SWP_SHOWWINDOW } else { flags },
            );
        };
        let Some(rect) = picture.filter(|_| visible && on_screen) else {
            if shown_at.take().is_some() {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            if let Some(rect) = picture
                && placed_at != Some(rect)
            {
                place(rect, false);
                placed_at = Some(rect);
            }
            continue;
        };
        if shown_at != Some(rect) {
            place(rect, true);
            shown_at = Some(rect);
            placed_at = Some(rect);
        }
        keep_above(hwnd, owner);
        let mut cursor = POINT::default();
        let wants = unsafe { GetCursorPos(&mut cursor).is_ok() }
            && inside(&clickable, cursor.x - rect.left, cursor.y - rect.top);
        if wants != takes_clicks {
            takes_clicks = wants;
            take_clicks(hwnd, wants);
        }
    }
}

/// The nearest window shown above `owner`, other than the overlay's `hwnd`.
fn shown_above(owner: HWND, hwnd: HWND) -> Option<HWND> {
    let mut w = owner;
    loop {
        w = unsafe { GetWindow(w, GW_HWNDPREV) }.ok()?;
        if w != hwnd && unsafe { IsWindowVisible(w).as_bool() } {
            return Some(w);
        }
    }
}

/// Over the video window and under anything over it: another program's window,
/// or the windows of the other monitors (they have no overlays). With nothing
/// over it and in front: topmost (a plain `HWND_TOP` from here left it under
/// the video window).
fn keep_above(hwnd: HWND, owner: HWND) {
    let is_topmost =
        |w: HWND| unsafe { GetWindowLongPtrW(w, GWL_EXSTYLE) } & WS_EX_TOPMOST.0 as isize != 0;
    let place = |after: HWND| unsafe {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        if let Err(e) = SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, flags) {
            eprintln!("ui: the overlay's window: {e}");
        }
    };
    let cover = shown_above(owner, hwnd).filter(|&w| !is_topmost(w));
    if cover.is_none() && unsafe { GetForegroundWindow() } == owner {
        if !is_topmost(hwnd) {
            place(HWND_TOPMOST);
        }
        return;
    }
    if is_topmost(hwnd) {
        place(HWND_NOTOPMOST);
    }
    if unsafe { GetWindow(owner, GW_HWNDPREV) }.ok() != Some(hwnd) {
        // After a topmost window it would become topmost too.
        place(cover.unwrap_or(HWND_TOP));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_from_the_page() {
        let c = |s: &str| serde_json::from_str::<Command>(s).unwrap();
        assert_eq!(
            c(r#"{"cmd":"overlay_ui","visible":true,"interactive":[[1,2,3,4]]}"#),
            Command::OverlayUi {
                visible: true,
                interactive: vec![[1, 2, 3, 4]]
            }
        );
        assert_eq!(
            c(r#"{"cmd":"cancel_transfer","outgoing":false,"transfer":7}"#),
            Command::CancelTransfer {
                outgoing: false,
                transfer: 7
            }
        );
        assert_eq!(
            c(r#"{"cmd":"show_monitor","index":2}"#),
            Command::ShowMonitor { index: 2 }
        );
        assert_eq!(
            c(r#"{"cmd":"monitor_choice","width":3440,"height":1440,"scale":125}"#),
            Command::MonitorChoice(crate::monitor_pick::MonitorPick {
                width: 3440,
                height: 1440,
                scale: 125
            })
        );
        assert!(serde_json::from_str::<Command>(r#"{"cmd":"format_disk"}"#).is_err());
    }

    #[test]
    fn clicks_are_taken_only_where_asked() {
        let rects = [[10, 10, 5, 5], [100, 0, 1, 1]];
        assert!(inside(&rects, 10, 10) && inside(&rects, 14, 14) && inside(&rects, 100, 0));
        assert!(!inside(&rects, 15, 10) && !inside(&rects, 9, 10) && !inside(&[], 0, 0));
    }
}
