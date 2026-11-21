//! The clipboard thread: a message-only window (writing needs an owner
//! window, and a clipboard owner must answer messages: another program's
//! `EmptyClipboard` waits for it), the sequence number read every
//! [`POLL`], our own writes skipped.

use std::sync::mpsc::{Receiver, Sender};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_MESSAGE, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use super::formats::{OPEN_TRIES, read, write};
use super::{Ask, Content, POLL};

unsafe extern "system" fn window_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

fn create_window() -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("LumenClipboard");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // A second registration (a second Clipboard in this process) fails: fine.
        RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("clipboard window: {e}"))
    }
}

/// The clipboard thread's state.
struct Worker {
    window: HWND,
    /// The sequence number up to which the clipboard has been looked at.
    seen: u32,
    /// What could not be written yet (the host's clipboard is out of reach
    /// while Windows is locked): tried again each round until it is.
    pending: Option<Content>,
}

impl Worker {
    fn new(window: HWND, with_current: bool) -> Worker {
        Worker {
            window,
            // 0: the first look reads what is on the clipboard now.
            seen: if with_current {
                0
            } else {
                unsafe { GetClipboardSequenceNumber() }
            },
            pending: None,
        }
    }

    /// Our own write is done: not a change to pass on, nothing left to write.
    fn wrote(&mut self) {
        self.seen = unsafe { GetClipboardSequenceNumber() };
        self.pending = None;
    }

    /// Answers the asks; false: stop.
    fn handle_asks(&mut self, asks: &Receiver<Ask>) -> bool {
        for ask in asks.try_iter() {
            match ask {
                Ask::Set(c, done) => {
                    let ok = write(self.window, &c, OPEN_TRIES);
                    if ok {
                        self.wrote();
                    } else {
                        self.pending = Some(c);
                    }
                    let _ = done.send(ok);
                }
                Ask::Stop => {
                    unsafe {
                        let _ = DestroyWindow(self.window);
                    }
                    return false;
                }
            }
        }
        true
    }

    /// Passes on what was copied here; false: nobody listens any more.
    fn poll_changes(&mut self, changes: &Sender<Content>) -> bool {
        let now = unsafe { GetClipboardSequenceNumber() };
        if now == self.seen {
            return true;
        }
        match read(self.window) {
            // Busy (another program has it open): next time.
            Err(()) => true,
            Ok(c) => {
                self.seen = now;
                // Copied here after it: newer than the pending one.
                self.pending = None;
                c.is_none_or(|c| changes.send(c).is_ok())
            }
        }
    }

    /// Once a round, one try: while locked, every try fails.
    fn retry_pending(&mut self) {
        if let Some(c) = self.pending.as_ref()
            && write(self.window, c, 1)
        {
            self.wrote();
            println!("clipboard: written now that the clipboard is free");
        }
    }
}

/// The thread: its window's handle goes to `ready` first.
pub(super) fn run(
    changes: Sender<Content>,
    asks: Receiver<Ask>,
    ready: Sender<Result<isize, String>>,
    with_current: bool,
) {
    let window = match create_window() {
        Ok(w) => w,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(window.0 as isize));
    let mut worker = Worker::new(window, with_current);
    loop {
        unsafe {
            MsgWaitForMultipleObjects(None, false, POLL.as_millis() as u32, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if !worker.handle_asks(&asks) || !worker.poll_changes(&changes) {
            return;
        }
        worker.retry_pending();
    }
}
