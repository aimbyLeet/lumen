//! Signaling client: a blocking TLS connection to the Lumen server carrying
//! [`SignalMessage`]s (see `protocol::signal` for the messages and the flow).
//!
//! A device registers once ([`SignalClient::register`]) and keeps the
//! [`Credentials`] it gets; later it logs in with them ([`SignalClient::login`]).
//!
//! The server certificate is checked against the operating system's trust
//! store ([`Trust::System`]: a Let's Encrypt certificate on the server's
//! domain), against given roots ([`Trust::Roots`]: a development CA from
//! `lumen-server dev-cert`), or by the key's fingerprint ([`Trust::Pinned`]: a
//! server in a local network with its own certificate; see [`pin`]).

mod agent;
pub mod pin;
pub mod update;

pub mod contract;

pub use agent::{
    ClientLink, ClientSession, ConnectError, HostAgent, HostEvent, HostInfo, IncomingSession,
    PING_EVERY, ServerConfig, connect_to_host, probe, sign_in, unregister,
};
pub use pin::{CertProblem, cert_problem};

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use protocol::signal::{
    APP_VERSION, Access, CapRule, Card, CardHost, ConnectFailure, FPS_CHOICES, FilePolicy,
    HostFailure, LOOKUP_LEN, Limits, LoginFailure, MAX_NAME, MBPS_PRESETS, Presence, Quality, Role,
    SIGNAL_VERSION, SignalMessage, UpdateStep, cap_for, name_allowed,
};
use protocol::signal::{FrameBuffer, SECRET_LEN};

/// The server's signaling port when an address is given without one.
pub const DEFAULT_PORT: u16 = 7443;

/// The server's address as the user typed it (`lumen.example.com`, with or
/// without `https://` and a port): `host:port`. The error is a code: the interface has the words.
pub fn server_address(s: &str) -> Result<String, String> {
    let s = s
        .trim()
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if s.is_empty() || s.contains(char::is_whitespace) {
        return Err("err_server_address".into());
    }
    let has_port = s
        .rsplit_once(':')
        .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok());
    Ok(if has_port {
        s.to_string()
    } else {
        format!("{s}:{DEFAULT_PORT}")
    })
}
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// Which certificates the client accepts from the server.
#[derive(Clone)]
pub enum Trust {
    /// The operating system's trust store.
    System,
    /// Only these roots (e.g. a development CA).
    Roots(Vec<CertificateDer<'static>>),
    /// Only the key with this fingerprint ([`pin::spki_fingerprint`]), whatever
    /// the certificate's names and dates: a server's own certificate.
    Pinned(String),
}

impl Trust {
    /// Roots from a PEM file (`--server-ca`).
    pub fn from_pem_file(path: &Path) -> io::Result<Trust> {
        let certs = CertificateDer::pem_file_iter(path)
            .and_then(|it| it.collect::<Result<Vec<_>, _>>())
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?;
        if certs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: no certificates", path.display()),
            ));
        }
        Ok(Trust::Roots(certs))
    }

    /// How a program checks its server: a CA file (`--server-ca`,
    /// development), else the key the user trusted (`--server-pin`), else the
    /// system.
    pub fn choose(ca: Option<&Path>, pin: Option<&str>) -> io::Result<Trust> {
        Ok(match (ca, pin) {
            (Some(p), _) => Trust::from_pem_file(p)?,
            (None, Some(fp)) => Trust::Pinned(fp.to_string()),
            (None, None) => Trust::System,
        })
    }

    fn client_config(&self) -> io::Result<ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?;
        Ok(match self {
            Trust::System => {
                let inner =
                    rustls_platform_verifier::Verifier::new(builder.crypto_provider().clone())
                        .map_err(io::Error::other)?;
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(pin::SystemVerifier {
                        inner: Arc::new(inner),
                    }))
                    .with_no_client_auth()
            }
            Trust::Pinned(fingerprint) => {
                let fingerprint = pin::normalize_fingerprint(fingerprint).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "err_cert_fingerprint")
                })?;
                let provider = builder.crypto_provider().clone();
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(pin::PinnedVerifier {
                        fingerprint,
                        provider,
                    }))
                    .with_no_client_auth()
            }
            Trust::Roots(certs) => {
                let mut roots = RootCertStore::empty();
                for c in certs {
                    roots.add(c.clone()).map_err(io::Error::other)?;
                }
                builder.with_root_certificates(roots).with_no_client_auth()
            }
        })
    }
}

/// What a device logs in with: its id on the server and its secret.
/// Written as `<id>:<64 hex digits>`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Credentials {
    pub device: u64,
    pub secret: [u8; SECRET_LEN],
}

impl std::fmt::Display for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:", self.device)?;
        self.secret.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials(device {})", self.device)
    }
}

impl std::str::FromStr for Credentials {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || "credentials are <id>:<64 hex digits>".to_string();
        let (id, hex) = s.trim().split_once(':').ok_or_else(bad)?;
        let device = id.parse().map_err(|_| bad())?;
        if hex.len() != SECRET_LEN * 2 || !hex.is_ascii() {
            return Err(bad());
        }
        let mut secret = [0u8; SECRET_LEN];
        for (i, b) in secret.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
        }
        Ok(Credentials { device, secret })
    }
}

