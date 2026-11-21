//! With --status-json: one `@status {...}` line on stdout per change of the
//! session's state, for the Lumen Client app (apps/client-app) that runs
//! this process and shows it on the host's card.
//!
//! Besides the state it says which addresses the session uses, as /24
//! subnets: what another VPN (one with a TUN adapter takes every route) must
//! leave alone on this machine (the host's) and on the host (this machine's).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use desktop::net::{route_to, subnet24};
use serde_json::{Value, json};

use crate::net::{Connection, Unreachable};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn emit(v: Value) {
    if ENABLED.load(Ordering::Relaxed) {
        println!("@status {v}");
    }
}

/// Asking the server for a session.
pub fn connecting() {
    emit(json!({ "event": "connecting" }));
}

/// The host has no monitor and asks for ours:
/// question `ask`, answered with its number.
pub fn monitor_wanted(ask: u32) {
    emit(crate::monitor_pick::question(
        ask,
        crate::monitor_pick::this_monitor(),
    ));
}

/// The host has no monitor, and none can be made (no Lumen VDD).
pub fn no_monitors() {
    emit(json!({ "event": "no_monitors" }));
}

/// The stream is up.
pub fn streaming(conn: &Connection) {
    emit(streaming_value(conn.peer, &windows_route));
}

/// The way to an address: the address we send from, the adapter.
type RouteTo<'a> = &'a dyn Fn(Ipv4Addr) -> Option<(Ipv4Addr, String)>;

fn windows_route(ip: Ipv4Addr) -> Option<(Ipv4Addr, String)> {
    route_to(ip).map(|r| (r.ip, r.adapter))
}

fn streaming_value(peer: SocketAddr, route: RouteTo) -> Value {
    let mut v = json!({ "event": "streaming", "host": peer.to_string() });
    if let IpAddr::V4(ip) = peer.ip() {
        v["host_net"] = subnet24(ip).into();
        if let Some((from, adapter)) = route(ip) {
            v["via"] = from.to_string().into();
            v["adapter"] = adapter.into();
            v["client_net"] = subnet24(from).into();
        }
    }
    v
}

/// The session was lost; a new one is being opened.
pub fn reconnecting(why: &str) {
    emit(json!({ "event": "reconnecting", "reason": why }));
}

/// An attempt failed and will be repeated: the host did not answer (with
/// the routes to its addresses), or anything else (host offline, server
/// unreachable).
pub fn retrying(e: &anyhow::Error) {
    // An older host updating to this version (the hub has the words).
    if matches!(
        e.downcast_ref::<signal::ConnectError>(),
        Some(signal::ConnectError::Version(_))
    ) {
        emit(json!({ "event": "waiting", "reason": "host_updating" }));
        return;
    }
    let Some(u) = e.downcast_ref::<Unreachable>() else {
        emit(json!({ "event": "waiting", "reason": format!("{e:#}") }));
        return;
    };
    emit(unreachable(&u.hosts, &windows_route));
}

/// The host's addresses are private (WireGuard / Tailscale): one that goes the
/// way of the internet has no route of its own here — the tunnel is down or
/// another VPN took it, and the address we would send from is not one the
/// host could reach us at.
fn unreachable(hosts: &[SocketAddr], route: RouteTo) -> Value {
    let internet = route(Ipv4Addr::new(1, 1, 1, 1)).map(|(from, _)| from);
    let mut client_nets: Vec<String> = Vec::new();
    let mut host_nets: Vec<String> = Vec::new();
    let routes: Vec<Value> = hosts
        .iter()
        .filter_map(|h| match h.ip() {
            IpAddr::V4(ip) => Some((h, ip)),
            IpAddr::V6(_) => None,
        })
        .map(|(h, ip)| {
            push_new(&mut host_nets, subnet24(ip));
            let mut v = json!({ "host": h.to_string() });
            if let Some((from, adapter)) = route(ip) {
                let default = Some(from) == internet;
                if !default {
                    push_new(&mut client_nets, subnet24(from));
                }
                v["default"] = default.into();
                v["via"] = from.to_string().into();
                v["adapter"] = adapter.into();
            }
            v
        })
        .collect();
    json!({
        "event": "unreachable",
        "routes": routes,
        "host_nets": host_nets,
        "client_nets": client_nets,
    })
}

fn push_new(list: &mut Vec<String>, s: String) {
    if !list.contains(&s) {
        list.push(s);
    }
}

#[cfg(test)]
mod contract {
    use signal::contract::check_example;

    use super::*;

    fn route(ip: Ipv4Addr) -> Option<(Ipv4Addr, String)> {
        match ip.octets()[0] {
            1 => Some((Ipv4Addr::new(192, 168, 0, 5), "Wi-Fi".into())),
            10 => Some((Ipv4Addr::new(10, 0, 0, 2), "WireGuard".into())),
            192 => Some((Ipv4Addr::new(192, 168, 0, 5), "Wi-Fi".into())),
            _ => None,
        }
    }

    /// What the hub gets: web/client/src/api.ts has the same.
    #[test]
    fn the_interface_has_the_same() {
        let at = |s: &str| s.parse::<SocketAddr>().unwrap();
        check_example(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../web/client/src/contract/video-status.example.ts"
            ),
            "VideoStatus",
            "../api",
            &[
                json!({ "event": "connecting" }),
                json!({ "event": "waiting", "reason": "host offline" }),
                json!({ "event": "reconnecting", "reason": "lost" }),
                streaming_value(at("10.0.0.1:9000"), &route),
                streaming_value(at("172.16.0.1:9000"), &route),
                unreachable(
                    &[
                        at("10.0.0.1:9000"),
                        at("192.168.100.41:9000"),
                        at("172.16.0.1:9000"),
                    ],
                    &route,
                ),
                crate::monitor_pick::question(1, (2880, 1800, 150)),
                json!({ "event": "no_monitors" }),
            ],
        );
    }
}
