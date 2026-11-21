//! Trusting a server by its key (a server in a local network: its own
//! certificate, no domain, no Let's Encrypt; `lumen-server self-cert`).
//!
//! The first time, the system's check fails on such a certificate: if it is
//! self-signed and the address is a local one (an IP, a name without dots,
//! `.lan`, `.local`…), the error carries the key's fingerprint
//! ([`CertProblem::Untrusted`]) so the program can show it and ask the user.
//! Once trusted, the device keeps the fingerprint and connects with
//! [`Trust::Pinned`](crate::Trust::Pinned): only that key is accepted, whatever
//! the address or the certificate's dates; another key is
//! [`CertProblem::Changed`]. A public domain's certificate never gets the
//! question: a broken one stays an error.

use std::io;
use std::sync::Arc;

use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, Error, OtherError, SignatureScheme};

/// Why the server's certificate was not accepted, when the user can decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertProblem {
    /// A self-signed certificate on a local address, not trusted yet.
    Untrusted { fingerprint: String },
    /// Not the key this device trusted (the server was reinstalled, or it is
    /// not the server).
    Changed { fingerprint: String },
}

impl CertProblem {
    pub fn fingerprint(&self) -> &str {
        match self {
            CertProblem::Untrusted { fingerprint } | CertProblem::Changed { fingerprint } => {
                fingerprint
            }
        }
    }
}

/// A code with the fingerprint for the interface (`code|fingerprint`).
impl std::fmt::Display for CertProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertProblem::Untrusted { fingerprint } => write!(f, "err_cert_untrusted|{fingerprint}"),
            CertProblem::Changed { fingerprint } => write!(f, "err_cert_changed|{fingerprint}"),
        }
    }
}

impl std::error::Error for CertProblem {}

/// The [`CertProblem`] behind a connection error, if that is what it was.
pub fn cert_problem(e: &io::Error) -> Option<CertProblem> {
    e.get_ref()?.downcast_ref::<CertProblem>().cloned()
}

/// A handshake error carrying a [`CertProblem`] from the verifier becomes an
/// error with the problem itself (see [`cert_problem`]).
pub(crate) fn lift(e: io::Error) -> io::Error {
    let problem = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Error>())
        .and_then(|tls| match tls {
            Error::InvalidCertificate(CertificateError::Other(OtherError(o))) => {
                o.downcast_ref::<CertProblem>().cloned()
            }
            _ => None,
        });
    match problem {
        Some(p) => io::Error::new(io::ErrorKind::InvalidData, p),
        None => e,
    }
}

fn reject(p: CertProblem) -> Error {
    Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(p))))
}

/// SHA-256 of a SubjectPublicKeyInfo (DER, with its outer SEQUENCE), as 16
/// groups of 4 upper-case hex digits (as `lumen-server self-cert` prints it).
pub fn spki_fingerprint(spki_der: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, spki_der);
    let hex: String = d.as_ref().iter().map(|b| format!("{b:02X}")).collect();
    group(&hex)
}

fn group(hex: &str) -> String {
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("hex"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The fingerprint of a certificate's key; `None` if it does not parse.
pub fn cert_fingerprint(cert: &CertificateDer<'_>) -> Option<String> {
    let c = webpki::EndEntityCert::try_from(cert).ok()?;
    Some(spki_fingerprint(c.subject_public_key_info().as_ref()))
}

/// A fingerprint as the user may have copied it (any case, spaces or colons)
/// in the canonical form; `None` if it is not 64 hex digits.
pub fn normalize_fingerprint(s: &str) -> Option<String> {
    let hex: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect::<String>()
        .to_ascii_uppercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| group(&hex))
}

/// Whether a server name is a local one (where a self-signed certificate may
/// be offered to trust): an IP address, a name without dots, or a name in a
/// local-only zone.
pub fn is_local_name(host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
        || [".lan", ".local", ".internal", ".home.arpa", ".localdomain"]
            .iter()
            .any(|z| host.ends_with(z))
}

fn self_signed(cert: &CertificateDer<'_>) -> bool {
    webpki::EndEntityCert::try_from(cert).is_ok_and(|c| c.issuer() == c.subject())
}

/// Signatures of the handshake: checked as usual, by the certificate's key.
fn tls12(
    p: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    verify_tls12_signature(message, cert, dss, &p.signature_verification_algorithms)
}

fn tls13(
    p: &CryptoProvider,
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    verify_tls13_signature(message, cert, dss, &p.signature_verification_algorithms)
}

/// Accepts exactly one key.
#[derive(Debug)]
pub(crate) struct PinnedVerifier {
    pub fingerprint: String,
    pub provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let fingerprint = cert_fingerprint(end_entity)
            .ok_or(Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if fingerprint == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(reject(CertProblem::Changed { fingerprint }))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        tls12(&self.provider, message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        tls13(&self.provider, message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The system's check; a self-signed certificate on a local address it
/// refuses becomes [`CertProblem::Untrusted`] with the key's fingerprint.
#[derive(Debug)]
pub(crate) struct SystemVerifier {
    pub inner: Arc<dyn ServerCertVerifier>,
}

impl ServerCertVerifier for SystemVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let e = match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let local = match server_name {
            ServerName::IpAddress(_) => true,
            ServerName::DnsName(n) => is_local_name(n.as_ref()),
            _ => false,
        };
        match cert_fingerprint(end_entity) {
            Some(fingerprint) if local && self_signed(end_entity) => {
                Err(reject(CertProblem::Untrusted { fingerprint }))
            }
            _ => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_names() {
        for n in [
            "192.168.1.10",
            "10.0.0.5",
            "[fd00::5]",
            "fd00::5",
            "lumen",
            "server.lan",
            "Lumen.Local",
            "a.internal",
            "box.home.arpa",
            "pc.localdomain",
        ] {
            assert!(is_local_name(n), "{n}");
        }
        for n in [
            "lumen.example.com",
            "example.org",
            "lan.example.com",
            "evil.local.com",
        ] {
            assert!(!is_local_name(n), "{n}");
        }
    }

    #[test]
    fn fingerprints_as_typed() {
        let fp = "9500 EF55 438E 47F8 F472 91B2 CE45 DCD6 998A 8F06 EDBC 34BF 914B 13E5 99DF E36A";
        assert_eq!(normalize_fingerprint(fp).as_deref(), Some(fp));
        let colons = fp.replace(' ', "").to_lowercase();
        let colons: String = colons
            .as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(normalize_fingerprint(&colons).as_deref(), Some(fp));
        assert_eq!(normalize_fingerprint(&fp[..fp.len() - 1]), None);
        assert_eq!(normalize_fingerprint(&fp.replace('E', "G")), None);
        assert_eq!(normalize_fingerprint(""), None);
    }

    #[test]
    fn problems_are_codes_and_survive_the_handshake_error() {
        let p = CertProblem::Changed {
            fingerprint: "AAAA".into(),
        };
        assert_eq!(p.to_string(), "err_cert_changed|AAAA");
        let wrapped = io::Error::new(io::ErrorKind::InvalidData, reject(p.clone()));
        assert_eq!(cert_problem(&lift(wrapped)), Some(p));
        let other = io::Error::new(
            io::ErrorKind::InvalidData,
            Error::InvalidCertificate(CertificateError::UnknownIssuer),
        );
        assert_eq!(cert_problem(&lift(other)), None);
    }
}
