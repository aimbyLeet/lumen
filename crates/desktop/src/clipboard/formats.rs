//! What the Windows clipboard holds, read and written: files (`CF_HDROP`),
//! text (`CF_UNICODETEXT`), pictures (PNG, `CF_DIBV5`, `CF_DIB` byte for
//! byte), our mark (`Lumen.Pending`).

use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::w;

use super::{Content, ImageFormat};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
/// How many times `open` tries for a new text: another program may hold the
/// clipboard for a moment.
pub(super) const OPEN_TRIES: u32 = 10;

impl ImageFormat {
    fn id(self) -> u32 {
        match self {
            ImageFormat::Png => unsafe { RegisterClipboardFormatW(w!("PNG")) },
            ImageFormat::DibV5 => CF_DIBV5,
            ImageFormat::Dib => CF_DIB,
        }
    }
}

/// The clipboard, open for `window`; closed on drop.
struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Opens the clipboard for `window`, `tries` times 10 ms apart at most.
fn open(window: HWND, tries: u32) -> Option<Open> {
    for i in 0..tries {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        if unsafe { OpenClipboard(Some(window)) }.is_ok() {
            return Some(Open);
        }
    }
    None
}

fn pending_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Lumen.Pending")) }
}

fn available(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// The bytes of `format` (the clipboard open).
fn bytes(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(format).ok().filter(|h| !h.is_invalid())?;
        let g = HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// What a change put on the clipboard: `Ok(None)` if nothing we pass on (or
/// our own mark), `Err` if it could not be opened.
pub(super) fn read(window: HWND) -> Result<Option<Content>, ()> {
    if available(pending_format()) {
        return Ok(None);
    }
    let files = available(CF_HDROP);
    let text = available(CF_UNICODETEXT);
    let images: Vec<ImageFormat> = ImageFormat::ALL
        .into_iter()
        .filter(|f| available(f.id()))
        .collect();
    if !files && !text && images.is_empty() {
        return Ok(None);
    }
    let _open = open(window, OPEN_TRIES).ok_or(())?;
    if files {
        return Ok(read_files().map(Content::Files));
    }
    if text {
        return Ok(read_text().map(Content::Text));
    }
    let parts: Vec<(ImageFormat, Vec<u8>)> = images
        .into_iter()
        .filter_map(|f| bytes(f.id()).map(|b| (f, b)))
        .collect();
    Ok((!parts.is_empty()).then_some(Content::Image(parts)))
}

fn read_text() -> Option<String> {
    let b = bytes(CF_UNICODETEXT)?;
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]))
}

fn read_files() -> Option<Vec<PathBuf>> {
    unsafe {
        let h = GetClipboardData(CF_HDROP)
            .ok()
            .filter(|h| !h.is_invalid())?;
        let drop = HDROP(h.0);
        let n = DragQueryFileW(drop, u32::MAX, None);
        Some(
            (0..n)
                .map(|i| {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(drop, i, Some(&mut buf));
                    PathBuf::from(String::from_utf16_lossy(&buf[..len]))
                })
                .collect(),
        )
    }
}

/// A movable global block with `data`; the clipboard takes it on success.
fn global(data: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)).ok()?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return None;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        let _ = GlobalUnlock(g);
        Some(g)
    }
}

/// Puts `data` as `format` (the clipboard open and emptied).
fn put(format: u32, data: &[u8]) -> bool {
    let Some(g) = global(data) else {
        return false;
    };
    unsafe {
        if SetClipboardData(format, Some(HANDLE(g.0))).is_err() {
            let _ = GlobalFree(Some(g));
            return false;
        }
    }
    true
}

/// A `CF_HDROP` block: `DROPFILES` + wide paths, each ending in 0, then 0.
fn drop_files(files: &[PathBuf]) -> Vec<u8> {
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt: POINT,
        f_nc: i32,
        f_wide: i32,
    }
    let head = std::mem::size_of::<DropFiles>();
    let df = DropFiles {
        p_files: head as u32,
        pt: POINT::default(),
        f_nc: 0,
        f_wide: 1,
    };
    let mut out =
        unsafe { std::slice::from_raw_parts(&df as *const DropFiles as *const u8, head).to_vec() };
    for f in files {
        for u in f.as_os_str().to_string_lossy().encode_utf16().chain([0]) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Puts `c` on the clipboard (opening it at most `tries` times).
pub(super) fn write(window: HWND, c: &Content, tries: u32) -> bool {
    let Some(_open) = open(window, tries) else {
        return false;
    };
    unsafe {
        let _ = EmptyClipboard();
    }
    match c {
        Content::Text(t) => {
            let bytes: Vec<u8> = t
                .encode_utf16()
                .chain([0])
                .flat_map(|u| u.to_le_bytes())
                .collect();
            put(CF_UNICODETEXT, &bytes)
        }
        Content::Files(f) => {
            // Copied, not moved (Explorer's "Preferred DropEffect": copy).
            let effect = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
            put(CF_HDROP, &drop_files(f)) && put(effect, &1u32.to_le_bytes())
        }
        Content::Image(parts) => parts.iter().all(|(f, b)| put(f.id(), b)),
        Content::Pending(s) => put(pending_format(), &s.to_le_bytes()),
    }
}
