//! The server over real TLS on loopback: registration, access requests and
//! the admin's decisions, sessions, overrides, drops.

mod common;

use std::time::{Duration, Instant};

use common::{Q, T, add_host, connect, env, env_with, next, quiet, status, sync, update};
use lumen_server::dev_cert;
use protocol::signal::{
    APP_VERSION, Access, ConnectFailure, FilePolicy, HostFailure, Limits, LoginFailure, Presence,
    Quality, Role, SIGNAL_VERSION, SignalMessage,
};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use signal::{Credentials, LoginError, SignalClient, Trust};

const WPC: [u8; 32] = [0x11; 32];

fn refused(r: Result<impl std::fmt::Debug, LoginError>) -> LoginFailure {
    match r {
        Err(LoginError::Rejected(f)) => f,
        other => panic!("{other:?}"),
    }
}

#[test]
fn registration_and_login() {
    let e = env();
    let mut c = e.connect();
    let (anna, caps) = c.register(Role::Client, "anna", [1; 32], T).unwrap();
    assert!(caps.is_empty());
    let mut h = e.connect();
    let (host, caps) = h.register(Role::Host, "", [2; 32], T).unwrap();
    assert_eq!(signal::cap_for(&caps, 3840, 2160, 60), Some(40.0));
    assert_ne!(anna.device, host.device);

    // Credentials survive a round trip through text (how apps keep them).
    let again: Credentials = anna.to_string().parse().unwrap();
    assert_eq!(e.connect().login(&again, T).unwrap().0, Role::Client);
    assert_eq!(e.connect().login(&host, T).unwrap().0, Role::Host);

    let wrong = Credentials {
        secret: [0; 32],
        ..anna
    };
    assert_eq!(
        refused(e.connect().login(&wrong, T)),
        LoginFailure::BadCredentials
    );
    assert_eq!(
        refused(e.connect().register(Role::Client, "  ", [1; 32], T)),
        LoginFailure::BadRequest
    );
    let mut c = e.connect();
    c.send(&SignalMessage::Login {
        version: SIGNAL_VERSION - 1,
        device: anna.device,
        secret: anna.secret,
        app_version: String::new(),
    })
    .unwrap();
    assert_eq!(
        next(&mut c),
        SignalMessage::LoginError {
            reason: LoginFailure::Version
        }
    );

    // The admin removes the client: its connection closes, it cannot log in.
    let mut live = e.login(&anna);
    assert!(e.admin().remove_device(anna.device).unwrap());
    let t = Instant::now();
    while live.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < T, "not closed");
    }
    assert_eq!(
        refused(e.connect().login(&anna, T)),
        LoginFailure::BadCredentials
    );
}

#[test]
fn only_the_same_version_of_lumen_connects() {
    let e = env();
    let mut host = e.host("WPC-10", "", WPC, 5);
    let anna = e.client("anna", 0xA);
    let mut anna_c = anna.c;
    add_host(&mut anna_c, WPC);
    e.admin().approve(anna.creds.device, host.id()).unwrap();
    quiet(&mut host.c);

    // The card says which version the host runs.
    let (cards, _) = status(&mut anna_c, &[WPC]);
    assert_eq!(cards[0].host.as_ref().unwrap().version, APP_VERSION);

    // The same client, logged in as another version of Lumen.
    let mut old = e.connect();
    old.send(&SignalMessage::Login {
        version: SIGNAL_VERSION,
        device: anna.creds.device,
        secret: anna.creds.secret,
        app_version: "0.0.1".into(),
    })
    .unwrap();
    assert!(matches!(next(&mut old), SignalMessage::LoginOk { .. }));
    assert_eq!(
        connect(&mut old, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Version,
            host_version: APP_VERSION.into(),
        }
    );
    assert_eq!(
        e.admin()
            .client(anna.creds.device)
            .unwrap()
            .unwrap()
            .version,
        "0.0.1"
    );
    quiet(&mut host.c);

    // Back on this version: the session opens.
    let mut now = e.login(&anna.creds);
    assert!(matches!(
        connect(&mut now, WPC, Q),
        SignalMessage::Session { .. }
    ));
}

