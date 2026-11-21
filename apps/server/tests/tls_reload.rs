//! The server picks up a renewed certificate without a restart.

use std::time::Duration;

use lumen_server::{Config, Server, dev_cert, tls_config_from_files};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use signal::{SignalClient, Trust};

const T: Duration = Duration::from_secs(3);

fn trust(ca_pem: &str) -> Trust {
    Trust::Roots(vec![
        CertificateDer::from_pem_slice(ca_pem.as_bytes()).unwrap(),
    ])
}

#[test]
fn renewed_certificate_is_used_without_restart() {
    let dir = std::env::temp_dir().join(format!("lumen-tls-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    let names = ["localhost".to_string(), "127.0.0.1".to_string()];
    let old = dev_cert(&names).unwrap();
    std::fs::write(&cert, &old.cert_pem).unwrap();
    std::fs::write(&key, &old.key_pem).unwrap();

    let server = Server::start(Config::new(
        "127.0.0.1:0".parse().unwrap(),
        dir.join("lumen.db"),
        tls_config_from_files(&cert, &key, Duration::ZERO).unwrap(),
    ))
    .unwrap();
    let addr = format!("127.0.0.1:{}", server.addr().port());
    let (old_ca, new) = (trust(&old.ca_pem), dev_cert(&names).unwrap());
    let new_ca = trust(&new.ca_pem);
    SignalClient::connect(&addr, &old_ca, T).unwrap();
    assert!(SignalClient::connect(&addr, &new_ca, T).is_err());

    // A half-written renewal (certificate only, old key) keeps the old pair.
    std::fs::write(&cert, &new.cert_pem).unwrap();
    SignalClient::connect(&addr, &old_ca, T).unwrap();

    std::fs::write(&key, &new.key_pem).unwrap();
    SignalClient::connect(&addr, &new_ca, T).unwrap();
    assert!(SignalClient::connect(&addr, &old_ca, T).is_err());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
