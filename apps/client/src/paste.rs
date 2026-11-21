//! Input held back after a paste typed in the video window while a
//! clipboard set copied here is still on its way to the host (files,
//! pictures): the Ctrl+V reaches the host only once the
//! set is on its clipboard, as with text (which goes in the input stream
//! itself). Everything after the paste waits with it, in order.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use input::PasteKeys;
use protocol::InputEvent;

pub use input::ctrl_v;

/// At most this long: a set that does not land (cancelled, refused) must
/// not keep the keyboard.
const HOLD_MAX: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct Hold {
    keys: PasteKeys,
    /// Our newest set, not on the host's clipboard yet.
    unplaced: Option<u32>,
    /// Holding since, and what waits.
    since: Option<Instant>,
    held: VecDeque<InputEvent>,
}

impl Hold {
    /// A set copied here went to the host.
    pub fn sent(&mut self, set: u32) {
        self.unplaced = Some(set);
    }

    /// The host put set `set` on its clipboard.
    pub fn placed(&mut self, set: u32) {
        if self.unplaced == Some(set) {
            self.unplaced = None;
            if self.since.take().is_some() {
                println!("clipboard: set {set} on the host's clipboard: input goes on");
            }
        }
    }

    /// New input events.
    pub fn extend(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        for ev in events {
            if self.since.is_none()
                && self.keys.is_paste(&ev)
                && let Some(set) = self.unplaced
            {
                println!("clipboard: a paste while set {set} is on its way: input waits for it");
                self.since = Some(Instant::now());
            }
            self.held.push_back(ev);
        }
    }

    /// What may go now.
    pub fn release(&mut self, now: Instant) -> Vec<InputEvent> {
        if let Some(t) = self.since {
            if now.duration_since(t) < HOLD_MAX {
                return Vec::new();
            }
            println!("clipboard: the set did not land in {HOLD_MAX:?}: input goes on");
            self.since = None;
            self.unplaced = None;
        }
        self.held.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(down: bool) -> InputEvent {
        InputEvent::Key {
            scancode: 0x1E,
            extended: false,
            down,
        }
    }

    #[test]
    fn a_paste_waits_for_the_set() {
        let mut h = Hold::default();
        let t = Instant::now();
        h.extend([key(true)]);
        assert_eq!(h.release(t).len(), 1, "nothing on its way");
        h.sent(3);
        h.extend([key(false)]);
        assert_eq!(h.release(t).len(), 1, "no paste yet");
        h.extend(ctrl_v());
        h.extend([key(true)]);
        assert!(h.release(t).is_empty(), "held");
        h.placed(2);
        assert!(h.release(t).is_empty(), "another set");
        h.placed(3);
        assert_eq!(
            h.release(t).len(),
            5,
            "the paste and what followed, in order"
        );
        // Not landing: let go after a while.
        h.sent(4);
        h.extend(ctrl_v());
        assert!(h.release(t).is_empty());
        assert_eq!(h.release(Instant::now() + HOLD_MAX).len(), 4);
    }
}