#[test]
fn access_needs_the_admin() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let admin = e.admin();

    // A wrong token finds nothing.
    assert_eq!(add_host(&mut anna.c, [0x99; 32]).host, None);
    assert_eq!(
        connect(&mut anna.c, [0x99; 32], Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::UnknownHost,
            host_version: String::new(),
        }
    );

    // Looking does not ask; adding does.
    let (cards, forced) = status(&mut anna.c, &[WPC]);
    assert_eq!(cards[0].host.as_ref().unwrap().access, Access::None);
    assert_eq!(forced, Limits::default());
    assert!(admin.access_list(None).unwrap().is_empty());
    let card = add_host(&mut anna.c, WPC).host.unwrap();
    assert_eq!(
        (card.machine.as_str(), card.account.as_str()),
        ("WPC-10", "A.Ivanov")
    );
    assert_eq!(
        (card.access, card.presence),
        (Access::Pending, Presence::Online)
    );
    let pending = admin.access_list(Some(Access::Pending)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            pending[0].client,
            pending[0].host,
            pending[0].client_name.as_str()
        ),
        (anna.id(), host.id(), "anna")
    );

    // Knowing the token is not enough.
    assert_eq!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::NotApproved,
            host_version: String::new(),
        }
    );
    quiet(&mut host.c);

    admin.approve(anna.id(), host.id()).unwrap();
    let SignalMessage::Session {
        id,
        host_key,
        addrs,
        quality,
        files,
    } = connect(&mut anna.c, WPC, Q)
    else {
        panic!("no session");
    };
    assert_eq!(host_key, [5; 32]);
    assert_eq!(addrs, ["127.0.0.1:9000", "10.9.9.9:9001"]);
    assert_eq!(quality, Q);
    assert_eq!(files, FilePolicy::default());
    assert_eq!(
        next(&mut host.c),
        SignalMessage::Incoming {
            id,
            client_key: [0xA; 32],
            client_name: "anna".into(),
            quality: Q,
            files,
        }
    );

    // The admin turns files off for her: both sides hear it in the session.
    admin.set_client_files(anna.id(), Some(false)).unwrap();
    let off = SignalMessage::SessionFiles {
        id,
        files: FilePolicy {
            allowed: false,
            mbps: 100,
        },
    };
    assert_eq!(next(&mut host.c), off);
    assert_eq!(next(&mut anna.c), off);

    // The client ends it; the host hears.
    anna.c.send(&SignalMessage::SessionEnd { id }).unwrap();
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id });
}

#[test]
fn rejected_requests_stay_rejected() {
    let e = env();
    let host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let admin = e.admin();
    add_host(&mut anna.c, WPC);
    admin.reject(anna.id(), host.id()).unwrap();

    let card = add_host(&mut anna.c, WPC).host.unwrap();
    assert_eq!(card.access, Access::Rejected);
    assert!(admin.access_list(Some(Access::Pending)).unwrap().is_empty());
    // Deleting the card and adding it again does not bring a new request.
    anna.c.send(&SignalMessage::Forget { lookup: WPC }).unwrap();
    assert_eq!(
        add_host(&mut anna.c, WPC).host.unwrap().access,
        Access::Rejected
    );
    assert_eq!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::NotApproved,
            host_version: String::new(),
        }
    );

    // The admin lifts the rejection: the client may ask again.
    assert!(admin.unlink(anna.id(), host.id()).unwrap());
    assert_eq!(
        add_host(&mut anna.c, WPC).host.unwrap().access,
        Access::Pending
    );

    // A pending card deleted by the client takes the request away.
    anna.c.send(&SignalMessage::Forget { lookup: WPC }).unwrap();
    sync(&mut anna.c);
    assert!(admin.access_list(None).unwrap().is_empty());
}

#[test]
fn taking_access_back_ends_the_session() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let host2 = e.host("WPC-11", "B.Petrov", [0x22; 32], 6);
    let mut anna = e.client("anna", 0xA);
    let admin = e.admin();
    add_host(&mut anna.c, WPC);
    add_host(&mut anna.c, [0x22; 32]);
    admin.approve(anna.id(), host.id()).unwrap();
    admin.approve(anna.id(), host2.id()).unwrap();
    let SignalMessage::Session { id, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));

    // From another process, like the CLI.
    let t = Instant::now();
    assert!(admin.unlink(anna.id(), host.id()).unwrap());
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id });
    assert_eq!(next(&mut anna.c), SignalMessage::SessionEnd { id });
    let took = t.elapsed();
    println!("session ended {took:?} after the unlink");
    assert!(took < Duration::from_secs(1), "{took:?}");

    let (cards, _) = status(&mut anna.c, &[WPC, [0x22; 32]]);
    assert_eq!(cards[0].host.as_ref().unwrap().access, Access::None);
    assert_eq!(cards[1].host.as_ref().unwrap().access, Access::Approved);
    assert_eq!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::NotApproved,
            host_version: String::new(),
        }
    );

    // «Unlink from all machines».
    assert_eq!(admin.unlink_all(anna.id()).unwrap(), 1);
    let (cards, _) = status(&mut anna.c, &[[0x22; 32]]);
    assert_eq!(cards[0].host.as_ref().unwrap().access, Access::None);
}

