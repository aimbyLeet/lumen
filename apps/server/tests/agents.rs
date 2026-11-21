//! The signaling agents the host and the client run, against a real server.

mod common;

use std::time::{Duration, Instant};

use common::{Q, env};
use protocol::signal::{Access, ConnectFailure, HostFailure, Limits, LoginFailure, Presence, Role};
use signal::{
    ClientLink, ConnectError, Credentials, HostAgent, HostEvent, HostInfo, LoginError,
    ServerConfig, connect_to_host,
};

const WPC: [u8; 32] = [0x11; 32];

fn cfg(e: &common::Env) -> ServerConfig {
    ServerConfig {
        server: e.addr(),
        trust: e.trust.clone(),
    }
}

fn info(active: bool) -> HostInfo {
    HostInfo {
        machine: "WPC-10".into(),
        account: "A.Ivanov".into(),
        lookup: WPC,
        addrs: vec!["127.0.0.1:9000".into()],
        active,
        auto_update: true,
    }
}

/// The next host event within `timeout`.
fn event(a: &HostAgent, timeout: Duration) -> HostEvent {
    a.events.recv_timeout(timeout).expect("host event")
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < timeout, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn register_ask_approve_connect() {
    let e = env();
    // The host registers itself and comes online.
    let host = HostAgent::start(cfg(&e), None, [5; 32], info(true));
    let HostEvent::Registered(host_creds) = event(&host, Duration::from_secs(5)) else {
        panic!("not registered");
    };
    let HostEvent::Online { caps } = event(&host, Duration::from_secs(5)) else {
        panic!("not online");
    };
    assert_eq!(signal::cap_for(&caps, 3840, 2160, 60), Some(40.0));

    // The client app registers and adds a card.
    let mut link = ClientLink::open(&cfg(&e), None, "R3tr0", [0xA; 32]).unwrap();
    let anna = link.credentials();
    wait_until("host announced", Duration::from_secs(5), || {
        link.status(&[WPC]).unwrap().0[0].host.is_some()
    });
    let (card, forced) = link.add_host(WPC).unwrap();
    let card = card.host.unwrap();
    assert_eq!(
        (card.access, card.presence, forced),
        (Access::Pending, Presence::Online, Limits::default())
    );
    assert!(matches!(
        connect_to_host(&cfg(&e), &anna, WPC, Q),
        Err(ConnectError::Refused(ConnectFailure::NotApproved))
    ));

    e.admin().approve(anna.device, host_creds.device).unwrap();
    let s = connect_to_host(&cfg(&e), &anna, WPC, Q).unwrap();
    assert_eq!(
        (s.host_key, s.addrs.clone(), s.quality),
        ([5; 32], vec!["127.0.0.1:9000".to_string()], Q)
    );
    let HostEvent::Incoming(inc) = event(&host, Duration::from_secs(5)) else {
        panic!("no incoming");
    };
    assert_eq!(
        (inc.id, inc.client_key, inc.client_name.as_str()),
        (s.id, [0xA; 32], "R3tr0")
    );
    // The app still sees the host (its own session does not make it busy).
    assert_eq!(
        link.status(&[WPC]).unwrap().0[0]
            .host
            .as_ref()
            .unwrap()
            .presence,
        Presence::Online
    );

    // The host ends it; the client notices.
    host.end_session(s.id);
    wait_until("client sees the end", Duration::from_secs(5), || s.ended());

    // The host goes inactive (another Windows user on screen).
    host.update(info(false));
    wait_until("host inactive", Duration::from_secs(5), || {
        link.status(&[WPC]).unwrap().0[0]
            .host
            .as_ref()
            .unwrap()
            .presence
            == Presence::Inactive
    });
    link.keepalive().unwrap();
    link.close();
}

#[test]
fn agents_survive_a_server_restart() {
    let mut e = env();
    let host = HostAgent::start(cfg(&e), None, [5; 32], info(true));
    let HostEvent::Registered(host_creds) = event(&host, Duration::from_secs(5)) else {
        panic!("not registered");
    };
    assert!(matches!(
        event(&host, Duration::from_secs(5)),
        HostEvent::Online { .. }
    ));
    let mut link = ClientLink::open(&cfg(&e), None, "anna", [0xA; 32]).unwrap();
    let anna = link.credentials();
    wait_until("host announced", Duration::from_secs(5), || {
        link.add_host(WPC).unwrap().0.host.is_some()
    });
    e.admin().approve(anna.device, host_creds.device).unwrap();
    let s = connect_to_host(&cfg(&e), &anna, WPC, Q).unwrap();
    assert!(matches!(
        event(&host, Duration::from_secs(5)),
        HostEvent::Incoming(_)
    ));

    // The server goes away: both sides notice.
    e.stop();
    assert!(matches!(
        event(&host, Duration::from_secs(5)),
        HostEvent::Offline { .. }
    ));
    wait_until("client sees the server go", Duration::from_secs(5), || {
        s.ended()
    });

    // It comes back: the host logs in again by itself (no new registration),
    // and announces itself again; a new session works.
    let t = Instant::now();
    e.restart();
    loop {
        match event(&host, Duration::from_secs(10)) {
            HostEvent::Online { .. } => break,
            HostEvent::Offline { .. } => continue,
            other => panic!("{other:?}"),
        }
    }
    println!("host back online {:?} after the restart", t.elapsed());
    let s2 = (0..50)
        .find_map(|_| match connect_to_host(&cfg(&e), &anna, WPC, Q) {
            Ok(s) => Some(s),
            Err(ConnectError::Refused(ConnectFailure::Offline)) => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
            Err(e) => panic!("{e}"),
        })
        .expect("host not announced again");
    assert!(
        matches!(event(&host, Duration::from_secs(5)), HostEvent::Incoming(inc) if inc.id == s2.id)
    );

    // The admin removes the host: its login is refused from now on.
    e.admin().remove_device(host_creds.device).unwrap();
    loop {
        match event(&host, Duration::from_secs(10)) {
            HostEvent::Refused(_) => break,
            HostEvent::Offline { .. } | HostEvent::SessionEnd { .. } => continue,
            other => panic!("{other:?}"),
        }
    }
}

/// No host event (an `Offline` after a refusal, say) for a while.
fn no_event(a: &HostAgent) {
    if let Ok(ev) = a.events.recv_timeout(Duration::from_millis(1500)) {
        panic!("unexpected {ev:?}");
    }
}

#[test]
fn a_refused_host_stays_refused() {
    let e = env();
    // Credentials the server does not know (its database was reset).
    let gone = Credentials {
        device: 999,
        secret: [1; 32],
    };
    let host = HostAgent::start(cfg(&e), Some(gone), [5; 32], info(true));
    assert!(matches!(
        event(&host, Duration::from_secs(5)),
        HostEvent::Refused(LoginFailure::BadCredentials)
    ));
    no_event(&host);

    // Its token is another host's there.
    let first = HostAgent::start(cfg(&e), None, [5; 32], info(true));
    assert!(matches!(
        event(&first, Duration::from_secs(5)),
        HostEvent::Registered(_)
    ));
    assert!(matches!(
        event(&first, Duration::from_secs(5)),
        HostEvent::Online { .. }
    ));
    let second = HostAgent::start(cfg(&e), None, [6; 32], info(true));
    assert!(matches!(
        event(&second, Duration::from_secs(5)),
        HostEvent::Registered(_)
    ));
    assert!(matches!(
        event(&second, Duration::from_secs(5)),
        HostEvent::Online { .. }
    ));
    assert!(matches!(
        event(&second, Duration::from_secs(5)),
        HostEvent::HostRefused(HostFailure::TokenTaken)
    ));
    no_event(&second);
    no_event(&first);
}

#[test]
fn probe_and_unregister() {
    let mut e = env();
    signal::probe(&cfg(&e)).unwrap();

    let host = HostAgent::start(cfg(&e), None, [5; 32], info(true));
    let HostEvent::Registered(creds) = event(&host, Duration::from_secs(5)) else {
        panic!("not registered");
    };
    drop(host);
    signal::unregister(&cfg(&e), Role::Host, &creds).unwrap();
    assert!(!e.admin().device_exists(creds.device).unwrap());
    assert!(matches!(
        signal::unregister(&cfg(&e), Role::Host, &creds),
        Err(LoginError::Rejected(LoginFailure::BadCredentials))
    ));

    let link = ClientLink::open(&cfg(&e), None, "R3tr0", [0xA; 32]).unwrap();
    let anna = link.credentials();
    link.unregister().unwrap();
    assert!(!e.admin().device_exists(anna.device).unwrap());
    assert!(matches!(
        ClientLink::open(&cfg(&e), Some(&anna), "R3tr0", [0xA; 32]),
        Err(LoginError::Rejected(LoginFailure::BadCredentials))
    ));

    e.stop();
    assert!(signal::probe(&cfg(&e)).is_err());
}
