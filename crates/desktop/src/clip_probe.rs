//! Files on the clipboard with delayed rendering, step 0 of the file transfer
//!: who asks for them, and what a paste does with what
//! we hand over. `lumen-host clip-probe` (runs as SYSTEM too, like the host).
//!
//! The probe puts `CF_HDROP` on the clipboard without data
//! (`SetClipboardData(CF_HDROP, NULL)`): Windows sends us `WM_RENDERFORMAT`
//! when a program asks for it — a paste. Each ask is logged with the program
//! that has the clipboard open (the one asking) and the foreground one.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, GetClipboardSequenceNumber, GetOpenClipboardWindow,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CHANGEFILTERSTRUCT, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, HWND_MESSAGE,
    MSG, MSGFLT_ALLOW, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_CLIPBOARDUPDATE, WM_DESTROYCLIPBOARD,
    WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// File list (`CF_HDROP`).
const CF_HDROP: u32 = 15;

/// What a `WM_RENDERFORMAT` for `CF_HDROP` gets.
#[derive(Clone, Debug)]
pub enum Render {
    /// A list of no files.
    Empty,
    /// Nothing (`GetClipboardData` returns NULL to the asker).
    Null,
    /// These files: a real paste, to see the mechanism work end to end.
    Files(Vec<PathBuf>),
}

pub struct Options {
    pub seconds: f64,
    pub render: Render,
    /// Wait this long before answering (as if the files were coming first).
    pub delay: Duration,
    /// Also put the formats that keep Windows' clipboard history and cloud
    /// clipboard away from it.
    pub exclude: bool,
    /// Let lower integrity programs send us `WM_RENDERFORMAT` (UIPI).
    pub allow_uipi: bool,
    /// Put the files on the clipboard again after each paste.
    pub again: bool,
}

struct State {
    start: Instant,
    opts: Options,
    renders: u32,
    /// The clipboard sequence number of our own last write.
    ours: u32,
    reput: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn log(start: Instant, line: &str) {
    println!("{:9.3} {line}", start.elapsed().as_secs_f64());
    let _ = std::io::stdout().flush();
}

/// The program (exe name) and class of `h`.
fn describe(h: HWND) -> String {
    if h.is_invalid() {
        return "-".into();
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut class = [0u16; 128];
        let n = GetClassNameW(h, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        let exe = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(p) => {
                let mut buf = [0u16; 520];
                let mut len = buf.len() as u32;
                let r = QueryFullProcessImageNameW(
                    p,
                    PROCESS_NAME_WIN32,
                    windows::core::PWSTR(buf.as_mut_ptr()),
                    &mut len,
                );
                let _ = CloseHandle(p);
                match r {
                    Ok(()) => {
                        let full = String::from_utf16_lossy(&buf[..len as usize]);
                        full.rsplit('\\').next().unwrap_or(&full).to_string()
                    }
                    Err(_) => format!("pid {pid}"),
                }
            }
            Err(_) => format!("pid {pid} (no access)"),
        };
        format!("{exe} [{class}]")
    }
}

/// A `CF_HDROP` block: `DROPFILES` + wide paths, each ending in 0, then 0.
fn hdrop(files: &[PathBuf]) -> Option<HANDLE> {
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt: POINT,
        f_nc: i32,
        f_wide: i32,
    }
    let mut names: Vec<u16> = Vec::new();
    for f in files {
        names.extend(f.as_os_str().to_string_lossy().encode_utf16());
        names.push(0);
    }
    names.push(0);
    if files.is_empty() {
        names.push(0);
    }
    let head = std::mem::size_of::<DropFiles>();
    let size = head + names.len() * 2;
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, size).ok()?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return None;
        }
        let df = DropFiles {
            p_files: head as u32,
            pt: POINT::default(),
            f_nc: 0,
            f_wide: 1,
        };
        std::ptr::copy_nonoverlapping(&df as *const DropFiles as *const u8, p, head);
        std::ptr::copy_nonoverlapping(names.as_ptr() as *const u8, p.add(head), names.len() * 2);
        let _ = GlobalUnlock(g);
        Some(HANDLE(g.0))
    }
}

/// A `DWORD` block (the clipboard history formats).
fn dword(v: u32) -> Option<HANDLE> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, 4).ok()?;
        let p = GlobalLock(g) as *mut u32;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return None;
        }
        *p = v;
        let _ = GlobalUnlock(g);
        Some(HANDLE(g.0))
    }
}

