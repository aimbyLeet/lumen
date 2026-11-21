//! Updates: the update door without a
//! login, the installer whole and checked, and a host asked to update when a
//! newer client wants it.

mod common;

use std::path::PathBuf;

use common::{Env, Q, T, add_host, connect, env, next, quiet, status, sync};
use lumen_server::update::UpdateFile;
use protocol::signal::{
    APP_VERSION, ConnectFailure, Presence, SIGNAL_VERSION, SignalMessage, UpdateStep,
};
use protocol::update::{CHUNK_LEN, DoorMessage};
use signal::SignalClient;
use signal::update::{UpdateError, download, query, sha256};

const WPC: [u8; 32] = [0x11; 32];
/// Newer than any real Lumen.
const NEWER: &str = "99.0.0";

/// A server whose door offers a test installer of `version` (`len` bytes).
fn env_offering(version: &str, len: usize) -> (Env, Vec<u8>) {
    let mut e = env();
    let dir = e.dir.join("update");
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<u8> = (0..len).map(|i| (i * 7 + i / 251) as u8).collect();
    std::fs::write(dir.join(format!("lumen-setup-{version}.exe")), &data).unwrap();
    e.update = UpdateFile::find(&dir).unwrap();
    e.restart();
    (e, data)
}

fn downloads(e: &Env) -> PathBuf {
    let d = e.dir.join("downloads");
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_door_offers_the_installer_without_a_login() {
    let (e, data) = env_offering(NEWER, CHUNK_LEN * 3 + 1234);
    let offer = query(&e.addr(), &e.trust, T).unwrap().unwrap();
    assert_eq!(offer.version, NEWER);
    assert_eq!(offer.size, data.len() as u64);
    assert!(offer.is_newer());
    let path = download(&e.addr(), &e.trust, &offer, &downloads(&e), T).unwrap();
    assert_eq!(path.file_name().unwrap(), "lumen-setup-99.0.0.exe");
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(sha256(&path).unwrap(), (offer.size, offer.sha256));
    // Downloaded already: nothing to fetch again.
    assert_eq!(
        download(&e.addr(), &e.trust, &offer, &downloads(&e), T).unwrap(),
        path
    );
}

#[test]
fn a_partial_download_resumes() {
    let (e, data) = env_offering(NEWER, CHUNK_LEN * 2 + 10);
    let offer = query(&e.addr(), &e.trust, T).unwrap().unwrap();
    let dir = downloads(&e);
    let part = dir.join("lumen-setup-99.0.0.exe.part");
    std::fs::write(&part, &data[..CHUNK_LEN + 3]).unwrap();
    let path = download(&e.addr(), &e.trust, &offer, &dir, T).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert!(!part.exists());
}

#[test]
fn damaged_bytes_are_refused_and_dropped() {
    let (e, data) = env_offering(NEWER, 5000);
    let offer = query(&e.addr(), &e.trust, T).unwrap().unwrap();
    let dir = downloads(&e);
    let part = dir.join("lumen-setup-99.0.0.exe.part");
    let mut bad = data[..4000].to_vec();
    bad[10] ^= 0xFF;
    std::fs::write(&part, &bad).unwrap();
    let r = download(&e.addr(), &e.trust, &offer, &dir, T);
    assert!(matches!(r, Err(UpdateError::Sha)), "{r:?}");
    assert!(!part.exists());
    assert!(!dir.join("lumen-setup-99.0.0.exe").exists());
    // The next try starts over and succeeds.
    let path = download(&e.addr(), &e.trust, &offer, &dir, T).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);
}

#[test]
fn no_installer_no_offer() {
    let e = env();
    assert_eq!(query(&e.addr(), &e.trust, T).unwrap(), None);
}

/// The door does not care about signaling: a raw query, as a device of any
/// signaling version would send it, gets the offer; a fetch from past the end
/// gets nothing and the connection closes.
#[test]
fn the_door_is_frozen_and_needs_no_signaling() {
    let (e, data) = env_offering(NEWER, 100);
    let mut c = e.connect();
    // A later door version with more fields: still answered in door 1.
    let mut q = DoorMessage::Query {
        door: 7,
        app_version: "0.1.0".into(),
    }
    .to_frame();
    q.extend_from_slice(b"later");
    let len = (q.len() - 4) as u32;
    q[..4].copy_from_slice(&len.to_le_bytes());
    raw_send(&mut c, &q);
    match raw_recv(&mut c) {
        DoorMessage::Offer { version, size, .. } => {
            assert_eq!((version.as_str(), size), (NEWER, data.len() as u64))
        }
        m => panic!("{m:?}"),
    }
    raw_send(&mut c, &DoorMessage::Fetch { offset: 1000 }.to_frame());
    assert!(c.recv(T).is_err(), "closed after the (empty) download");
}

fn raw_send(c: &mut SignalClient, frame: &[u8]) {
    c.send_frame(frame).unwrap();
}

fn raw_recv(c: &mut SignalClient) -> DoorMessage {
    let body = c.recv_frame(T).unwrap().unwrap();
    DoorMessage::decode(&body).unwrap()
}

/// A client registered as a newer Lumen, with access to the host.
fn newer_client(e: &Env, host: u64) -> SignalClient {
    let mut c = e.connect();
    c.send(&SignalMessage::Register {
        version: SIGNAL_VERSION,
        role: protocol::signal::Role::Client,
        name: "anna".into(),
        noise_key: [0xA; 32],
        app_version: NEWER.into(),
    })
    .unwrap();
    let device = match next(&mut c) {
        SignalMessage::Registered { device, .. } => device,
        m => panic!("{m:?}"),
    };
    assert!(matches!(next(&mut c), SignalMessage::LoginOk { .. }));
    add_host(&mut c, WPC);
    e.admin().approve(device, host).unwrap();
    c
}

