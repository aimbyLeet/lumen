//! What a side of a session knows about the clipboard: what passed either
//! way (nothing echoes back), the newest local content not sent yet, and
//! whether our mark is still on the clipboard.

use std::path::PathBuf;

use super::{Clipboard, Content, ImageFormat};
use crate::files::Event;

/// When [`ClipSync::apply_remote`] returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Write {
    /// Once it is written (what comes next, a Ctrl+V, finds it).
    Now,
    /// At once: the caller must not wait (a network thread).
    Later,
}

/// What a side of a session knows about the clipboard: the last content it
/// passed either way (never sent back where it came from; a text not twice,
/// files and pictures copied again go again: the last ones may have been
/// cancelled), the newest local one not passed yet, and whether its mark is
/// still on the clipboard.
pub struct ClipSync {
    board: Option<Clipboard>,
    last: Option<u64>,
    /// `last` came from the other side.
    last_remote: bool,
    latest: Option<Content>,
    /// The mark we put, until something else is copied here.
    pending: Option<u32>,
}

/// Tells equal contents apart inside this process only (`DefaultHasher`,
/// not stable across runs): not `protocol::clipboard::text_hash`, which both
/// sides log and compare.
fn dedup_hash(c: &Content) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match c {
        Content::Text(t) => (0u8, t).hash(&mut h),
        Content::Files(f) => (1u8, f).hash(&mut h),
        Content::Image(i) => (2u8, i).hash(&mut h),
        Content::Pending(s) => (3u8, s).hash(&mut h),
    }
    h.finish()
}

impl ClipSync {
    /// `board`: `None` for a dry run (no clipboard: contents only logged).
    pub fn new(board: Option<Clipboard>) -> ClipSync {
        ClipSync {
            board,
            last: None,
            last_remote: false,
            latest: None,
            pending: None,
        }
    }

    /// Takes in what was copied here meanwhile (the newest wins).
    pub fn poll(&mut self) {
        if let Some(c) = self.board.as_ref().and_then(|b| b.changes().pop()) {
            self.pending = None;
            self.latest = Some(c);
        }
    }

    /// The local content to send, if there is a new one.
    pub fn take_local(&mut self) -> Option<Content> {
        let c = self.latest.take()?;
        let h = dedup_hash(&c);
        let again = self.last == Some(h) && (self.last_remote || matches!(c, Content::Text(_)));
        if again || matches!(&c, Content::Text(t) if t.is_empty()) {
            return None;
        }
        self.last = Some(h);
        self.last_remote = false;
        Some(c)
    }

    /// Something from the other side: onto the clipboard (unless it is
    /// already the one there). `how`: whether to return only once it is written.
    pub fn apply_remote(&mut self, c: Content, how: Write) {
        let h = dedup_hash(&c);
        if self.last == Some(h) {
            return;
        }
        self.last = Some(h);
        self.last_remote = true;
        // A local content not sent yet is older than this one.
        self.latest = None;
        self.pending = match c {
            Content::Pending(s) => Some(s),
            _ => None,
        };
        if let Some(b) = &self.board {
            match how {
                Write::Now => {
                    if !b.set(c) {
                        eprintln!(
                            "clipboard: the clipboard is busy (locked?): it goes on it once it is free"
                        );
                    }
                }
                Write::Later => b.set_later(c),
            }
        }
    }

    /// The set whose mark is on the clipboard now (nothing copied since).
    pub fn pending(&mut self) -> Option<u32> {
        self.poll();
        self.pending
    }

    /// The text of a dry run (`None` board) is never read: the caller makes
    /// its own contents (`--clipboard-script`).
    pub fn is_dry_run(&self) -> bool {
        self.board.is_none()
    }

    /// A dry run says what it would have put on the clipboard.
    fn log_dry(&self, what: impl FnOnce() -> String) {
        if self.is_dry_run() {
            println!("clipboard (dry run): would put {}", what());
        }
    }

    /// Set `set` came, still awaited here (its mark on the clipboard):
    /// `content` goes there. Returns it as placed.
    fn place_set(&mut self, set: u32, content: Content, how: Write) -> Option<u32> {
        if self.pending() != Some(set) {
            // Something else was copied here meanwhile: it stays.
            return None;
        }
        self.log_dry(|| match &content {
            Content::Files(paths) => format!("files of set {set}: {}", joined(paths)),
            c => format!("picture of set {set}: {}", c.describe()),
        });
        self.apply_remote(content, how);
        Some(set)
    }
}