#[test]
fn one_client_per_host_and_one_session_per_client() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let mut boris = e.client("boris", 0xB);
    let admin = e.admin();
    for c in [&mut anna, &mut boris] {
        add_host(&mut c.c, WPC);
        admin.approve(c.id(), host.id()).unwrap();
    }

    let SignalMessage::Session { id: first, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    assert_eq!(
        connect(&mut boris.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Busy,
            host_version: String::new(),
        }
    );
    let presence = |c: &mut SignalClient| status(c, &[WPC]).0[0].host.as_ref().unwrap().presence;
    assert_eq!(presence(&mut boris.c), Presence::Busy);
    assert_eq!(presence(&mut anna.c), Presence::Online);

    // Anna again, from another connection (her video window after a hang):
    // her old session gives way.
    let mut anna2 = e.login(&anna.creds);
    let SignalMessage::Session { id: second, .. } = connect(&mut anna2, WPC, Q) else {
        panic!("no session");
    };
    assert_ne!(first, second);
    assert_eq!(next(&mut anna.c), SignalMessage::SessionEnd { id: first });
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id: first });
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { id, .. } if id == second));

    // Her connection goes away: the session ends, Boris gets in.
    drop(anna2);
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id: second });
    assert!(matches!(
        connect(&mut boris.c, WPC, Q),
        SignalMessage::Session { .. }
    ));
}

#[test]
fn hosts_offline_inactive_or_replaced() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let admin = e.admin();
    add_host(&mut anna.c, WPC);
    admin.approve(anna.id(), host.id()).unwrap();

    // Another user is on screen.
    host.c
        .send(&update("WPC-10", "A.Ivanov", WPC, false))
        .unwrap();
    sync(&mut host.c);
    let card = status(&mut anna.c, &[WPC]).0.remove(0).host.unwrap();
    assert_eq!(card.presence, Presence::Inactive);
    assert_eq!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Inactive,
            host_version: String::new(),
        }
    );

    // Back on screen; a session; the host goes inactive: the session ends.
    host.c
        .send(&update("WPC-10", "A.Ivanov", WPC, true))
        .unwrap();
    sync(&mut host.c);
    let SignalMessage::Session { id, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    host.c
        .send(&update("WPC-10", "A.Ivanov", WPC, false))
        .unwrap();
    assert_eq!(next(&mut anna.c), SignalMessage::SessionEnd { id });
    host.c
        .send(&update("WPC-10", "A.Ivanov", WPC, true))
        .unwrap();
    sync(&mut host.c);

    // The host logs in again elsewhere: the older connection is closed.
    let mut again = e.login(&host.creds);
    again
        .send(&update("WPC-10", "A.Ivanov", WPC, true))
        .unwrap();
    sync(&mut again);
    let t = Instant::now();
    while host.c.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < T, "old connection not closed");
    }
    assert!(matches!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::Session { .. }
    ));
    assert!(matches!(next(&mut again), SignalMessage::Incoming { .. }));

    // Gone: offline.
    drop(again);
    let t = Instant::now();
    loop {
        let p = status(&mut anna.c, &[WPC]).0[0]
            .host
            .as_ref()
            .unwrap()
            .presence;
        if p == Presence::Offline {
            break;
        }
        assert!(t.elapsed() < T, "still {p:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Offline,
            host_version: String::new(),
        }
    );
}

