//! Pastes in a stream of input events: Ctrl+V and Shift+Insert, by scan
//! code (a paste starts the transfer of big files, and
//! the client holds its input back after one until a set copied here is on
//! the host's clipboard).

use protocol::InputEvent;

const CTRL: u16 = 0x1D;
const ALT: u16 = 0x38;
const LSHIFT: u16 = 0x2A;
const RSHIFT: u16 = 0x36;
const V: u16 = 0x2F;
/// Insert is E0 52 (52 alone is numpad 0).
const INSERT: u16 = 0x52;

/// Ctrl, Alt, Shift held, as the events say.
#[derive(Default, Clone, Copy, Debug)]
pub struct PasteKeys {
    ctrl: [bool; 2],
    alt: [bool; 2],
    shift: [bool; 2],
}

impl PasteKeys {
    /// Takes `ev` in; true if it is a paste (the key going down).
    pub fn is_paste(&mut self, ev: &InputEvent) -> bool {
        let InputEvent::Key {
            scancode,
            extended,
            down,
        } = *ev
        else {
            return false;
        };
        let side = usize::from(extended);
        match scancode {
            CTRL => self.ctrl[side] = down,
            ALT => self.alt[side] = down,
            LSHIFT => self.shift[0] = down,
            RSHIFT => self.shift[1] = down,
            _ => {}
        }
        let (ctrl, alt, shift) = (
            self.ctrl.contains(&true),
            self.alt.contains(&true),
            self.shift.contains(&true),
        );
        down && !alt
            && ((ctrl && scancode == V) || (shift && !ctrl && scancode == INSERT && extended))
    }
}

/// Ctrl+V as key events (checks).
pub fn ctrl_v() -> Vec<InputEvent> {
    [(CTRL, true), (V, true), (V, false), (CTRL, false)]
        .into_iter()
        .map(|(scancode, down)| InputEvent::Key {
            scancode,
            extended: false,
            down,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(scancode: u16, extended: bool, down: bool) -> InputEvent {
        InputEvent::Key {
            scancode,
            extended,
            down,
        }
    }

    #[test]
    fn pastes() {
        let mut k = PasteKeys::default();
        assert!(!k.is_paste(&key(V, false, true)), "V alone");
        let pastes: Vec<bool> = ctrl_v().iter().map(|e| k.is_paste(e)).collect();
        assert_eq!(pastes, [false, true, false, false]);
        // Shift+Insert (extended), not Shift+numpad 0.
        assert!(!k.is_paste(&key(LSHIFT, false, true)));
        assert!(!k.is_paste(&key(INSERT, false, true)));
        assert!(k.is_paste(&key(INSERT, true, true)));
        assert!(!k.is_paste(&key(LSHIFT, false, false)));
        // Ctrl+Alt+V: not a paste.
        assert!(!k.is_paste(&key(CTRL, true, true)));
        assert!(!k.is_paste(&key(ALT, false, true)));
        assert!(!k.is_paste(&key(V, false, true)));
    }
}
