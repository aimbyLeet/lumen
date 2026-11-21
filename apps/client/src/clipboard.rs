//! The session's clipboard on the client: what is copied here goes to the
//! host, but only while the video window is in front (the passwords copied
//! in other programs stay here); what the host copies comes here.
//!
//! Text (protocol v5) goes in the input stream and comes as
//! `ControlMessage::Clipboard`. Files and pictures (protocol v8)
//! go through the files service as clipboard sets:
//! small ones at once, big ones on a paste on the other side; while a set of
//! the host's is on its way, our clipboard holds its mark ([`Content::Pending`]).

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use desktop::clipboard::{ClipSync, Clipboard, Content, Write, image_to_send, on_files_event};
use desktop::files::{Cmd, Event};
use protocol::clipboard::{ClipAssembler, ClipLog, ClipPiece, ClipTraffic, script_text};
use transport::ReliableReceiver;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Clipboard bytes in flight to the host at most (input waits behind them:
/// a Ctrl+V needs the text first anyway).
pub const WINDOW: usize = 64 << 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum ClipMode {
    /// Shared with the host.
    On,
    /// Texts only logged (--clipboard-log), the clipboard untouched.
    DryRun,
    Off,
}

/// What the network thread needs for the clipboard (`--clipboard*`).
#[derive(Clone, Debug)]
pub struct ClipOptions {
    pub mode: ClipMode,
    /// "R|S <bytes> <hash>" per text received / sent.
    pub log: Option<PathBuf>,
    /// Checks: send this many generated texts instead of the clipboard's.
    pub script: u32,
    /// Checks: copies as if made here (`--clip-copy`, `--clip-copy-image`).
    pub copies: Vec<Content>,
    /// Checks: a paste in another program, this many seconds in.
    pub paste_after: Vec<f64>,
    /// Checks: Ctrl+V typed in the video window, this many seconds in.
    pub host_paste_after: Vec<f64>,
}

/// The foreground window is ours (the video window: the only one this
/// process has).
fn in_front() -> bool {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == std::process::id()
    }
}

pub struct ClientClipboard {
    sync: ClipSync,
    incoming: ClipAssembler,
    receiver: ReliableReceiver<ClipPiece>,
    traffic: ClipTraffic,
    script: std::ops::Range<u32>,
    /// Our clipboard sets are numbered from here.
    next_set: u32,
    /// Checks (`--clip-copy`, `--clip-copy-image`): copies made as if here.
    pub copies: Vec<Content>,
}

/// Where a copy here goes: text in the input stream, a set to the files
/// service.
pub enum Outgoing {
    Text(Vec<ClipPiece>),
    Set(u32),
    Nothing,
}

impl ClientClipboard {
    /// `None` when off.
    pub fn new(opts: &ClipOptions) -> Option<Self> {
        let board = match opts.mode {
            ClipMode::Off => return None,
            // Only what is copied once connected: what was on the clipboard
            // before stays here (the owner's choice).
            ClipMode::On if opts.script == 0 => match Clipboard::start(false) {
                Ok(b) => Some(b),
                Err(e) => {
                    eprintln!("clipboard: {e}");
                    return None;
                }
            },
            ClipMode::On | ClipMode::DryRun => None,
        };
        let log = opts.log.as_ref().and_then(|p| {
            ClipLog::open(p)
                .map_err(|e| eprintln!("--clipboard-log {}: {e}", p.display()))
                .ok()
        });
        Some(ClientClipboard {
            sync: ClipSync::new(board),
            incoming: ClipAssembler::new(),
            receiver: ReliableReceiver::new(),
            traffic: ClipTraffic::new(log),
            script: 0..opts.script,
            next_set: 1,
            copies: opts.copies.clone(),
        })
    }

    /// A new local copy, if there is one: text as pieces for the input
    /// stream; files and pictures handed to the files service as a set.
    pub fn outgoing(&mut self, files: Option<&Sender<Cmd>>) -> Outgoing {
        self.sync.poll();
        let content = match self.script.next() {
            Some(i) => Some(Content::Text(script_text(i))),
            None if !self.copies.is_empty() => Some(self.copies.remove(0)),
            None if in_front() => self.sync.take_local(),
            None => None,
        };
        let set = self.next_set;
        let cmd = match content {
            Some(Content::Text(t)) => {
                return self
                    .traffic
                    .outgoing(&t)
                    .map_or(Outgoing::Nothing, Outgoing::Text);
            }
            Some(Content::Files(paths)) => Cmd::Copy { set, paths },
            Some(Content::Image(parts)) => Cmd::CopyImage {
                set,
                formats: image_to_send(parts),
            },
            Some(Content::Pending(_)) | None => return Outgoing::Nothing,
        };
        let Some(files) = files else {
            return Outgoing::Nothing;
        };
        self.next_set += 1;
        let _ = files.send(cmd);
        Outgoing::Set(set)
    }