/// Puts the delayed `CF_HDROP` (and the exclusion formats) on the clipboard.
fn put(window: HWND, exclude: bool) -> Result<(), String> {
    unsafe {
        OpenClipboard(Some(window)).map_err(|e| format!("OpenClipboard: {e}"))?;
        let _ = EmptyClipboard();
        // Delayed: the call returns NULL, which `windows` reports as an error.
        let _ = SetClipboardData(CF_HDROP, None);
        if exclude {
            for (name, v) in [
                (w!("ExcludeClipboardContentFromMonitorProcessing"), 0),
                (w!("CanIncludeInClipboardHistory"), 0),
                (w!("CanUploadToCloudClipboard"), 0),
            ] {
                let f = RegisterClipboardFormatW(name);
                if let Some(h) = dword(v) {
                    let _ = SetClipboardData(f, Some(h));
                }
            }
        }
        let _ = CloseClipboard();
        IsClipboardFormatAvailable(CF_HDROP)
            .map_err(|_| "CF_HDROP is not on the clipboard".to_string())
    }
}

/// Answers `WM_RENDERFORMAT` (the clipboard is open by the asker).
fn render(start: Instant, how: &Render) {
    let h = match how {
        Render::Null => {
            log(start, "  answer: nothing");
            return;
        }
        Render::Empty => hdrop(&[]),
        Render::Files(f) => hdrop(f),
    };
    let Some(h) = h else {
        log(start, "  answer: GlobalAlloc failed");
        return;
    };
    match unsafe { SetClipboardData(CF_HDROP, Some(h)) } {
        Ok(_) => log(start, &format!("  answer: {how:?}")),
        Err(e) => {
            unsafe {
                let _ = GlobalFree(Some(windows::Win32::Foundation::HGLOBAL(h.0)));
            }
            log(start, &format!("  answer failed: {e}"));
        }
    }
}

unsafe extern "system" fn window_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut guard = STATE.lock().unwrap();
    let Some(s) = guard.as_mut() else {
        return unsafe { DefWindowProcW(h, m, wp, lp) };
    };
    match m {
        WM_RENDERFORMAT => {
            s.renders += 1;
            let asker = describe(unsafe { GetOpenClipboardWindow() }.unwrap_or_default());
            let front = describe(unsafe { GetForegroundWindow() });
            log(
                s.start,
                &format!(
                    "WM_RENDERFORMAT #{} format {}: asker {asker}; foreground {front}",
                    s.renders, wp.0
                ),
            );
            if !s.opts.delay.is_zero() {
                std::thread::sleep(s.opts.delay);
            }
            render(s.start, &s.opts.render);
            if s.opts.again {
                s.reput = true;
            }
            LRESULT(0)
        }
        WM_RENDERALLFORMATS => {
            log(s.start, "WM_RENDERALLFORMATS (we are going away)");
            unsafe {
                if OpenClipboard(Some(h)).is_ok() {
                    if GetClipboardOwner().ok() == Some(h) {
                        render(s.start, &Render::Empty);
                    }
                    let _ = CloseClipboard();
                }
            }
            LRESULT(0)
        }
        WM_DESTROYCLIPBOARD => {
            log(
                s.start,
                "WM_DESTROYCLIPBOARD (someone emptied the clipboard)",
            );
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            let n = unsafe { GetClipboardSequenceNumber() };
            if n != s.ours {
                let owner = describe(unsafe { GetClipboardOwner() }.unwrap_or_default());
                log(
                    s.start,
                    &format!("WM_CLIPBOARDUPDATE seq {n}: new owner {owner}"),
                );
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, m, wp, lp) },
    }
}

fn create_window() -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("LumenClipProbe");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
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
        .map_err(|e| format!("window: {e}"))
    }
}

fn put_logged(window: HWND, start: Instant, exclude: bool) -> Result<u32, String> {
    put(window, exclude)?;
    let seq = unsafe { GetClipboardSequenceNumber() };
    log(
        start,
        &format!("on the clipboard: CF_HDROP, delayed (exclusion formats: {exclude}), seq {seq}"),
    );
    Ok(seq)
}

