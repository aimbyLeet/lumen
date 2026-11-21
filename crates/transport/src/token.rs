//! A host's token: 120 random bits the host shows its user, who gives it to
//! the clients that may connect.
//!
//! Two values come from it, each a keyed BLAKE2s with its own label so that
//! one says nothing about the other:
//! * [`lookup`](HostToken::lookup) — the server finds the host by it;
//! * [`psk`](HostToken::psk) — the pre-shared key of the Noise handshake.
//!
//! The server only ever sees the lookup, so it cannot open a session itself.
//!
//! Written as six groups of four Crockford base32 digits
//! (`7K3Q-0M9X-…`): no I, L, O or U; parsing ignores case, spaces and dashes
//! (and line ends) and reads I/L as 1 and O as 0.

use std::fmt;
use std::str::FromStr;

use blake2::Blake2sMac256;
use blake2::digest::Mac;

pub const TOKEN_LEN: usize = 15;
const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Base32 digits of a token (5 bits each).
const TOKEN_DIGITS: usize = TOKEN_LEN * 8 / 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HostToken([u8; TOKEN_LEN]);

impl HostToken {
    pub fn generate() -> HostToken {
        let mut b = [0u8; TOKEN_LEN];
        getrandom::fill(&mut b).expect("system random");
        HostToken(b)
    }

    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let mut mac = <Blake2sMac256 as blake2::digest::KeyInit>::new_from_slice(&self.0)
            .expect("key of 15 bytes");
        mac.update(label);
        mac.finalize().into_bytes().into()
    }

    /// What the server knows the host by.
    pub fn lookup(&self) -> [u8; 32] {
        self.derive(b"lumen host lookup v1")
    }

    /// The handshake's pre-shared key.
    pub fn psk(&self) -> [u8; 32] {
        self.derive(b"lumen handshake psk v1")
    }
}

impl fmt::Display for HostToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut acc = 0u32;
        let mut bits = 0;
        let mut n = 0;
        for &b in &self.0 {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                if n > 0 && n % 4 == 0 {
                    f.write_str("-")?;
                }
                let d = DIGITS[((acc >> bits) & 31) as usize];
                write!(f, "{}", d as char)?;
                n += 1;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for HostToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostToken(…)")
    }
}

impl FromStr for HostToken {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let mut out = [0u8; TOKEN_LEN];
        let (mut acc, mut bits, mut digits, mut at) = (0u32, 0, 0, 0);
        for c in s.chars().filter(|c| *c != '-' && !c.is_whitespace()) {
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let v = DIGITS
                .iter()
                .position(|&d| d as char == c)
                .ok_or_else(|| format!("{c:?} is not a token character"))?;
            digits += 1;
            if digits > TOKEN_DIGITS {
                return Err(format!("a token has {TOKEN_DIGITS} characters"));
            }
            acc = (acc << 5) | v as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out[at] = (acc >> bits) as u8;
                at += 1;
            }
        }
        if digits != TOKEN_DIGITS {
            return Err(format!("a token has {TOKEN_DIGITS} characters"));
        }
        Ok(HostToken(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        for _ in 0..100 {
            let t = HostToken::generate();
            let s = t.to_string();
            assert_eq!(s.len(), 29, "{s}");
            assert_eq!(s.matches('-').count(), 5);
            assert_eq!(s.parse::<HostToken>().unwrap(), t);
            // Typed by a human: lower case, spaces, O for 0, l for 1.
            let sloppy = s
                .to_lowercase()
                .replace('-', " ")
                .replace('0', "o")
                .replace('1', "l");
            assert_eq!(sloppy.parse::<HostToken>().unwrap(), t);
            // As read from a file.
            assert_eq!(format!("{s}\r\n").parse::<HostToken>().unwrap(), t);
        }
        assert!("ABCD".parse::<HostToken>().is_err());
        assert!(
            "UUUU-UUUU-UUUU-UUUU-UUUU-UUUU"
                .parse::<HostToken>()
                .is_err()
        );
        let all = HostToken([0xff; TOKEN_LEN]);
        assert_eq!(all.to_string(), "ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ");
        assert!(
            "ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-Z"
                .parse::<HostToken>()
                .is_err()
        );
    }

    #[test]
    fn lookup_and_psk_differ_per_token() {
        let (a, b) = (HostToken::generate(), HostToken::generate());
        assert_ne!(a.lookup(), a.psk());
        assert_ne!(a.lookup(), b.lookup());
        assert_ne!(a.psk(), b.psk());
        assert_eq!(
            a.lookup(),
            a.to_string().parse::<HostToken>().unwrap().lookup()
        );
    }
}