fn joined(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// What the other side sent, onto our clipboard: a set's mark, then the set.
/// Returns the set to report as placed, if any.
pub fn on_files_event(sync: &mut ClipSync, ev: Event, how: Write) -> Option<u32> {
    match ev {
        Event::Pending { set, big } => {
            sync.apply_remote(Content::Pending(set), how);
            sync.log_dry(|| format!("mark of set {set}"));
            // A big one stays a mark: placed as it will be.
            big.then_some(set)
        }
        Event::Files(set, paths) => sync.place_set(set, Content::Files(paths), how),
        Event::Image(set, formats) => {
            let parts: Vec<(ImageFormat, Vec<u8>)> = formats
                .into_iter()
                .filter_map(|(f, b)| ImageFormat::from_u8(f).map(|f| (f, b)))
                .collect();
            sync.place_set(set, Content::Image(parts), how)
        }
        Event::Placed(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync() -> ClipSync {
        ClipSync::new(None)
    }

    fn text(s: &str) -> Content {
        Content::Text(s.into())
    }

    #[test]
    fn local_content_is_taken_once_and_empty_text_is_skipped() {
        let mut s = sync();
        s.latest = Some(text("a"));
        assert_eq!(s.take_local(), Some(text("a")));
        // The same copied again is not sent again.
        s.latest = Some(text("a"));
        assert_eq!(s.take_local(), None);
        s.latest = Some(text(""));
        assert_eq!(s.take_local(), None);
        s.latest = Some(Content::Files(vec!["a".into()]));
        assert_eq!(s.take_local(), Some(Content::Files(vec!["a".into()])));
    }

    #[test]
    fn files_copied_again_go_again() {
        let mut s = sync();
        let files = Content::Files(vec!["a".into()]);
        s.latest = Some(files.clone());
        assert_eq!(s.take_local(), Some(files.clone()));
        // Cancelled there, copied again: a paste there must find them.
        s.latest = Some(files.clone());
        assert_eq!(s.take_local(), Some(files.clone()));
        // What came from there is not sent back, files neither.
        let theirs = Content::Files(vec!["b".into()]);
        s.apply_remote(theirs.clone(), Write::Later);
        s.latest = Some(theirs);
        assert_eq!(s.take_local(), None);
    }

    #[test]
    fn remote_content_does_not_echo_back() {
        let mut s = sync();
        s.apply_remote(text("x"), Write::Later);
        s.latest = Some(text("x"));
        assert_eq!(s.take_local(), None);
    }

    #[test]
    fn remote_content_replaces_an_unsent_local_one() {
        let mut s = sync();
        s.latest = Some(text("old"));
        s.apply_remote(text("new"), Write::Now);
        assert_eq!(s.take_local(), None);
    }

    #[test]
    fn same_remote_content_twice_changes_nothing() {
        let mut s = sync();
        s.apply_remote(text("x"), Write::Now);
        s.latest = Some(text("local"));
        s.apply_remote(text("x"), Write::Now);
        // The second one returned early: the local text is still waiting.
        assert_eq!(s.take_local(), Some(text("local")));
    }

    #[test]
    fn the_mark_holds_until_something_else() {
        let mut s = sync();
        s.apply_remote(Content::Pending(3), Write::Later);
        assert_eq!(s.pending(), Some(3));
        s.apply_remote(text("y"), Write::Later);
        assert_eq!(s.pending(), None);
    }

    #[test]
    fn a_set_goes_on_the_clipboard_only_over_its_mark() {
        let mut s = sync();
        let files = |set| Event::Files(set, vec!["a".into()]);
        assert_eq!(
            on_files_event(&mut s, files(1), Write::Later),
            None,
            "no mark"
        );
        let pending = Event::Pending { set: 2, big: false };
        assert_eq!(on_files_event(&mut s, pending, Write::Later), None);
        assert_eq!(
            on_files_event(&mut s, files(1), Write::Later),
            None,
            "older"
        );
        assert_eq!(on_files_event(&mut s, files(2), Write::Later), Some(2));
        let big = Event::Pending { set: 3, big: true };
        assert_eq!(on_files_event(&mut s, big, Write::Later), Some(3));
    }
}