fn presence(c: &mut SignalClient) -> (Presence, String) {
    let (cards, _) = status(c, &[WPC]);
    let h = cards[0].host.clone().unwrap();
    (h.presence, h.update_failed)
}

#[test]
fn a_newer_client_asks_the_host_to_update() {
    let e = env();
    let mut host = e.host("WPC-10", "", WPC, 5);
    let mut anna = newer_client(&e, host.id());
    quiet(&mut host.c);

    // Turned away as another version; the host is asked to update.
    assert_eq!(
        connect(&mut anna, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Version,
            host_version: APP_VERSION.into(),
        }
    );
    assert_eq!(
        next(&mut host.c),
        SignalMessage::UpdateWanted {
            version: NEWER.into()
        }
    );

    // It updates: «updating» for the client and the admin, also while gone.
    host.c
        .send(&SignalMessage::UpdateState {
            state: UpdateStep::Updating,
            detail: String::new(),
        })
        .unwrap();
    sync(&mut host.c);
    assert_eq!(presence(&mut anna), (Presence::Updating, String::new()));
    assert_eq!(e.server().hub().updating_hosts(), vec![host.id()]);
    // Asked again meanwhile: not asked twice.
    connect(&mut anna, WPC, Q);
    quiet(&mut host.c);
    drop(host.c);
    assert_eq!(presence(&mut anna).0, Presence::Updating);

    // Back on the old version: the update failed.
    let mut back = e.login(&host.creds);
    back.send(&common::update("WPC-10", "", WPC, true)).unwrap();
    back.send(&SignalMessage::UpdateState {
        state: UpdateStep::Failed,
        detail: "err_update_sha".into(),
    })
    .unwrap();
    sync(&mut back);
    assert_eq!(
        presence(&mut anna),
        (Presence::Online, "err_update_sha".to_string())
    );
    assert!(e.server().hub().updating_hosts().is_empty());
    assert_eq!(
        e.admin()
            .host(host.creds.device)
            .unwrap()
            .unwrap()
            .update_failed,
        "err_update_sha"
    );

    // Back on the newer version: the failure is over, the session opens.
    drop(back);
    let mut newer = e.connect();
    newer
        .send(&SignalMessage::Login {
            version: SIGNAL_VERSION,
            device: host.creds.device,
            secret: host.creds.secret,
            app_version: NEWER.into(),
        })
        .unwrap();
    assert!(matches!(next(&mut newer), SignalMessage::LoginOk { .. }));
    newer
        .send(&common::update("WPC-10", "", WPC, true))
        .unwrap();
    sync(&mut newer);
    assert_eq!(presence(&mut anna), (Presence::Online, String::new()));
    assert!(matches!(
        connect(&mut anna, WPC, Q),
        SignalMessage::Session { .. }
    ));
}

#[test]
fn an_older_client_or_a_busy_host_is_not_an_update() {
    let e = env();
    let mut host = e.host("WPC-10", "", WPC, 5);
    let boris = e.client("boris", 0xB);
    let mut boris_c = boris.c;
    add_host(&mut boris_c, WPC);
    e.admin().approve(boris.creds.device, host.id()).unwrap();
    quiet(&mut host.c);

    // Older than the host: no update.
    let mut old = e.connect();
    old.send(&SignalMessage::Login {
        version: SIGNAL_VERSION,
        device: boris.creds.device,
        secret: boris.creds.secret,
        app_version: "0.0.1".into(),
    })
    .unwrap();
    assert!(matches!(next(&mut old), SignalMessage::LoginOk { .. }));
    connect(&mut old, WPC, Q);
    quiet(&mut host.c);
    drop(old);

    // The same version in a session with the host...
    let mut boris_now = e.login(&boris.creds);
    assert!(matches!(
        connect(&mut boris_now, WPC, Q),
        SignalMessage::Session { .. }
    ));
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));

    // ...so a newer client gets no update: the host is busy.
    let mut anna = newer_client(&e, host.id());
    assert!(matches!(
        connect(&mut anna, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Version,
            ..
        }
    ));
    quiet(&mut host.c);
}

/// The host's switch (signaling v10): it says it with every `HostUpdate`;
/// the card shows it; off, a newer client is told so at once (no waiting)
/// and the host is not asked; on again, it is.
#[test]
fn the_host_says_whether_it_updates_itself() {
    let e = env();
    let mut host = e.host("WPC-10", "", WPC, 5);
    let mut anna = newer_client(&e, host.id());
    quiet(&mut host.c);
    let auto = |c: &mut SignalClient| status(c, &[WPC]).0[0].host.clone().unwrap().auto_update;
    assert!(auto(&mut anna), "on until the host says");

    let mut off = common::update("WPC-10", "", WPC, true);
    if let SignalMessage::HostUpdate { auto_update, .. } = &mut off {
        *auto_update = false;
    }
    host.c.send(&off).unwrap();
    sync(&mut host.c);
    assert!(!auto(&mut anna));
    assert_eq!(
        connect(&mut anna, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::UpdateOff,
            host_version: APP_VERSION.into(),
        }
    );
    quiet(&mut host.c);

    host.c
        .send(&common::update("WPC-10", "", WPC, true))
        .unwrap();
    sync(&mut host.c);
    assert!(auto(&mut anna));
    assert_eq!(
        connect(&mut anna, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Version,
            host_version: APP_VERSION.into(),
        }
    );
    assert_eq!(
        next(&mut host.c),
        SignalMessage::UpdateWanted {
            version: NEWER.into()
        }
    );
}
