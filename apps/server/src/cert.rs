//! The server's certificates: TLS settings from PEM files (re-read when
//! certbot renews them), a development CA, the server's own certificate for a
//! local network and the fingerprints devices trust it by.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

/// TLS settings from a PEM certificate chain and private key.
pub fn tls_config(cert_chain_pem: &[u8], key_pem: &[u8]) -> Result<Arc<ServerConfig>> {
    let certs = CertificateDer::pem_slice_iter(cert_chain_pem)
        .collect::<Result<Vec<_>, _>>()
        .context("certificate PEM")?;
    let key = PrivateKeyDer::from_pem_slice(key_pem).context("private key PEM")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Arc::new(cfg))
}

fn certified_key(
    provider: &rustls::crypto::CryptoProvider,
    cert_chain_pem: &[u8],
    key_pem: &[u8],
) -> Result<Arc<CertifiedKey>> {
    let certs = CertificateDer::pem_slice_iter(cert_chain_pem)
        .collect::<Result<Vec<_>, _>>()
        .context("certificate PEM")?;
    anyhow::ensure!(!certs.is_empty(), "no certificate in the PEM");
    let key = PrivateKeyDer::from_pem_slice(key_pem).context("private key PEM")?;
    // Fails when the key is not the certificate's (a renewal written halfway).
    let key = CertifiedKey::from_der(certs, key, provider).context("certificate and key")?;
    Ok(Arc::new(key))
}

/// How often the server looks whether its certificate files changed.
pub const CERT_CHECK_EVERY: Duration = Duration::from_secs(60);

/// TLS settings from PEM files, re-read when they change (certbot renews the
/// certificate in place; no restart needed): looked at no more often than
/// `check_every`, on a new connection. A broken file keeps the old certificate.
pub fn tls_config_from_files(
    cert: &Path,
    key: &Path,
    check_every: Duration,
) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let resolver = ReloadingCert::new(provider.clone(), cert, key, check_every)?;
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    Ok(Arc::new(cfg))
}

#[derive(Debug)]
struct ReloadingCert {
    provider: Arc<rustls::crypto::CryptoProvider>,
    cert: PathBuf,
    key: PathBuf,
    check_every: Duration,
    state: Mutex<CertState>,
}

#[derive(Debug)]
struct CertState {
    key: Arc<CertifiedKey>,
    /// Contents the current key was made from.
    files: (Vec<u8>, Vec<u8>),
    checked: Instant,
}

impl ReloadingCert {
    fn new(
        provider: Arc<rustls::crypto::CryptoProvider>,
        cert: &Path,
        key: &Path,
        check_every: Duration,
    ) -> Result<Self> {
        let files = read_pair(cert, key)?;
        let certified = certified_key(&provider, &files.0, &files.1)?;
        Ok(ReloadingCert {
            provider,
            cert: cert.into(),
            key: key.into(),
            check_every,
            state: Mutex::new(CertState {
                key: certified,
                files,
                checked: Instant::now(),
            }),
        })
    }

