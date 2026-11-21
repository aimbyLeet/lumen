//! End-to-end encryption between devices: Noise IKpsk2 handshake, then
//! ChaCha20-Poly1305 per datagram.
//!
//! * Every device has a static X25519 key ([`StaticKey`]). The client knows the
//!   host's public key in advance (IK, from the server); the host learns the
//!   client's from the first handshake message and admits it only if the
//!   server announced a session for that key.
//! * The pre-shared key is derived from the host's token ([`HostToken::psk`]),
//!   which the server never sees: the server can introduce devices and refuse
//!   access, but cannot open a session on its own.
//! * Handshake: two `PacketKind::Handshake` datagrams. Message 1 (client →
//!   host) carries the client's `Hello`, encrypted (without the psk: the host
//!   reads it to learn the client's key, then picks the psk of its session);
//!   message 2 (host → client) mixes in the psk and completes the key
//!   exchange. A wrong token fails on the client when it reads message 2, and
//!   the keys of both sides differ, so nothing gets through either way.
//! * Transport: the Noise split gives one key per direction. Each datagram body
//!   is sealed with ChaCha20-Poly1305, the 20-byte header (with
//!   `Flags::ENCRYPTED` set) as associated data and the header's `seq` as the
//!   nonce (Noise layout: 4 zero bytes + 64-bit little-endian counter). Senders
//!   never reuse a `seq` under one key; receivers drop replays ([`Opener`]).
//!   Snow's own transport mode is not used because it cannot authenticate the
//!   header — hence the raw split.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use protocol::{AUTH_TAG_LEN, Flags, HEADER_LEN};
use snow::{Builder, HandshakeState};

pub const NOISE_PARAMS: &str = "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
/// Where the psk goes in the pattern (`psk2`: end of message 2).
const PSK_AT: u8 = 2;
pub const KEY_LEN: usize = 32;
/// Byte offsets inside the packet header (see `protocol::header`).
const FLAGS_AT: usize = 4;
const SEQ_AT: usize = 12;
/// Replay window, in datagrams behind the newest one seen.
pub const REPLAY_WINDOW: u64 = 1024;

#[derive(Debug)]
pub enum SecureError {
    Noise(snow::Error),
    /// Too short, wrong flags, or not a datagram of ours.
    Malformed,
    /// Authentication failed: wrong key, tampered, or not encrypted.
    Auth,
    /// Already received (or too old for the replay window).
    Replay,
    Io(io::Error),
}

impl fmt::Display for SecureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecureError::Noise(e) => write!(f, "noise: {e}"),
            SecureError::Malformed => f.write_str("malformed datagram"),
            SecureError::Auth => f.write_str("authentication failed"),
            SecureError::Replay => f.write_str("replayed datagram"),
            SecureError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SecureError {}

impl From<snow::Error> for SecureError {
    fn from(e: snow::Error) -> Self {
        SecureError::Noise(e)
    }
}

impl From<io::Error> for SecureError {
    fn from(e: io::Error) -> Self {
        SecureError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, SecureError>;

fn params() -> snow::params::NoiseParams {
    NOISE_PARAMS.parse().expect("valid noise params")
}

/// A device's public key, written as 64 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub [u8; KEY_LEN]);

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

fn parse_hex(s: &str) -> Option<[u8; KEY_LEN]> {
    let s = s.trim();
    if s.len() != KEY_LEN * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; KEY_LEN];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl FromStr for PublicKey {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        parse_hex(s)
            .map(PublicKey)
            .ok_or_else(|| format!("a public key is {} hex digits", KEY_LEN * 2))
    }
}

/// A device's static key pair.
#[derive(Clone)]
pub struct StaticKey {
    private: [u8; KEY_LEN],
    public: PublicKey,
}

const KEY_FILE_HEADER: &str = "# Lumen device key: keep it private\n";

impl StaticKey {
    pub fn generate() -> Self {
        let kp = Builder::new(params())
            .generate_keypair()
            .expect("X25519 key generation");
        StaticKey {
            private: kp.private.try_into().expect("32-byte private key"),
            public: PublicKey(kp.public.try_into().expect("32-byte public key")),
        }
    }

