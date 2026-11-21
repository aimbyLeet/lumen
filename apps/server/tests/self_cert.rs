//! The server's own certificate (a local network): the key is made once and
//! kept, so its fingerprint survives restarts, updates and new addresses.

use std::path::Path;
use std::time::Duration;

use lumen_server::{
    CERT_CHECK_EVERY, Config, Server, dev_cert, key_fingerprint, self_cert, tls_config,
    tls_config_from_files,
};
use signal::{CertProblem, Role, SignalClient, Trust, cert_problem};

const T: Duration = Duration::from_secs(3);

fn serve(dir: &Path, tls: std::sync::Arc<rustls::ServerConfig>) -> Server {
    Server::start(Config::new(
        "127.0.0.1:0".parse().unwrap(),
        dir.join("lumen.db"),
        tls,
    ))
    .unwrap()
}

/// What a device gets connecting with `trust`: a connection, or the problem
/// with the certificate the user could decide about (`None`: another error).
fn try_connect(addr: &str, trust: &Trust) -> Result<SignalClient, Option<CertProblem>> {
    SignalClient::connect(addr, trust, T).map_err(|e| cert_problem(&e))
}

/// Devices meet a server's own certificate: the system refuses it, the
/// fingerprint they are shown is the one the server printed, and once pinned
/// it works; another key is refused as a changed one.
#[test]
fn devices_trust_the_key_they_were_shown() {
    let dir = std::env::temp_dir().join(format!("lumen-self-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let c = self_cert(&dir.join("tls"), &["127.0.0.1".into(), "localhost".into()]).unwrap();
    let server = serve(
        &dir,
        tls_config_from_files(&c.cert, &c.key, CERT_CHECK_EVERY).unwrap(),
    );
    let port = server.addr().port();
    // By IP (a name is local by `signal::pin::is_local_name`, tested there).
    let by_ip = format!("127.0.0.1:{port}");

    // The first time: not trusted, with the server's fingerprint.
    match try_connect(&by_ip, &Trust::System) {
        Err(Some(CertProblem::Untrusted { fingerprint })) => assert_eq!(fingerprint, c.fingerprint),
        other => panic!("{:?}", other.err()),
    }
    // Trusted: connects and registers, the fingerprint as the user copied it too.
    let mut dev = try_connect(&by_ip, &Trust::Pinned(c.fingerprint.clone())).unwrap();
    dev.register(Role::Client, "anna", [7; 32], T).unwrap();
    let typed = c.fingerprint.replace(' ', ":").to_lowercase();
    assert!(try_connect(&by_ip, &Trust::Pinned(typed)).is_ok());
    let e = SignalClient::connect(&by_ip, &Trust::Pinned("junk".into()), T)
        .err()
        .unwrap();
    assert_eq!(e.to_string(), "err_cert_fingerprint");

    // Another key on the same address: changed, with the new fingerprint.
    drop(dev);
    drop(server);
    let other = self_cert(&dir.join("other"), &["127.0.0.1".into()]).unwrap();
    let server = serve(
        &dir,
        tls_config_from_files(&other.cert, &other.key, CERT_CHECK_EVERY).unwrap(),
    );
    let addr = format!("127.0.0.1:{}", server.addr().port());
    match try_connect(&addr, &Trust::Pinned(c.fingerprint.clone())) {
        Err(Some(CertProblem::Changed { fingerprint })) => {
            assert_eq!(fingerprint, other.fingerprint)
        }
        r => panic!("{:?}", r.err()),
    }
    drop(server);

    // A certificate that is not self-signed is never offered.
    let dev_ca = dev_cert(&["127.0.0.1".into()]).unwrap();
    let server = serve(
        &dir,
        tls_config(dev_ca.cert_pem.as_bytes(), dev_ca.key_pem.as_bytes()).unwrap(),
    );
    let addr = format!("127.0.0.1:{}", server.addr().port());
    assert_eq!(try_connect(&addr, &Trust::System).err(), Some(None));
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_key_and_its_fingerprint_stay() {
    let dir = std::env::temp_dir().join(format!("lumen-self-cert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let names = vec!["192.168.1.10".to_string()];

    let first = self_cert(&dir, &names).unwrap();
    assert!(first.new_key);
    let groups: Vec<&str> = first.fingerprint.split(' ').collect();
    assert_eq!(groups.len(), 16, "{}", first.fingerprint);
    assert!(groups.iter().all(|g| {
        g.len() == 4
            && g.chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
    }));
    let key_pem = std::fs::read_to_string(&first.key).unwrap();
    assert_eq!(key_fingerprint(&key_pem).unwrap(), first.fingerprint);
    // Serves TLS as it is.
    tls_config_from_files(&first.cert, &first.key, CERT_CHECK_EVERY).unwrap();
    let cert = std::fs::read(&first.cert).unwrap();

    // Again (a restart, an update): nothing changes.
    let again = self_cert(&dir, &names).unwrap();
    assert!(!again.new_key);
    assert_eq!(again.fingerprint, first.fingerprint);
    assert_eq!(std::fs::read(&again.cert).unwrap(), cert);
    assert_eq!(std::fs::read_to_string(&again.key).unwrap(), key_pem);

    // A new address: a new certificate with the same key.
    let moved = vec!["10.0.0.5".to_string(), "lumen.lan".to_string()];
    let m = self_cert(&dir, &moved).unwrap();
    assert!(!m.new_key);
    assert_eq!(m.fingerprint, first.fingerprint);
    assert_ne!(std::fs::read(&m.cert).unwrap(), cert);
    tls_config_from_files(&m.cert, &m.key, CERT_CHECK_EVERY).unwrap();

    // The folder gone (a reinstall): another key, another fingerprint.
    std::fs::remove_dir_all(&dir).unwrap();
    let other = self_cert(&dir, &names).unwrap();
    assert!(other.new_key);
    assert_ne!(other.fingerprint, first.fingerprint);

    assert!(self_cert(&dir, &[]).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