    fn current(&self) -> Arc<CertifiedKey> {
        let mut st = self.state.lock().unwrap();
        if st.checked.elapsed() >= self.check_every {
            st.checked = Instant::now();
            match read_pair(&self.cert, &self.key) {
                Ok(files) if files != st.files => {
                    match certified_key(&self.provider, &files.0, &files.1) {
                        Ok(k) => {
                            println!("certificate reloaded from {}", self.cert.display());
                            st.key = k;
                            st.files = files;
                        }
                        // Certbot may be halfway through writing: try again later.
                        Err(e) => eprintln!(
                            "certificate {}: {e:#}; keeping the old one",
                            self.cert.display()
                        ),
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("{e:#}; keeping the old certificate"),
            }
        }
        st.key.clone()
    }
}

impl rustls::server::ResolvesServerCert for ReloadingCert {
    fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

fn read_pair(cert: &Path, key: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let c = std::fs::read(cert).with_context(|| format!("read {}", cert.display()))?;
    let k = std::fs::read(key).with_context(|| format!("read {}", key.display()))?;
    Ok((c, k))
}

/// A development CA and a server certificate it signed.
pub struct DevCert {
    /// Give this to devices (`--server-ca`).
    pub ca_pem: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// Makes a development CA and a certificate for `names` (DNS names or IPs).
pub fn dev_cert(names: &[String]) -> Result<DevCert> {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::new(Vec::<String>::new())?;
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "Lumen development CA");
    let ca_cert = ca.self_signed(&ca_key)?;
    let issuer = Issuer::new(ca, ca_key);
    let key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(names.to_vec())?;
    leaf.distinguished_name
        .push(DnType::CommonName, "Lumen server (development)");
    let cert = leaf.signed_by(&key, &issuer)?;
    Ok(DevCert {
        ca_pem: ca_cert.pem(),
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// A server's own certificate (no domain: a local network), in its folder.
pub struct SelfCert {
    pub cert: PathBuf,
    pub key: PathBuf,
    /// Of the key ([`key_fingerprint`]): devices trust the server by it.
    pub fingerprint: String,
    /// The key was made now (devices that trusted the old one will refuse).
    pub new_key: bool,
}

/// Names (DNS or IP) the certificate in a [`self_cert`] folder is for.
const SELF_CERT_NAMES: &str = "names.txt";

/// The server's own key and a certificate it signed for `names`, kept in
/// `dir` (`key.pem`, `cert.pem`): the key is made once and never replaced,
/// so the fingerprint devices trusted survives updates and new names (the
/// certificate is issued again with the same key when the names change).
/// Valid for a century: nothing to renew.
pub fn self_cert(dir: &Path, names: &[String]) -> Result<SelfCert> {
    use rcgen::{CertificateParams, DnType, KeyPair, date_time_ymd};
    anyhow::ensure!(
        !names.is_empty(),
        "the certificate needs a name or an address"
    );
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    let new_key = !key.exists();
    let pair = if new_key {
        let pair = KeyPair::generate()?;
        write_replace(&key, pair.serialize_pem().as_bytes(), true)?;
        pair
    } else {
        let pem =
            std::fs::read_to_string(&key).with_context(|| format!("read {}", key.display()))?;
        KeyPair::from_pem(&pem).with_context(|| format!("key {}", key.display()))?
    };
    let names_file = dir.join(SELF_CERT_NAMES);
    let wanted = names.join("\n");
    let same_names = std::fs::read_to_string(&names_file).is_ok_and(|n| n == wanted);
    if new_key || !same_names || !cert.exists() {
        let mut params = CertificateParams::new(names.to_vec())?;
        params
            .distinguished_name
            .push(DnType::CommonName, "Lumen server");
        params.not_before = date_time_ymd(2020, 1, 1);
        params.not_after = date_time_ymd(2125, 1, 1);
        let c = params.self_signed(&pair)?;
        write_replace(&cert, c.pem().as_bytes(), false)?;
        write_replace(&names_file, wanted.as_bytes(), false)?;
    }
    Ok(SelfCert {
        cert,
        key,
        fingerprint: pair_fingerprint(&pair),
        new_key,
    })
}

/// The fingerprint of a PEM private key's public half (`lumen-server
/// fingerprint`): what devices show and keep for a server's own certificate.
pub fn key_fingerprint(key_pem: &str) -> Result<String> {
    let pair = rcgen::KeyPair::from_pem(key_pem).context("private key PEM")?;
    Ok(pair_fingerprint(&pair))
}

fn pair_fingerprint(pair: &rcgen::KeyPair) -> String {
    spki_fingerprint(&rcgen::PublicKeyData::subject_public_key_info(pair))
}

/// SHA-256 of a SubjectPublicKeyInfo (DER, with its outer SEQUENCE), as 16
/// groups of 4 upper-case hex digits. The same as `signal::pin::spki_fingerprint`.
pub fn spki_fingerprint(spki_der: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, spki_der);
    let hex: String = d.as_ref().iter().map(|b| format!("{b:02X}")).collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("hex"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Writes through a temporary file and a rename: never half a file.
/// `private`: readable by the owner only (a private key).
fn write_replace(path: &Path, data: &[u8], private: bool) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut f = o
        .open(&tmp)
        .with_context(|| format!("write {}", tmp.display()))?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