#[derive(Debug)]
pub enum LoginError {
    Rejected(LoginFailure),
    Io(io::Error),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Rejected(r) => write!(f, "login rejected: {r:?}"),
            LoginError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoginError {}

impl LoginError {
    /// The server's certificate the user can decide about, if that was it.
    pub fn cert_problem(&self) -> Option<CertProblem> {
        match self {
            LoginError::Io(e) => cert_problem(e),
            LoginError::Rejected(_) => None,
        }
    }
}

impl From<io::Error> for LoginError {
    fn from(e: io::Error) -> Self {
        LoginError::Io(e)
    }
}

/// One connection to the server.
pub struct SignalClient {
    stream: StreamOwned<ClientConnection, TcpStream>,
    frames: FrameBuffer,
}

/// Granularity of [`SignalClient::recv`] waits.
const POLL: Duration = Duration::from_millis(20);

impl SignalClient {
    /// Connects to `server` (`host:port`; `host` is also the TLS name) and
    /// completes the TLS handshake.
    pub fn connect(server: &str, trust: &Trust, timeout: Duration) -> io::Result<Self> {
        let host = server
            .rsplit_once(':')
            .map(|(h, _)| h.trim_start_matches('[').trim_end_matches(']'))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "server must be host:port")
            })?;
        let name = ServerName::try_from(host.to_string())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let addr = server
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "server address"))?;
        let tcp = TcpStream::connect_timeout(&addr, timeout)?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(timeout))?;
        tcp.set_write_timeout(Some(timeout))?;
        let conn = ClientConnection::new(Arc::new(trust.client_config()?), name)
            .map_err(io::Error::other)?;
        let mut stream = StreamOwned::new(conn, tcp);
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(pin::lift)?;
        }
        stream.sock.set_read_timeout(Some(POLL))?;
        Ok(SignalClient {
            stream,
            frames: FrameBuffer::default(),
        })
    }

    pub fn send(&mut self, msg: &SignalMessage) -> io::Result<()> {
        self.stream.write_all(&msg.to_frame())?;
        self.stream.flush()
    }

    /// The next message, waiting at most `timeout`. `Ok(None)`: nothing yet.
    pub fn recv(&mut self, timeout: Duration) -> io::Result<Option<SignalMessage>> {
        match self.recv_frame(timeout)? {
            Some(body) => SignalMessage::decode(&body)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            None => Ok(None),
        }
    }

    /// A frame as it is (the update door's), length prefix and all.
    pub fn send_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        self.stream.write_all(frame)?;
        self.stream.flush()
    }

    /// The next frame's body, not decoded, waiting at most `timeout`.
    pub fn recv_frame(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 16 * 1024];
        loop {
            if let Some(body) = self
                .frames
                .next_frame()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            {
                return Ok(Some(body));
            }
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.frames.extend(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Creates this device on the server (once); it is logged in after that.
    /// Returns its credentials (keep them) and the bitrate caps (hosts).
    pub fn register(
        &mut self,
        role: Role,
        name: &str,
        noise_key: [u8; 32],
        timeout: Duration,
    ) -> Result<(Credentials, Vec<CapRule>), LoginError> {
        self.send(&SignalMessage::Register {
            version: SIGNAL_VERSION,
            role,
            name: name.into(),
            noise_key,
            app_version: APP_VERSION.into(),
        })?;
        let creds = match self.expect(timeout)? {
            SignalMessage::Registered { device, secret } => Credentials { device, secret },
            SignalMessage::LoginError { reason } => return Err(LoginError::Rejected(reason)),
            other => return Err(unexpected(&other).into()),
        };
        Ok((creds, self.login_reply(timeout)?.1))
    }

    /// Logs in; returns the device's role and the bitrate caps (hosts), or
    /// why the server refused.
    pub fn login(
        &mut self,
        creds: &Credentials,
        timeout: Duration,
    ) -> Result<(Role, Vec<CapRule>), LoginError> {
        self.send(&SignalMessage::Login {
            version: SIGNAL_VERSION,
            device: creds.device,
            secret: creds.secret,
            app_version: APP_VERSION.into(),
        })?;
        self.login_reply(timeout)
    }

    fn login_reply(&mut self, timeout: Duration) -> Result<(Role, Vec<CapRule>), LoginError> {
        match self.expect(timeout)? {
            SignalMessage::LoginOk { role, caps } => Ok((role, caps)),
            SignalMessage::LoginError { reason } => Err(LoginError::Rejected(reason)),
            other => Err(unexpected(&other).into()),
        }
    }

    /// Waits for the next message that is not a `Pong`.
    pub fn expect(&mut self, timeout: Duration) -> io::Result<SignalMessage> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.recv(left)? {
                Some(SignalMessage::Pong { .. }) => continue,
                Some(m) => return Ok(m),
                None => return Err(io::ErrorKind::TimedOut.into()),
            }
        }
    }

    /// Closes the TLS session cleanly.
    pub fn close(mut self) {
        self.stream.conn.send_close_notify();
        let _ = self.stream.flush();
    }
}

fn unexpected(m: &SignalMessage) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected message {m:?}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses_as_typed() {
        let ok = |s: &str| server_address(s).unwrap();
        assert_eq!(ok("lumen.example.com"), "lumen.example.com:7443");
        assert_eq!(ok(" https://lumen.example.com/ "), "lumen.example.com:7443");
        assert_eq!(ok("lumen.example.com:8443"), "lumen.example.com:8443");
        assert_eq!(ok("10.0.0.2"), "10.0.0.2:7443");
        assert!(server_address("").is_err());
        assert!(server_address("a b").is_err());
    }
}
