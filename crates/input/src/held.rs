//! What the client holds down on the host: keys and mouse buttons pressed and
//! not yet released. When the client window loses the focus (the Windows key
//! opens the local Start menu, Alt+Tab, a click elsewhere), the release goes
//! to another window and never reaches the host, which keeps the key down
//! (a stuck Windows key turns 1, 2, 3 into Win+1… on the host). The client
//! then sends the releases itself.

use protocol::{InputEvent, MouseButton};

#[derive(Debug, Default)]
pub struct Held {
    keys: Vec<(u16, bool)>,
    buttons: Vec<MouseButton>,
}

impl Held {
    /// Notes an event sent to the host.
    pub fn track(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Key {
                scancode,
                extended,
                down,
            } => {
                let k = (scancode, extended);
                self.keys.retain(|&h| h != k);
                if down {
                    self.keys.push(k);
                }
            }
            InputEvent::MouseButton { button, down } => {
                self.buttons.retain(|&b| b != button);
                if down {
                    self.buttons.push(button);
                }
            }
            InputEvent::MouseMove { .. }
            | InputEvent::MouseWheel { .. }
            | InputEvent::SecureAttention
            | InputEvent::Clipboard(_)
            | InputEvent::File(_) => {}
        }
    }

    /// The releases of everything held, last pressed first; forgets it.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let keys = self
            .keys
            .drain(..)
            .rev()
            .map(|(scancode, extended)| InputEvent::Key {
                scancode,
                extended,
                down: false,
            });
        let buttons = self
            .buttons
            .drain(..)
            .rev()
            .map(|button| InputEvent::MouseButton {
                button,
                down: false,
            });
        keys.chain(buttons).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(scancode: u16, down: bool) -> InputEvent {
        InputEvent::Key {
            scancode,
            extended: scancode == 0x5B,
            down,
        }
    }

    #[test]
    fn releases_what_is_still_down() {
        let mut h = Held::default();
        // Win (0x5B, extended) down, its release lost; Shift down and up; a repeat of A.
        for ev in [
            key(0x5B, true),
            key(0x2A, true),
            key(0x2A, false),
            key(0x1E, true),
            key(0x1E, true),
        ] {
            h.track(&ev);
        }
        h.track(&InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        });
        assert_eq!(
            h.release_all(),
            vec![
                key(0x1E, false),
                key(0x5B, false),
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: false
                },
            ]
        );
        assert!(h.release_all().is_empty());
    }
}
