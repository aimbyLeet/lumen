//! Clipboard text in pieces (protocol v5).
//!
//! A text copied on one side goes to the other as consecutive [`ClipPiece`]s
//! over a reliable, ordered channel: client → host inside the input stream
//! ([`InputEvent::Clipboard`](crate::InputEvent::Clipboard), so a Ctrl+V
//! typed right after copying finds the new text), host → client as
//! [`ControlMessage::Clipboard`](crate::ControlMessage::Clipboard). The last
//! piece of a text has `last` set; UTF-8 bytes are cut anywhere, the
//! receiver decodes the whole text.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::{AUTH_TAG_LEN, DEFAULT_MAX_DATAGRAM, HEADER_LEN};

/// Longest text that is passed on (UTF-8 bytes); longer ones are not.
pub const MAX_CLIPBOARD_TEXT: usize = 1 << 20;

/// Bytes of text in one piece: a piece with its message (type, id, event
/// type, `last`, length: 9 bytes) fills a datagram of [`DEFAULT_MAX_DATAGRAM`].
pub const CLIP_PIECE_LEN: usize = DEFAULT_MAX_DATAGRAM - HEADER_LEN - AUTH_TAG_LEN - 9;

/// One piece of a clipboard text.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipPiece {
    /// The text's last piece.
    pub last: bool,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for ClipPiece {
    // The bytes are the user's text: logs get their length only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClipPiece {{ last: {}, len: {} }}",
            self.last,
            self.data.len()
        )
    }
}

/// `text` in pieces; `None` if it is longer than [`MAX_CLIPBOARD_TEXT`].
pub fn split_text(text: &str) -> Option<Vec<ClipPiece>> {
    let bytes = text.as_bytes();
    if bytes.len() > MAX_CLIPBOARD_TEXT {
        return None;
    }
    if bytes.is_empty() {
        return Some(vec![ClipPiece {
            last: true,
            data: Vec::new(),
        }]);
    }
    let n = bytes.len().div_ceil(CLIP_PIECE_LEN);
    Some(
        bytes
            .chunks(CLIP_PIECE_LEN)
            .enumerate()
            .map(|(i, c)| ClipPiece {
                last: i + 1 == n,
                data: c.to_vec(),
            })
            .collect(),
    )
}

/// FNV-1a of a text: what the clipboard logs of both sides compare.
pub fn text_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A log line for a text: its length and hash (never the text).
pub fn text_log_line(text: &str) -> String {
    format!("{} {:016x}", text.len(), text_hash(text))
}

/// The clipboard log of a side (`--clipboard-log`): a line per text, "R|S
/// <bytes> <hash>", which the checks compare between host and client.
pub struct ClipLog(BufWriter<File>);

impl ClipLog {
    pub fn open(path: &Path) -> io::Result<Self> {
        File::create(path).map(|f| Self(BufWriter::new(f)))
    }

    /// `dir`: `'R'` received, `'S'` sent. A failed write is not worth a session.
    pub fn line(&mut self, dir: char, text: &str) {
        let _ = writeln!(self.0, "{dir} {}", text_log_line(text));
    }

    pub fn flush(&mut self) {
        let _ = self.0.flush();
    }
}

/// What a side counts and logs of the texts it passes: the same on both sides.
#[derive(Default)]
pub struct ClipTraffic {
    log: Option<ClipLog>,
    pub received: u64,
    pub sent: u64,
}

impl ClipTraffic {
    pub fn new(log: Option<ClipLog>) -> Self {
        ClipTraffic {
            log,
            received: 0,
            sent: 0,
        }
    }

    /// A text arrived whole.
    pub fn on_received(&mut self, text: &str) {
        self.received += 1;
        if let Some(log) = self.log.as_mut() {
            log.line('R', text);
        }
    }

    /// A local text to send: its pieces, counted and logged; `None` (said
    /// aloud) if it is over [`MAX_CLIPBOARD_TEXT`].
    pub fn outgoing(&mut self, text: &str) -> Option<Vec<ClipPiece>> {
        let Some(pieces) = split_text(text) else {
            eprintln!(
                "clipboard: a text of {} bytes is not sent (over 1 MiB)",
                text.len()
            );
            return None;
        };
        self.sent += 1;
        if let Some(log) = self.log.as_mut() {
            log.line('S', text);
        }
        Some(pieces)
    }

    /// Session over: flush the log, say what passed.
    pub fn finish(&mut self) {
        if let Some(log) = self.log.as_mut() {
            log.flush();
        }
        println!(
            "clipboard: {} texts sent, {} received",
            self.sent, self.received
        );
    }
}

/// Text number `i` of `--clipboard-script` (checks): short and long, ASCII,
/// Cyrillic, emoji; every 50th is a megabyte.
pub fn script_text(i: u32) -> String {
    let words = [
        "цвет",
        "color",
        "🎨",
        "10 бит",
        "BT.709",
        "4:2:0",
        "\r\n",
        "—",
    ];
    let len = match i % 50 {
        49 => MAX_CLIPBOARD_TEXT / 8,
        k => (k as usize * 37) % 900 + 1,
    };
    let mut s = format!("{i}: ");
    let mut k = i as usize;
    while s.len() < len * 4 {
        s.push_str(words[k % words.len()]);
        s.push(' ');
        k = k.wrapping_mul(31).wrapping_add(7);
    }
    let mut end = len * 4;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end.min(MAX_CLIPBOARD_TEXT));
    s
}