pub fn run(opts: Options) -> Result<(), String> {
    let start = Instant::now();
    let window = create_window()?;
    unsafe {
        if opts.allow_uipi {
            let mut cf = CHANGEFILTERSTRUCT {
                cbSize: std::mem::size_of::<CHANGEFILTERSTRUCT>() as u32,
                ..Default::default()
            };
            for m in [WM_RENDERFORMAT, WM_RENDERALLFORMATS, WM_DESTROYCLIPBOARD] {
                ChangeWindowMessageFilterEx(window, m, MSGFLT_ALLOW, Some(&mut cf))
                    .map_err(|e| format!("ChangeWindowMessageFilterEx: {e}"))?;
            }
        }
        AddClipboardFormatListener(window).map_err(|e| format!("listener: {e}"))?;
    }
    log(
        start,
        &format!(
            "clip-probe: render {:?}, delay {:?}, UIPI allowed: {}, again: {}",
            opts.render, opts.delay, opts.allow_uipi, opts.again
        ),
    );
    let exclude = opts.exclude;
    let seconds = opts.seconds;
    let ours = put_logged(window, start, exclude)?;
    *STATE.lock().unwrap() = Some(State {
        start,
        opts,
        renders: 0,
        ours,
        reput: false,
    });
    let end = start + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        unsafe {
            MsgWaitForMultipleObjects(None, false, 100, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let reput = {
            let mut g = STATE.lock().unwrap();
            let s = g.as_mut().unwrap();
            std::mem::take(&mut s.reput)
        };
        if reput {
            // After the paste has closed the clipboard.
            std::thread::sleep(Duration::from_millis(300));
            match put_logged(window, start, exclude) {
                Ok(seq) => STATE.lock().unwrap().as_mut().unwrap().ours = seq,
                Err(e) => log(start, &format!("put again: {e}")),
            }
        }
    }
    let renders = STATE.lock().unwrap().as_ref().unwrap().renders;
    log(start, &format!("done: {renders} render asks"));
    // Leaving the clipboard to us would make Windows ask for everything now.
    unsafe {
        if OpenClipboard(Some(window)).is_ok() {
            if GetClipboardOwner().ok() == Some(window) {
                let _ = EmptyClipboard();
            }
            let _ = CloseClipboard();
        }
    }
    Ok(())
}

/// The files on the clipboard (`CF_HDROP`), as a program pasting them sees
/// them; empty if there are none.
pub fn read_files() -> Result<Vec<PathBuf>, String> {
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP).is_err() {
            return Ok(Vec::new());
        }
        OpenClipboard(None).map_err(|e| format!("OpenClipboard: {e}"))?;
        let files = match GetClipboardData(CF_HDROP) {
            Ok(h) if !h.is_invalid() => {
                let drop = HDROP(h.0);
                let n = DragQueryFileW(drop, u32::MAX, None);
                (0..n)
                    .map(|i| {
                        let len = DragQueryFileW(drop, i, None) as usize;
                        let mut buf = vec![0u16; len + 1];
                        DragQueryFileW(drop, i, Some(&mut buf));
                        PathBuf::from(String::from_utf16_lossy(&buf[..len]))
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let _ = CloseClipboard();
        Ok(files)
    }
}

/// Puts these files on the clipboard (`CF_HDROP`, at once), as a copy in
/// Explorer would.
pub fn put_files(files: &[PathBuf]) -> Result<(), String> {
    let h = hdrop(files).ok_or("GlobalAlloc failed")?;
    unsafe {
        OpenClipboard(None).map_err(|e| format!("OpenClipboard: {e}"))?;
        let _ = EmptyClipboard();
        let r = SetClipboardData(CF_HDROP, Some(h));
        let _ = CloseClipboard();
        if let Err(e) = r {
            let _ = GlobalFree(Some(windows::Win32::Foundation::HGLOBAL(h.0)));
            return Err(format!("SetClipboardData: {e}"));
        }
    }
    Ok(())
}

/// The clipboard's sequence number and the formats on it (a look at which
/// clipboard this process sees).
pub fn formats() -> String {
    use windows::Win32::System::DataExchange::{EnumClipboardFormats, GetClipboardFormatNameW};
    unsafe {
        let seq = GetClipboardSequenceNumber();
        if let Err(e) = OpenClipboard(None) {
            return format!("clipboard seq {seq}; OpenClipboard: {e}");
        }
        let mut names = Vec::new();
        let mut f = 0;
        loop {
            f = EnumClipboardFormats(f);
            if f == 0 {
                break;
            }
            let mut buf = [0u16; 128];
            let n = GetClipboardFormatNameW(f, &mut buf) as usize;
            names.push(if n > 0 {
                String::from_utf16_lossy(&buf[..n])
            } else {
                f.to_string()
            });
        }
        let _ = CloseClipboard();
        format!("clipboard seq {seq}; formats: {}", names.join(", "))
    }
}