    pub fn public(&self) -> PublicKey {
        self.public
    }

    /// Reads the key at `path`, or creates the file (and its directory) with a
    /// new key. Returns the key and whether it was just created.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        match fs::read_to_string(path) {
            // Both halves are stored: snow cannot derive the public key.
            Ok(text) => {
                let field = |name: &str| {
                    text.lines()
                        .filter_map(|l| l.trim().split_once(' '))
                        .find(|(k, _)| *k == name)
                        .and_then(|(_, v)| parse_hex(v))
                };
                match (field("private"), field("public")) {
                    (Some(private), Some(public)) => Ok((
                        StaticKey {
                            private,
                            public: PublicKey(public),
                        },
                        false,
                    )),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: not a Lumen key file", path.display()),
                    )
                    .into()),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let key = Self::generate();
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                let private: String = key.private.iter().map(|b| format!("{b:02x}")).collect();
                fs::write(
                    path,
                    format!(
                        "{KEY_FILE_HEADER}private {private}\npublic {}\n",
                        key.public
                    ),
                )?;
                Ok((key, true))
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Client side of the handshake.
pub struct Initiator {
    hs: HandshakeState,
    message: Vec<u8>,
}

impl Initiator {
    /// Builds message 1 carrying `payload` (encrypted to the host).
    pub fn new(
        local: &StaticKey,
        host: &PublicKey,
        psk: &[u8; KEY_LEN],
        payload: &[u8],
    ) -> Result<Self> {
        let mut hs = Builder::new(params())
            .local_private_key(&local.private)?
            .remote_public_key(&host.0)?
            .psk(PSK_AT, psk)?
            .build_initiator()?;
        let mut message = vec![0u8; payload.len() + 128];
        let n = hs.write_message(payload, &mut message)?;
        message.truncate(n);
        Ok(Initiator { hs, message })
    }

    /// Message 1, to send (and resend unchanged until message 2 arrives).
    pub fn message(&self) -> &[u8] {
        &self.message
    }

    /// Reads message 2. On error (also: another psk than the host's) the
    /// handshake cannot continue: start a new one.
    pub fn finish(mut self, reply: &[u8]) -> Result<SessionKeys> {
        let mut payload = vec![0u8; reply.len()];
        self.hs.read_message(reply, &mut payload)?;
        let remote = remote_static(&self.hs)?;
        let (to_responder, to_initiator) = self.hs.dangerously_get_raw_split();
        Ok(SessionKeys {
            send: to_responder,
            recv: to_initiator,
            remote,
        })
    }
}

/// Host side of the handshake: message 1 has been read, the client's key is
/// known; check it, then [`finish`](Self::finish) with its session's psk.
pub struct Responder {
    hs: HandshakeState,
    remote: PublicKey,
    payload: Vec<u8>,
}

impl Responder {
    pub fn read(local: &StaticKey, message: &[u8]) -> Result<Self> {
        let mut hs = Builder::new(params())
            .local_private_key(&local.private)?
            .build_responder()?;
        let mut payload = vec![0u8; message.len()];
        let n = hs.read_message(message, &mut payload)?;
        payload.truncate(n);
        let remote = remote_static(&hs)?;
        Ok(Responder {
            hs,
            remote,
            payload,
        })
    }

    /// The client's static public key.
    pub fn remote(&self) -> PublicKey {
        self.remote
    }