/// Puts the pieces back together. A text that grows past
/// [`MAX_CLIPBOARD_TEXT`] (a hostile peer) or is not UTF-8 is dropped.
#[derive(Default)]
pub struct ClipAssembler {
    buf: Vec<u8>,
    too_long: bool,
}

impl ClipAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the next piece; returns the text when it was its last.
    pub fn push(&mut self, piece: ClipPiece) -> Option<String> {
        if !self.too_long {
            if self.buf.len() + piece.data.len() > MAX_CLIPBOARD_TEXT {
                self.too_long = true;
                self.buf = Vec::new();
            } else {
                self.buf.extend_from_slice(&piece.data);
            }
        }
        if !piece.last {
            return None;
        }
        let too_long = std::mem::take(&mut self.too_long);
        let bytes = std::mem::take(&mut self.buf);
        if too_long {
            return None;
        }
        String::from_utf8(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(text: &str) -> String {
        let mut a = ClipAssembler::new();
        let pieces = split_text(text).unwrap();
        let n = pieces.len();
        let mut out = None;
        for (i, p) in pieces.into_iter().enumerate() {
            assert!(p.data.len() <= CLIP_PIECE_LEN);
            assert_eq!(p.last, i + 1 == n);
            out = a.push(p);
            assert_eq!(out.is_some(), i + 1 == n);
        }
        out.unwrap()
    }

    #[test]
    fn texts_come_back_whole() {
        for text in [
            "",
            "a",
            "Привет, мир! 👋🏽 — «цвет» 10 бит",
            &"ы".repeat(CLIP_PIECE_LEN), // a two-byte letter cut at every piece edge
            &"🎨".repeat(MAX_CLIPBOARD_TEXT / 4),
        ] {
            assert_eq!(roundtrip(text), text);
        }
    }

    #[test]
    fn too_long_texts_are_not_passed_on() {
        assert!(split_text(&"x".repeat(MAX_CLIPBOARD_TEXT + 1)).is_none());
        assert!(split_text(&"x".repeat(MAX_CLIPBOARD_TEXT)).is_some());
        // A peer that sends more anyway: that text is dropped, the next one is fine.
        let mut a = ClipAssembler::new();
        let big = vec![b'x'; CLIP_PIECE_LEN];
        for _ in 0..=MAX_CLIPBOARD_TEXT / CLIP_PIECE_LEN {
            let p = ClipPiece {
                last: false,
                data: big.clone(),
            };
            assert_eq!(a.push(p), None);
        }
        let end = ClipPiece {
            last: true,
            data: b"y".to_vec(),
        };
        assert_eq!(a.push(end), None);
        let next = ClipPiece {
            last: true,
            data: b"ok".to_vec(),
        };
        assert_eq!(a.push(next).as_deref(), Some("ok"));
    }

    #[test]
    fn script_texts_fit() {
        for i in 0..100 {
            let t = script_text(i);
            assert!(!t.is_empty() && t.len() <= MAX_CLIPBOARD_TEXT, "{i}");
            assert_eq!(roundtrip(&t), t);
        }
        assert_eq!(script_text(49).len(), MAX_CLIPBOARD_TEXT / 2);
        assert_ne!(text_hash("a"), text_hash("b"));
    }

    #[test]
    fn text_hash_is_fnv1a() {
        // Published FNV-1a 64 vectors.
        assert_eq!(text_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(text_hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(text_hash("foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn log_line_has_length_and_hash_only() {
        assert_eq!(text_log_line("a"), "1 af63dc4c8601ec8c");
        assert_eq!(text_log_line("ы"), format!("2 {:016x}", text_hash("ы")));
    }

    #[test]
    fn piece_debug_hides_the_text() {
        let p = ClipPiece {
            last: true,
            data: b"secret".to_vec(),
        };
        let s = format!("{p:?}");
        assert!(!s.contains("secret") && !s.contains("115"), "{s}");
        assert_eq!(s, "ClipPiece { last: true, len: 6 }");
    }

    #[test]
    fn traffic_counts_and_refuses_long_texts() {
        let mut t = ClipTraffic::new(None);
        assert_eq!(t.outgoing("abc").unwrap().len(), 1);
        assert!(t.outgoing(&"x".repeat(MAX_CLIPBOARD_TEXT + 1)).is_none());
        t.on_received("abc");
        assert_eq!((t.sent, t.received), (1, 1));
    }

    #[test]
    fn broken_utf8_is_dropped() {
        let mut a = ClipAssembler::new();
        let bad = ClipPiece {
            last: true,
            data: vec![0xd0],
        };
        assert_eq!(a.push(bad), None);
    }
}
