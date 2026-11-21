//! The session's clipboard on the host (protocol v5): the client's texts
//! arrive in the input stream and go onto the clipboard before the next input
//! event (a Ctrl+V right after finds them); the host's own copies go to the
//! client as `ControlMessage::Clipboard`, reliable and windowed.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use std::sync::mpsc::Sender;

use desktop::clipboard::{ClipSync, Clipboard, Content, Write, image_to_send, on_files_event};
use desktop::files::{Cmd, Event};
use input::PasteKeys;
use protocol::ControlMessage;
use protocol::clipboard::{ClipAssembler, ClipLog, ClipPiece, ClipTraffic, script_text};
use transport::ReliableSender;

/// Clipboard bytes in flight to the client at most (≈ 55 datagrams): a
/// megabyte goes out window by window, not as one burst.
const WINDOW: usize = 64 << 10;

pub struct HostClipboard {
    sync: ClipSync,
    incoming: ClipAssembler,
    outgoing: ReliableSender<ClipPiece>,
    /// Counts, and the log of texts received / sent (`--clipboard-log`).
    traffic: ClipTraffic,
    /// `--clipboard-script`: texts still to send instead of the clipboard's.
    script: std::ops::Range<u32>,
    /// The files service: files and pictures copied go there as sets.
    pub files: Option<Sender<Cmd>>,
    next_set: u32,
    /// Checks (`--clip-copy`, `--clip-copy-image`): copies as if made here.
    pub copies: Vec<Content>,
    /// Pastes in the client's input (a big set of the client's comes then).
    keys: PasteKeys,
}

impl HostClipboard {
    /// `real`: on the clipboard; otherwise a dry run (texts only logged: the
    /// client is on this machine, they share one clipboard).
    pub fn new(real: bool, log: Option<&Path>, script: u32) -> Result<Self> {
        let board = if real && script == 0 {
            // Only what is copied from now on: the client's text comes first.
            // As SYSTEM, through a helper run as the user (it sees all copies).
            Some(match crate::clip_agent::start() {
                Some(c) => c,
                None => Clipboard::start(false).map_err(anyhow::Error::msg)?,
            })
        } else {
            None
        };
        let log = match log {
            Some(p) => Some(ClipLog::open(p).with_context(|| format!("create {p:?}"))?),
            None => None,
        };
        Ok(HostClipboard {
            sync: ClipSync::new(board),
            incoming: ClipAssembler::new(),
            outgoing: ReliableSender::new(),
            traffic: ClipTraffic::new(log),
            script: 0..script,
            files: None,
            next_set: 1,
            copies: Vec::new(),
            keys: PasteKeys::default(),
        })
    }

    /// A piece from the client's input stream.
    pub fn on_piece(&mut self, piece: ClipPiece) {
        if let Some(text) = self.incoming.push(piece) {
            self.traffic.on_received(&text);
            // Waiting: a Ctrl+V right after finds the text.
            self.sync.apply_remote(Content::Text(text), Write::Now);
        }
    }

    /// The client's input event, before it is applied: a paste while the
    /// mark of a big set of the client's is on the clipboard fetches it.
    pub fn on_input(&mut self, ev: &protocol::InputEvent) {
        if self.keys.is_paste(ev)
            && let Some(set) = self.sync.pending()
            && let Some(f) = &self.files
        {
            let _ = f.send(Cmd::Paste(set));
        }
    }

    /// The files service about the client's sets.
    pub fn on_files_event(&mut self, ev: Event) {
        if let Some(set) = on_files_event(&mut self.sync, ev, Write::Now)
            && let Some(f) = &self.files
        {
            let _ = f.send(Cmd::Placed(set));
        }
    }

    /// What to send the client now: pieces of a new local text, resends.
    /// Files and pictures go to the files service.
    pub fn due(&mut self, now: Instant) -> Vec<ControlMessage> {
        let content = match self.script.next() {
            Some(i) => Some(Content::Text(script_text(i))),
            None if !self.copies.is_empty() => Some(self.copies.remove(0)),
            None => {
                self.sync.poll();
                self.sync.take_local()
            }
        };
        let set = self.next_set;
        let (text, cmd) = match content {
            Some(Content::Text(t)) => (Some(t), None),
            Some(Content::Files(paths)) => (None, Some(Cmd::Copy { set, paths })),
            Some(Content::Image(parts)) => (
                None,
                Some(Cmd::CopyImage {
                    set,
                    formats: image_to_send(parts),
                }),
            ),
            _ => (None, None),
        };
        if let (Some(cmd), Some(f)) = (cmd, &self.files) {
            self.next_set += 1;
            let _ = f.send(cmd);
        }
        if let Some(pieces) = text.and_then(|t| self.traffic.outgoing(&t)) {
            for p in pieces {
                self.outgoing.push(p);
            }
        }
        self.outgoing
            .due_within(now, WINDOW, |p| p.data.len())
            .into_iter()
            .map(|(id, piece)| ControlMessage::Clipboard { id, piece })
            .collect()
    }

    pub fn ack(&mut self, last: u32, now: Instant) {
        self.outgoing.ack(last, now);
    }

    /// Session over: flush the log, say what passed.
    pub fn finish(&mut self) {
        self.traffic.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::clipboard::split_text;

    fn texts_in(msgs: &[ControlMessage]) -> usize {
        msgs.iter()
            .filter(|m| matches!(m, ControlMessage::Clipboard { piece, .. } if piece.last))
            .count()
    }

    #[test]
    fn script_sends_n_texts_then_stops() {
        let mut c = HostClipboard::new(false, None, 3).unwrap();
        let t0 = Instant::now();
        let mut last_id = None;
        let mut texts = 0;
        // Every call may start a text; pieces are held back by the window.
        for _ in 0..200 {
            let msgs = c.due(t0);
            texts += texts_in(&msgs);
            if let Some(ControlMessage::Clipboard { id, .. }) = msgs.last() {
                last_id = Some(*id);
            }
            if let Some(id) = last_id {
                c.ack(id, t0);
            }
        }
        assert_eq!(texts, 3);
        assert_eq!(c.traffic.sent, 3);
        assert!(c.due(t0).is_empty());
    }

    #[test]
    fn pieces_from_the_client_make_a_text() {
        let mut c = HostClipboard::new(false, None, 0).unwrap();
        let text = "ы".repeat(protocol::clipboard::CLIP_PIECE_LEN);
        for p in split_text(&text).unwrap() {
            c.on_piece(p);
        }
        assert_eq!(c.traffic.received, 1);
    }
}
