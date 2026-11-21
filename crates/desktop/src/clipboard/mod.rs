//! The Windows clipboard, watched and written from a thread of its own (host
//! and client: text since protocol v5, files and pictures since v8).
//!
//! The thread owns a message-only window: writing needs an owner window, and
//! a clipboard owner must answer messages (another program's `EmptyClipboard`
//! waits for it). Changes are found by the clipboard's sequence number, read
//! every [`POLL`]; the number our own write produced is skipped, so what came
//! from the other side does not go back.
//!
//! What is taken from a change, by priority: files (`CF_HDROP`, a copy in
//! Explorer), text (`CF_UNICODETEXT`: a copy in a table may carry a picture
//! of it too — the text is what is meant), a picture (PNG, `CF_DIBV5`,
//! `CF_DIB`: the formats themselves, byte for byte — no conversion, no colour
//! change). Our own mark ([`Content::Pending`]: files on the other side, the
//! clipboard holds nothing else) is never taken.
//!
//! Its parts: `thread` (the window and its loop), `formats` (reading and
//! writing the clipboard's formats), `sync` ([`ClipSync`]: what passed either
//! way), `agent` (the clipboard of a `clip-agent` over pipes).

mod agent;
mod formats;
mod sync;
mod thread;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_NULL};

pub use agent::{decode_content, encode_content, serve_agent};
pub use sync::{ClipSync, Write, on_files_event};

/// How often the sequence number is looked at.
const POLL: Duration = Duration::from_millis(100);
/// How long `set` waits for the write.
const SET_WAIT: Duration = Duration::from_millis(500);

/// A picture format on the clipboard (the wire's numbers, protocol v8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png = 1,
    DibV5 = 2,
    Dib = 3,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::DibV5, ImageFormat::Dib];

    pub fn from_u8(v: u8) -> Option<ImageFormat> {
        Self::ALL.into_iter().find(|f| *f as u8 == v)
    }
}

/// What a copy put on the clipboard (or what goes onto it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Files(Vec<PathBuf>),
    /// The picture's formats, each as the clipboard holds it.
    Image(Vec<(ImageFormat, Vec<u8>)>),
    /// Files copied on the other side, set number `.0`: the clipboard holds
    /// nothing else (a paste here pastes nothing old; it starts the transfer).
    Pending(u32),
}