    /// What the client sent along with message 1.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Writes message 2, mixing in `psk`.
    pub fn finish(mut self, psk: &[u8; KEY_LEN]) -> Result<(Vec<u8>, SessionKeys)> {
        self.hs.set_psk(usize::from(PSK_AT), psk)?;
        let mut message = vec![0u8; 128];
        let n = self.hs.write_message(&[], &mut message)?;
        message.truncate(n);
        let (to_responder, to_initiator) = self.hs.dangerously_get_raw_split();
        Ok((
            message,
            SessionKeys {
                send: to_initiator,
                recv: to_responder,
                remote: self.remote,
            },
        ))
    }
}

fn remote_static(hs: &HandshakeState) -> Result<PublicKey> {
    let key = hs.get_remote_static().ok_or(SecureError::Malformed)?;
    Ok(PublicKey(
        key.try_into().map_err(|_| SecureError::Malformed)?,
    ))
}

/// Keys of one established session, one per direction.
pub struct SessionKeys {
    send: [u8; KEY_LEN],
    recv: [u8; KEY_LEN],
    /// The peer's static public key.
    pub remote: PublicKey,
}

impl SessionKeys {
    pub fn split(self) -> (Sealer, Opener) {
        (
            Sealer {
                cipher: ChaCha20Poly1305::new(Key::from_slice(&self.send)),
            },
            Opener {
                cipher: ChaCha20Poly1305::new(Key::from_slice(&self.recv)),
                window: ReplayWindow::default(),
            },
        )
    }
}

fn nonce(seq: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&seq.to_le_bytes());
    *Nonce::from_slice(&n)
}

fn header_seq(datagram: &[u8]) -> u64 {
    u64::from_le_bytes(datagram[SEQ_AT..SEQ_AT + 8].try_into().expect("8 bytes"))
}

/// Encrypts outgoing datagrams. Shareable between threads.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
}

impl Sealer {
    /// Encrypts the body of a complete datagram (header + plaintext body) in
    /// place: sets `Flags::ENCRYPTED` and appends the tag.
    pub fn seal(&self, datagram: &mut Vec<u8>) -> Result<()> {
        if datagram.len() < HEADER_LEN {
            return Err(SecureError::Malformed);
        }
        datagram[FLAGS_AT] |= Flags::ENCRYPTED.0;
        let seq = header_seq(datagram);
        let (header, body) = datagram.split_at_mut(HEADER_LEN);
        let tag = self
            .cipher
            .encrypt_in_place_detached(&nonce(seq), header, body)
            .map_err(|_| SecureError::Malformed)?;
        datagram.extend_from_slice(&tag);
        Ok(())
    }
}

/// Decrypts incoming datagrams and drops replays. One per receiving thread.
pub struct Opener {
    cipher: ChaCha20Poly1305,
    window: ReplayWindow,
}

impl Opener {
    /// Decrypts `datagram` in place; returns the length of the plaintext
    /// datagram (header + body) at its start. Unencrypted datagrams fail.
    pub fn open(&mut self, datagram: &mut [u8]) -> Result<usize> {
        if datagram.len() < HEADER_LEN + AUTH_TAG_LEN {
            return Err(SecureError::Malformed);
        }
        if datagram[FLAGS_AT] & Flags::ENCRYPTED.0 == 0 {
            return Err(SecureError::Auth);
        }
        let seq = header_seq(datagram);
        if !self.window.fresh(seq) {
            return Err(SecureError::Replay);
        }
        let body_end = datagram.len() - AUTH_TAG_LEN;
        let tag = *Tag::from_slice(&datagram[body_end..]);
        let (header, rest) = datagram.split_at_mut(HEADER_LEN);
        let body = &mut rest[..body_end - HEADER_LEN];
        self.cipher
            .decrypt_in_place_detached(&nonce(seq), header, body, &tag)
            .map_err(|_| SecureError::Auth)?;
        self.window.mark(seq);
        Ok(body_end)
    }
}

/// Sliding window over sequence numbers: accepts each one once, drops those
/// more than [`REPLAY_WINDOW`] behind the newest.
#[derive(Default)]
struct ReplayWindow {
    /// Newest accepted seq + 1 (0 = nothing yet).
    top: u64,
    /// Bit i: seq `top - 1 - i` was accepted.
    bits: [u64; (REPLAY_WINDOW / 64) as usize],
}

impl ReplayWindow {
    fn fresh(&self, seq: u64) -> bool {
        if seq >= self.top {
            return true;
        }
        let back = self.top - 1 - seq;
        back < REPLAY_WINDOW && self.bits[(back / 64) as usize] & (1 << (back % 64)) == 0
    }

