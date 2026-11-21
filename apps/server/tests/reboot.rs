//! A client restarts a host's machine (signaling v11): approved access and the
//! host online are enough — a session on it does not matter.

mod common;

use common::{Q, add_host, connect, env, next, quiet, reply, status};
use protocol::signal::{ConnectFailure, Presence, SignalMessage};
use signal::SignalClient;

const WPC: [u8; 32] = [0x11; 32];

fn reboot(c: &mut SignalClient, lookup: [u8; 32]) -> Option<ConnectFailure> {
    c.send(&SignalMessage::Reboot { lookup }).unwrap();
    match reply(c) {
        SignalMessage::RebootAnswer { refused } => refused,
        m => panic!("{m:?}"),
    }
}

#[test]
fn an_approved_client_restarts_the_host_even_when_busy() {
    let e = env();
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("anna", 0xA);
    let mut boris = e.client("boris", 0xB);
    for c in [&mut anna, &mut boris] {
        add_host(&mut c.c, WPC);
        e.admin().approve(c.id(), host.id()).unwrap();
    }
    // Anna is connected; Boris restarts the machine anyway.
    assert!(matches!(
        connect(&mut anna.c, WPC, Q),
        SignalMessage::Session { .. }
    ));
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    assert_eq!(reboot(&mut boris.c, WPC), None);
    assert_eq!(
        next(&mut host.c),
        SignalMessage::RebootWanted {
            client_name: "boris".into()
        }
    );
    // «Restarting» at once, nobody connects; back when it logs in again.
    let presence = |c: &mut SignalClient| status(c, &[WPC]).0[0].host.as_ref().unwrap().presence;
    assert_eq!(presence(&mut boris.c), Presence::Restarting);
    assert_eq!(
        connect(&mut boris.c, WPC, Q),
        SignalMessage::ConnectError {
            reason: ConnectFailure::Offline,
            host_version: String::new(),
        }
    );
    drop(host.c);
    let mut back = e.login(&host.creds);
    back.send(&common::update("WPC-10", "A.Ivanov", WPC, true))
        .unwrap();
    common::sync(&mut back);
    assert_eq!(presence(&mut boris.c), Presence::Online);
}

#[test]
fn no_access_unknown_or_offline_host_is_refused() {
    let e = env();
    let mut host = e.host("WPC-10", "", WPC, 5);
    let mut anna = e.client("anna", 0xA);

    assert_eq!(
        reboot(&mut anna.c, [0x22; 32]),
        Some(ConnectFailure::UnknownHost)
    );
    // Asked, not approved yet.
    add_host(&mut anna.c, WPC);
    assert_eq!(reboot(&mut anna.c, WPC), Some(ConnectFailure::NotApproved));
    quiet(&mut host.c);

    e.admin().approve(anna.id(), host.id()).unwrap();
    drop(host.c);
    // The server notices the host is gone.
    let deadline = std::time::Instant::now() + common::T;
    loop {
        match reboot(&mut anna.c, WPC) {
            Some(ConnectFailure::Offline) => break,
            None if std::time::Instant::now() < deadline => continue,
            r => panic!("{r:?}"),
        }
    }
}