impl Content {
    /// For logs: what, how much (not the user's data).
    pub fn describe(&self) -> String {
        match self {
            Content::Text(t) => format!("text, {} bytes", t.len()),
            Content::Files(f) => format!("{} files/folders", f.len()),
            Content::Image(i) => format!(
                "picture: {}",
                i.iter()
                    .map(|(f, b)| format!("{f:?} {} bytes", b.len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Content::Pending(s) => format!("mark of set {s}"),
        }
    }
}

/// A picture copied goes to the other side if its formats come to at most
/// this much (a 4K picture as a DIB is 33 MB); a bigger one goes as PNG only,
/// if it has one that fits.
pub const IMAGE_LIMIT: usize = 64 << 20;

/// A picture's formats that go: all of them, or the PNG alone if all are
/// too much (or nothing).
pub fn image_to_send(parts: Vec<(ImageFormat, Vec<u8>)>) -> Vec<(u8, Vec<u8>)> {
    let total: usize = parts.iter().map(|(_, b)| b.len()).sum();
    if total <= IMAGE_LIMIT {
        return parts.into_iter().map(|(f, b)| (f as u8, b)).collect();
    }
    parts
        .into_iter()
        .filter(|(f, b)| *f == ImageFormat::Png && b.len() <= IMAGE_LIMIT)
        .map(|(f, b)| (f as u8, b))
        .collect()
}

enum Ask {
    /// Write this; say when done (true: written).
    Set(Content, Sender<bool>),
    Stop,
}

/// The clipboard thread. Dropping it stops the thread.
pub struct Clipboard {
    changes: Receiver<Content>,
    asks: Sender<Ask>,
    /// The thread's window (`HWND` is not `Send`): woken through it.
    window: isize,
    thread: Option<JoinHandle<()>>,
}

impl Clipboard {
    /// Starts the thread. `with_current`: its first change is what is on the
    /// clipboard now; otherwise only later ones.
    pub fn start(with_current: bool) -> Result<Clipboard, String> {
        let (changes_tx, changes) = mpsc::channel();
        let (asks, asks_rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || thread::run(changes_tx, asks_rx, ready_tx, with_current))
            .map_err(|e| e.to_string())?;
        let window = ready
            .recv()
            .map_err(|_| "the clipboard thread stopped".to_string())??;
        Ok(Clipboard {
            changes,
            asks,
            window,
            thread: Some(thread),
        })
    }

    /// Copied here since the last call (not what [`set`](Self::set) wrote).
    pub fn changes(&self) -> Vec<Content> {
        self.changes.try_iter().collect()
    }

    /// Puts `c` on the clipboard; returns once it is there (or after
    /// [`SET_WAIT`]): what comes after it — a Ctrl+V — finds it.
    pub fn set(&self, c: Content) -> bool {
        match self.ask_set(c) {
            Some(done) => done.recv_timeout(SET_WAIT).unwrap_or(false),
            None => false,
        }
    }

    /// Puts `c` on the clipboard, without waiting.
    pub fn set_later(&self, c: Content) {
        let _ = self.ask_set(c);
    }

    /// Hands `c` to the thread; the receiver gets whether it was written.
    /// `None`: the thread is gone.
    fn ask_set(&self, c: Content) -> Option<Receiver<bool>> {
        let (done_tx, done) = mpsc::channel();
        self.asks.send(Ask::Set(c, done_tx)).ok()?;
        self.wake();
        Some(done)
    }

    fn wake(&self) {
        if self.window == 0 {
            // Over pipes: no window to wake.
            return;
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.window as *mut _)),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = self.asks.send(Ask::Stop);
        self.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};

    /// Uses this machine's clipboard: run by hand,
    /// `cargo test -p desktop -- --ignored clipboard`.
    #[test]
    #[ignore]
    fn clipboard_changes_and_own_writes() {
        let a = Clipboard::start(false).unwrap();
        let b = Clipboard::start(false).unwrap();
        // b writes: a sees it, b does not.
        assert!(b.set(Content::Text("lumen: проверка буфера 🎨".into())));
        std::thread::sleep(POLL * 3);
        assert_eq!(
            a.changes(),
            [Content::Text("lumen: проверка буфера 🎨".into())]
        );
        assert!(b.changes().is_empty(), "its own write");
        // Files, with a Cyrillic name.
        let dir = std::env::temp_dir().join("lumen-clip-test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("кадр.exr");
        std::fs::write(&f, b"x").unwrap();
        assert!(a.set(Content::Files(vec![f.clone(), dir.clone()])));
        std::thread::sleep(POLL * 3);
        assert_eq!(b.changes(), [Content::Files(vec![f, dir])]);
        // A picture, byte for byte (a 2x2 BITMAPINFOHEADER DIB).
        let mut dib = Vec::new();
        for v in [40u32, 2, 2] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend((0..16).map(|i| i as u8 * 13));
        assert!(a.set(Content::Image(vec![(ImageFormat::Dib, dib.clone())])));
        std::thread::sleep(POLL * 3);
        let got = b.changes();
        let Some(Content::Image(parts)) = got.last() else {
            panic!("{got:?}");
        };
        let (_, d) = parts
            .iter()
            .find(|(f, _)| *f == ImageFormat::Dib)
            .expect("the DIB");
        assert_eq!(&d[..dib.len()], &dib[..]);
        // The mark: not passed on.
        assert!(a.set(Content::Pending(7)));
        std::thread::sleep(POLL * 3);
        assert!(b.changes().is_empty());
        // The clipboard held by another program: b's text goes on it later.
        let (held_tx, held) = mpsc::channel();
        let holder = std::thread::spawn(move || unsafe {
            OpenClipboard(None).unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(600));
            let _ = CloseClipboard();
        });
        held.recv().unwrap();
        assert!(!b.set(Content::Text("потом".into())), "written while held");
        holder.join().unwrap();
        std::thread::sleep(POLL * 3);
        assert_eq!(a.changes(), [Content::Text("потом".into())]);
    }

    #[test]
    fn big_pictures_go_as_png_only() {
        let big = vec![0u8; IMAGE_LIMIT];
        let sent = image_to_send(vec![
            (ImageFormat::Png, vec![1; 10]),
            (ImageFormat::Dib, big),
        ]);
        assert_eq!(sent, vec![(ImageFormat::Png as u8, vec![1; 10])]);
    }
}