#[test]
fn a_new_token_keeps_the_approval() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    add_host(&mut anna.c, WPC);
    e.admin().approve(anna.id(), host.id()).unwrap();

    let new = [0x33; 32];
    host.c
        .send(&update("WPC-10", "A.Ivanov", new, true))
        .unwrap();
    sync(&mut host.c);
    // The old token finds nothing; the new one finds the same host, approved.
    assert_eq!(status(&mut anna.c, &[WPC]).0[0].host, None);
    let card = add_host(&mut anna.c, new).host.unwrap();
    assert_eq!(card.access, Access::Approved);
    assert!(matches!(
        connect(&mut anna.c, new, Q),
        SignalMessage::Session { .. }
    ));

    // Another host cannot take a token that is in use: it is cut off.
    let mut thief = e.connect();
    thief.register(Role::Host, "", [9; 32], T).unwrap();
    thief.send(&update("EVIL", "X", new, true)).unwrap();
    let t = Instant::now();
    while thief.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < T, "not closed");
    }
    assert_eq!(
        add_host(&mut anna.c, new).host.unwrap().machine,
        "WPC-10".to_string()
    );
}

#[test]
fn the_admin_forces_quality() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let admin = e.admin();
    add_host(&mut anna.c, WPC);
    admin.approve(anna.id(), host.id()).unwrap();
    let asked = Quality {
        fps: 60,
        mbps: 80.0,
    };
    let session_quality = |anna: &mut SignalClient, host: &mut SignalClient| {
        let SignalMessage::Session { quality, .. } = connect(anna, WPC, asked) else {
            panic!("no session");
        };
        // The client's older session ends first (one per client).
        let q = loop {
            match next(host) {
                SignalMessage::Incoming { quality, .. } => break quality,
                SignalMessage::SessionEnd { .. } => continue,
                m => panic!("{m:?}"),
            }
        };
        assert_eq!(q, quality);
        quality
    };

    assert_eq!(session_quality(&mut anna.c, &mut host.c), asked);
    // Each on its own: everyone's frame rate alone, the client's cap stays hers.
    let global = Limits {
        fps: Some(30),
        mbps: None,
    };
    admin.set_global_limits(global).unwrap();
    assert_eq!(status(&mut anna.c, &[WPC]).1, global);
    assert_eq!(
        session_quality(&mut anna.c, &mut host.c),
        Quality {
            fps: 30,
            mbps: 80.0
        }
    );

    // Her own cap alone: everyone's frame rate still holds.
    let mine = Limits {
        fps: None,
        mbps: Some(20.0),
    };
    admin.set_client_limits(anna.id(), mine).unwrap();
    assert_eq!(status(&mut anna.c, &[WPC]).1, mine.over(global));
    assert_eq!(
        session_quality(&mut anna.c, &mut host.c),
        Quality {
            fps: 30,
            mbps: 20.0
        }
    );

    // Her own frame rate wins over everyone's; «auto» can be forced too.
    let mine = Limits {
        fps: Some(60),
        mbps: Some(0.0),
    };
    admin.set_client_limits(anna.id(), mine).unwrap();
    assert_eq!(
        session_quality(&mut anna.c, &mut host.c),
        Quality { fps: 60, mbps: 0.0 }
    );

    admin
        .set_client_limits(anna.id(), Limits::default())
        .unwrap();
    admin.set_global_limits(Limits::default()).unwrap();
    assert_eq!(status(&mut anna.c, &[WPC]).1, Limits::default());
    assert_eq!(session_quality(&mut anna.c, &mut host.c), asked);

    // Nonsense from a client is replaced by the default.
    let SignalMessage::Session { quality, .. } = connect(
        &mut anna.c,
        WPC,
        Quality {
            fps: 1000,
            mbps: -1.0,
        },
    ) else {
        panic!("no session");
    };
    assert_eq!(quality, Q);
}

#[test]
fn removing_a_host_cuts_it_off() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    add_host(&mut anna.c, WPC);
    let admin = e.admin();
    admin.approve(anna.id(), host.id()).unwrap();
    let SignalMessage::Session { id, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(admin.remove_device(host.id()).unwrap());
    assert_eq!(next(&mut anna.c), SignalMessage::SessionEnd { id });
    let t = Instant::now();
    while host.c.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < T, "not closed");
    }
    assert_eq!(status(&mut anna.c, &[WPC]).0[0].host, None);
    assert_eq!(
        refused(e.connect().login(&host.creds, T)),
        LoginFailure::BadCredentials
    );
}

/// The server closes this connection soon.
fn closes(c: &mut SignalClient) {
    let t = Instant::now();
    while c.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < T, "not closed");
    }
}