    /// Piece `id` from the host; returns the acknowledgement to send.
    pub fn on_piece(&mut self, id: u32, piece: ClipPiece) -> Option<u32> {
        for p in self.receiver.accept(id, piece) {
            if let Some(text) = self.incoming.push(p) {
                self.traffic.on_received(&text);
                // Not waiting: this is the network thread.
                self.sync.apply_remote(Content::Text(text), Write::Later);
            }
        }
        self.receiver.last_delivered()
    }

    /// The files service about the host's sets; returns a set now on our
    /// clipboard (the host is told).
    pub fn on_files_event(&mut self, ev: Event) -> Option<u32> {
        on_files_event(&mut self.sync, ev, Write::Later)
    }

    /// The set whose mark is on our clipboard now.
    pub fn pending(&mut self) -> Option<u32> {
        self.sync.pending()
    }

    /// Session over: flush the log, say what passed.
    pub fn finish(&mut self) {
        self.traffic.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop::clipboard::ImageFormat;
    use protocol::clipboard::split_text;

    fn dry_run(script: u32) -> ClientClipboard {
        let opts = ClipOptions {
            mode: ClipMode::DryRun,
            log: None,
            script,
            copies: Vec::new(),
            paste_after: Vec::new(),
            host_paste_after: Vec::new(),
        };
        ClientClipboard::new(&opts).unwrap()
    }

    #[test]
    fn off_has_no_clipboard() {
        let opts = ClipOptions {
            mode: ClipMode::Off,
            log: None,
            script: 0,
            copies: Vec::new(),
            paste_after: Vec::new(),
            host_paste_after: Vec::new(),
        };
        assert!(ClientClipboard::new(&opts).is_none());
    }

    #[test]
    fn script_sends_n_texts_then_stops() {
        let mut c = dry_run(3);
        for _ in 0..3 {
            let Outgoing::Text(p) = c.outgoing(None) else {
                panic!("a text");
            };
            assert!(p.last().unwrap().last);
        }
        assert_eq!(c.traffic.sent, 3);
    }

    #[test]
    fn pieces_from_the_host_make_a_text_and_are_acknowledged() {
        let mut c = dry_run(0);
        let text = "ы".repeat(protocol::clipboard::CLIP_PIECE_LEN);
        let pieces = split_text(&text).unwrap();
        assert!(pieces.len() > 1);
        let n = pieces.len() as u32;
        let mut ack = None;
        for (id, p) in pieces.into_iter().enumerate() {
            ack = c.on_piece(id as u32, p);
        }
        assert_eq!(ack, Some(n - 1));
        assert_eq!(c.traffic.received, 1);
    }

    #[test]
    fn copies_go_as_sets() {
        let mut c = dry_run(0);
        let (tx, rx) = std::sync::mpsc::channel();
        c.copies.push(Content::Files(vec!["a".into()]));
        c.copies
            .push(Content::Image(vec![(ImageFormat::Png, vec![1, 2])]));
        assert!(matches!(c.outgoing(Some(&tx)), Outgoing::Set(1)));
        assert!(matches!(c.outgoing(Some(&tx)), Outgoing::Set(2)));
        assert!(matches!(rx.try_recv(), Ok(Cmd::Copy { set: 1, .. })));
        assert!(matches!(rx.try_recv(), Ok(Cmd::CopyImage { set: 2, .. })));
    }

    #[test]
    fn a_set_lands_only_while_its_mark_holds() {
        let mut c = dry_run(0);
        assert_eq!(
            c.on_files_event(Event::Pending { set: 4, big: false }),
            None
        );
        assert_eq!(c.pending(), Some(4));
        assert_eq!(
            c.on_files_event(Event::Files(3, vec![])),
            None,
            "an older set"
        );
        assert_eq!(c.on_files_event(Event::Files(4, vec!["x".into()])), Some(4));
        assert_eq!(c.pending(), None);
        // Big: the mark is all, placed at once.
        assert_eq!(
            c.on_files_event(Event::Pending { set: 5, big: true }),
            Some(5)
        );
        assert_eq!(c.pending(), Some(5));
    }
}