    fn mark(&mut self, seq: u64) {
        if seq >= self.top {
            let shift = seq + 1 - self.top;
            self.shift(shift);
            self.top = seq + 1;
        }
        let back = self.top - 1 - seq;
        self.bits[(back / 64) as usize] |= 1 << (back % 64);
    }

    /// Moves every bit `n` places further back.
    fn shift(&mut self, n: u64) {
        if n >= REPLAY_WINDOW {
            self.bits = Default::default();
            return;
        }
        let (words, bits) = ((n / 64) as usize, (n % 64) as u32);
        let len = self.bits.len();
        for i in (0..len).rev() {
            let src = i.checked_sub(words);
            let hi = src.map_or(0, |s| self.bits[s] << bits);
            let lo = match (src.and_then(|s| s.checked_sub(1)), bits) {
                (Some(s), b) if b > 0 => self.bits[s] >> (64 - b),
                _ => 0,
            };
            self.bits[i] = hi | lo;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{PacketHeader, PacketKind};

    fn datagram(seq: u64, body: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        PacketHeader {
            kind: PacketKind::Control,
            flags: Flags::empty(),
            stream: 0,
            session: 7,
            seq,
        }
        .encode(&mut d);
        d.extend_from_slice(body);
        d
    }

    const PSK: [u8; 32] = [0x42; 32];

    fn session() -> ((Sealer, Opener), (Sealer, Opener), StaticKey) {
        let host = StaticKey::generate();
        let client = StaticKey::generate();
        let init = Initiator::new(&client, &host.public(), &PSK, b"hello").unwrap();
        let resp = Responder::read(&host, init.message()).unwrap();
        assert_eq!(resp.remote(), client.public());
        assert_eq!(resp.payload(), b"hello");
        let (reply, host_keys) = resp.finish(&PSK).unwrap();
        let client_keys = init.finish(&reply).unwrap();
        assert_eq!(client_keys.remote, host.public());
        (client_keys.split(), host_keys.split(), client)
    }

    #[test]
    fn handshake_then_both_directions() {
        let ((c_seal, mut c_open), (h_seal, mut h_open), _) = session();
        let mut d = datagram(1, b"to host");
        c_seal.seal(&mut d).unwrap();
        assert_ne!(&d[HEADER_LEN..HEADER_LEN + 7], b"to host");
        let n = h_open.open(&mut d).unwrap();
        let (h, body) = PacketHeader::decode(&d[..n]).unwrap();
        assert!(h.flags.contains(Flags::ENCRYPTED));
        assert_eq!(body, b"to host");

        let mut d = datagram(1, b"to client");
        h_seal.seal(&mut d).unwrap();
        let n = c_open.open(&mut d).unwrap();
        assert_eq!(&d[HEADER_LEN..n], b"to client");
    }

    #[test]
    fn tampered_header_or_body_fails() {
        let ((c_seal, _), (_, mut h_open), _) = session();
        for at in [8, HEADER_LEN + 1] {
            let mut d = datagram(5, b"payload");
            c_seal.seal(&mut d).unwrap();
            d[at] ^= 1;
            assert!(
                matches!(h_open.open(&mut d), Err(SecureError::Auth)),
                "{at}"
            );
        }
    }

    #[test]
    fn unencrypted_and_foreign_datagrams_fail() {
        let ((_, _), (_, mut h_open), _) = session();
        let mut plain = datagram(1, b"0123456789abcdefgh");
        assert!(h_open.open(&mut plain).is_err());
        // Sealed under another session's keys.
        let ((other_seal, _), _, _) = session();
        let mut d = datagram(1, b"payload");
        other_seal.seal(&mut d).unwrap();
        assert!(matches!(h_open.open(&mut d), Err(SecureError::Auth)));
    }

    #[test]
    fn replays_are_dropped() {
        let ((c_seal, _), (_, mut h_open), _) = session();
        let sealed: Vec<Vec<u8>> = (1..=3000u64)
            .map(|s| {
                let mut d = datagram(s, b"x");
                c_seal.seal(&mut d).unwrap();
                d
            })
            .collect();
        // Out of order within the window is fine, each exactly once.
        for i in [2usize, 0, 1, 10, 5] {
            let mut d = sealed[i].clone();
            assert!(h_open.open(&mut d).is_ok(), "{i}");
            let mut d = sealed[i].clone();
            assert!(
                matches!(h_open.open(&mut d), Err(SecureError::Replay)),
                "{i}"
            );
        }
        let mut d = sealed[2999].clone();
        h_open.open(&mut d).unwrap();
        // 3 is now 2996 behind the newest: outside the window.
        let mut d = sealed[3].clone();
        assert!(matches!(h_open.open(&mut d), Err(SecureError::Replay)));
        // 2000 (seq 2001) is inside the window and unseen.
        let mut d = sealed[2000].clone();
        assert!(h_open.open(&mut d).is_ok());
    }

    #[test]
    fn replay_window_shifts_exactly() {
        let mut w = ReplayWindow::default();
        for s in [0u64, 1, 63, 64, 65, 200, 1000] {
            w.mark(s);
        }
        for s in [0u64, 1, 63, 64, 65, 200, 1000] {
            assert!(!w.fresh(s), "{s}");
        }
        for s in [2u64, 62, 66, 199, 999, 1001] {
            assert!(w.fresh(s), "{s}");
        }
        w.mark(1100);
        assert!(!w.fresh(200) && !w.fresh(1000) && w.fresh(201));
        assert!(!w.fresh(1100 - REPLAY_WINDOW)); // just out of the window
    }

    #[test]
    fn host_rejects_unknown_host_key() {
        // Client believes in another host key: the host cannot read message 1.
        let host = StaticKey::generate();
        let impostor = StaticKey::generate();
        let client = StaticKey::generate();
        let init = Initiator::new(&client, &impostor.public(), &PSK, b"hello").unwrap();
        assert!(Responder::read(&host, init.message()).is_err());
    }

    #[test]
    fn another_psk_gets_nowhere() {
        // A client without the host's token (or a server posing as a client).
        let host = StaticKey::generate();
        let client = StaticKey::generate();
        let init = Initiator::new(&client, &host.public(), &[0x24; 32], b"hello").unwrap();
        let resp = Responder::read(&host, init.message()).unwrap();
        let (reply, _) = resp.finish(&PSK).unwrap();
        assert!(init.finish(&reply).is_err());
    }

    /// Cost per 4K frame at the cap (~90 KiB in 1200-byte datagrams):
    /// `cargo test -p transport --release secure_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn secure_cost() {
        let ((c_seal, _), (_, mut h_open), _) = session();
        let frame: Vec<Vec<u8>> = (0..80u64).map(|s| datagram(s, &[0x5a; 1164])).collect();
        let rounds = 2000u64;
        let t = std::time::Instant::now();
        let mut sealed = Vec::new();
        for r in 0..rounds {
            sealed.clear();
            for (i, d) in frame.iter().enumerate() {
                let mut d = d.clone();
                d[SEQ_AT..SEQ_AT + 8].copy_from_slice(&(r * 80 + i as u64).to_le_bytes());
                c_seal.seal(&mut d).unwrap();
                sealed.push(d);
            }
            for d in &mut sealed {
                h_open.open(d).unwrap();
            }
        }
        let per_frame = t.elapsed() / rounds as u32;
        let bytes = 80.0 * 1200.0 * rounds as f64;
        println!(
            "seal + open of one 80-datagram frame: {per_frame:?} ({:.0} MB/s each way)",
            bytes / t.elapsed().as_secs_f64() / 1e6
        );
    }

    #[test]
    fn key_files() {
        let dir = std::env::temp_dir().join(format!("lumen-secure-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let key_path = dir.join("sub").join("device.key");
        let (k1, created) = StaticKey::load_or_create(&key_path).unwrap();
        assert!(created);
        let (k2, created) = StaticKey::load_or_create(&key_path).unwrap();
        assert!(!created);
        assert_eq!(k1.public(), k2.public());
        assert_eq!(
            k1.public().to_string().parse::<PublicKey>().unwrap(),
            k1.public()
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