#[test]
fn ping_before_login() {
    let e = env();
    let mut c = e.connect();
    c.send(&SignalMessage::Ping { id: 7 }).unwrap();
    // `expect` skips pongs: `recv` takes them.
    assert_eq!(c.recv(T).unwrap(), Some(SignalMessage::Pong { id: 7 }));
    // The same connection may register after it.
    let (creds, _) = c.register(Role::Client, "anna", [1; 32], T).unwrap();
    assert!(e.admin().device_exists(creds.device).unwrap());
}

#[test]
fn a_client_removes_itself() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    add_host(&mut anna.c, WPC);
    e.admin().approve(anna.id(), host.id()).unwrap();
    // Its video window: another connection, with the session.
    let mut video = e.login(&anna.creds);
    let SignalMessage::Session { id, .. } = connect(&mut video, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));

    anna.c.send(&SignalMessage::Unregister).unwrap();
    assert_eq!(next(&mut anna.c), SignalMessage::Unregistered);
    closes(&mut anna.c);
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id });
    closes(&mut video);
    assert!(!e.admin().device_exists(anna.id()).unwrap());
    assert_eq!(
        refused(e.connect().login(&anna.creds, T)),
        LoginFailure::BadCredentials
    );
    // The host goes on.
    sync(&mut host.c);
}

#[test]
fn a_host_removes_itself() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    add_host(&mut anna.c, WPC);
    e.admin().approve(anna.id(), host.id()).unwrap();

    host.c.send(&SignalMessage::Unregister).unwrap();
    assert_eq!(next(&mut host.c), SignalMessage::Unregistered);
    closes(&mut host.c);
    assert_eq!(status(&mut anna.c, &[WPC]).0[0].host, None);
    assert_eq!(
        refused(e.connect().login(&host.creds, T)),
        LoginFailure::BadCredentials
    );
    // Registered again with the same token: a new host, asked for anew.
    let again = e.host("WPC-10", "", WPC, 5);
    assert_ne!(again.id(), host.id());
    let card = add_host(&mut anna.c, WPC);
    assert_eq!(card.host.unwrap().access, Access::Pending);
}

#[test]
fn a_taken_token_is_refused() {
    let e = env();
    let mut first = e.host("WPC-10", "", WPC, 5);
    let mut c = e.connect();
    c.register(Role::Host, "", [6; 32], T).unwrap();
    c.send(&update("WPC-10", "", WPC, true)).unwrap();
    assert_eq!(
        next(&mut c),
        SignalMessage::HostRefused {
            reason: HostFailure::TokenTaken
        }
    );
    closes(&mut c);
    let mut c = e.connect();
    c.register(Role::Host, "", [7; 32], T).unwrap();
    c.send(&update("", "", [0x33; 32], true)).unwrap();
    assert_eq!(
        next(&mut c),
        SignalMessage::HostRefused {
            reason: HostFailure::BadName
        }
    );
    sync(&mut first.c);
}

#[test]
fn untrusted_certificate_is_refused() {
    let e = env();
    let other = dev_cert(&["127.0.0.1".into()]).unwrap();
    let wrong = Trust::Roots(vec![
        CertificateDer::from_pem_slice(other.ca_pem.as_bytes()).unwrap(),
    ]);
    assert!(SignalClient::connect(&e.addr(), &wrong, T).is_err());
}

#[test]
fn silent_connections_are_dropped() {
    let e = env_with(Duration::from_secs(1), Duration::from_secs(2));

    // Never logs in: closed after the login timeout.
    let mut idle = e.connect();
    let t = Instant::now();
    while idle.recv(Duration::from_millis(200)).is_ok() {
        assert!(t.elapsed() < Duration::from_secs(5), "not closed");
    }

    // A client that goes silent (network gone, no FIN): the host is freed.
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    add_host(&mut anna.c, WPC);
    e.admin().approve(anna.id(), host.id()).unwrap();
    let SignalMessage::Session { id, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    // The host keeps pinging; Anna says nothing (her socket stays open).
    let t = Instant::now();
    let ended = loop {
        host.c.send(&SignalMessage::Ping { id: 1 }).unwrap();
        match host.c.recv(Duration::from_millis(500)).unwrap() {
            Some(SignalMessage::SessionEnd { id: ended }) => break ended,
            _ if t.elapsed() > Duration::from_secs(6) => panic!("session not ended"),
            _ => {}
        }
    };
    assert_eq!(ended, id);
    assert!(
        t.elapsed() >= Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
    drop(anna);
}
