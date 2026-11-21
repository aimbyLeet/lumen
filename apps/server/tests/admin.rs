//! The web admin's API over real HTTPS on loopback: login, the admin's
//! actions and what they do to devices and sessions, static files.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{ADMIN_PASSWORD, Env, Q, add_host, connect, env_admin, env_admin_with, next, status};
use protocol::signal::{Access, FilePolicy, Limits, SignalMessage};
use rustls::pki_types::ServerName;
use serde_json::{Value, json};
use signal::Trust;

const WPC: [u8; 32] = [0x11; 32];

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The session cookie the server set.
    fn cookie(&self) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix("Set-Cookie: lumen_admin="))
            .map(|v| v.split(';').next().unwrap().to_string())
            .filter(|v| !v.is_empty())
    }
}

/// One HTTPS request to the admin.
fn http(e: &Env, method: &str, path: &str, body: Option<&Value>, cookie: Option<&str>) -> Reply {
    let Trust::Roots(roots) = &e.trust else {
        unreachable!()
    };
    let mut store = rustls::RootCertStore::empty();
    for c in roots {
        store.add(c.clone()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(store)
    .with_no_client_auth();
    let conn = rustls::ClientConnection::new(
        Arc::new(cfg),
        ServerName::try_from("localhost".to_string()).unwrap(),
    )
    .unwrap();
    let tcp = TcpStream::connect(e.server().admin_addr().unwrap()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = rustls::StreamOwned::new(conn, tcp);
    exchange(&mut s, &request(method, path, body, cookie, None))
}

/// One plain HTTP request to the admin behind a proxy, from the browser at `real_ip`.
fn proxied(e: &Env, method: &str, path: &str, body: Option<&Value>, real_ip: &str) -> Reply {
    let tcp = TcpStream::connect(e.server().admin_addr().unwrap()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    exchange(&mut &tcp, &request(method, path, body, None, Some(real_ip)))
}

fn request(
    method: &str,
    path: &str,
    body: Option<&Value>,
    cookie: Option<&str>,
    real_ip: Option<&str>,
) -> Vec<u8> {
    let body = body
        .map(|b| serde_json::to_vec(b).unwrap())
        .unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n",
        body.len()
    );
    if method == "POST" {
        req.push_str("Content-Type: application/json\r\n");
    }
    if let Some(c) = cookie {
        req.push_str(&format!("Cookie: other=1; lumen_admin={c}\r\n"));
    }
    if let Some(ip) = real_ip {
        req.push_str(&format!("X-Real-IP: {ip}\r\n"));
    }
    req.push_str("\r\n");
    let mut raw = req.into_bytes();
    raw.extend_from_slice(&body);
    raw
}

fn exchange(s: &mut (impl Read + Write), request: &[u8]) -> Reply {
    s.write_all(request).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        head,
        body: raw[split + 4..].to_vec(),
    }
}

fn login(e: &Env) -> String {
    let r = http(
        e,
        "POST",
        "/admin/api/login",
        Some(&json!({ "user": "admin", "password": ADMIN_PASSWORD })),
        None,
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.head.contains("HttpOnly; Secure; SameSite=Strict"));
    r.cookie().expect("cookie")
}

fn post(e: &Env, cookie: &str, path: &str, body: Value) -> Reply {
    http(e, "POST", path, Some(&body), Some(cookie))
}

fn state(e: &Env, cookie: &str) -> Value {
    let r = http(e, "GET", "/admin/api/state", None, Some(cookie));
    assert_eq!(r.status, 200);
    r.json()
}

#[test]
fn login_is_required_and_guarded() {
    let e = env_admin(None);
    assert_eq!(http(&e, "GET", "/admin/api/state", None, None).status, 401);
    assert_eq!(
        http(&e, "GET", "/admin/api/state", None, Some("forged")).status,
        401
    );
    let wrong = |pw: &str| {
        http(
            &e,
            "POST",
            "/admin/api/login",
            Some(&json!({ "user": "admin", "password": pw })),
            None,
        )
        .status
    };
    // A POST that is not JSON (what a cross-site form sends) is refused.
    let mut r = http(&e, "POST", "/admin/api/logout", None, None);
    assert_eq!(r.status, 401);
    let c = login(&e);
    r = http(&e, "POST", "/admin/api/approve", None, Some(&c));
    assert_eq!(r.status, 400, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(state(&e, &c)["user"], "admin");

    // Logging out ends that session only.
    let c2 = login(&e);
    assert_eq!(post(&e, &c, "/admin/api/logout", json!({})).status, 200);
    assert_eq!(
        http(&e, "GET", "/admin/api/state", None, Some(&c)).status,
        401
    );
    assert_eq!(
        http(&e, "GET", "/admin/api/state", None, Some(&c2)).status,
        200
    );

    // Wrong passwords: locked out, even for the right one.
    for _ in 0..lumen_server::admin::MAX_FAILURES {
        assert_eq!(wrong("guess"), 401);
    }
    assert_eq!(wrong("guess"), 429);
    assert_eq!(wrong(ADMIN_PASSWORD), 429);
}

#[test]
fn behind_a_proxy_locks_out_the_browser_not_the_proxy() {
    let e = env_admin_with(None, true);
    let login = |pw: &str, ip: &str| {
        proxied(
            &e,
            "POST",
            "/admin/api/login",
            Some(&json!({ "user": "admin", "password": pw })),
            ip,
        )
    };
    for _ in 0..lumen_server::admin::MAX_FAILURES {
        assert_eq!(login("guess", "203.0.113.7").status, 401);
    }
    assert_eq!(login(ADMIN_PASSWORD, "203.0.113.7").status, 429);
    // Every request comes from the proxy's 127.0.0.1: another browser still gets in.
    let r = login(ADMIN_PASSWORD, "198.51.100.2");
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.cookie().is_some());
}

/// What the admin's page gets: web/admin/src/api.ts has the same (signal::contract).
#[test]
fn the_page_has_the_same() {
    let e = env_admin(None);
    let c = login(&e);
    let mut host = e.host("WPC-10", "", WPC, 5);
    let mut anna = e.client("R3tr0", 0xA);
    add_host(&mut anna.c, WPC);
    let mut pending = state(&e, &c);
    let pair = json!({ "client": anna.id(), "host": host.id() });
    assert_eq!(post(&e, &c, "/admin/api/approve", pair).status, 200);
    connect(&mut anna.c, WPC, Q);
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "client": anna.id(), "limits": { "fps": 30, "mbps": null } }),
    );
    let mut session = state(&e, &c);
    assert_eq!(session["sessions"].as_array().unwrap().len(), 1);
    // What changes from run to run; a device's id does not, a session's does.
    const TIMES: [&str; 5] = ["now", "last_seen", "requested", "decided", "version"];
    signal::contract::steady(&mut pending, &TIMES);
    signal::contract::steady(&mut session, &TIMES);
    signal::contract::steady(&mut session["sessions"], &["seconds", "id"]);
    signal::contract::check_example(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/admin/src/contract/state.example.ts"
        ),
        "State",
        "../api",
        &[pending, session],
    );
}

#[test]
fn the_admin_runs_access_and_quality() {
    let e = env_admin(None);
    let c = login(&e);
    let mut host = e.host("WPC-10", "A.Ivanov", WPC, 5);
    let mut anna = e.client("R3tr0", 0xA);
    add_host(&mut anna.c, WPC);

    let s = state(&e, &c);
    assert_eq!(s["pending"], 1);
    assert_eq!(s["fps_choices"], json!([30, 60]));
    assert_eq!(s["hosts"][0]["machine"], "WPC-10");
    assert_eq!(s["hosts"][0]["online"], true);
    assert_eq!(s["clients"][0]["name"], "R3tr0");
    assert_eq!(s["clients"][0]["online"], true);
    let a = &s["access"][0];
    assert_eq!(
        (
            a["client_name"].as_str(),
            a["account"].as_str(),
            a["status"].as_str()
        ),
        (Some("R3tr0"), Some("A.Ivanov"), Some("pending"))
    );
    let pair = json!({ "client": anna.id(), "host": host.id() });

    assert_eq!(post(&e, &c, "/admin/api/approve", pair.clone()).status, 200);
    let SignalMessage::Session { id, .. } = connect(&mut anna.c, WPC, Q) else {
        panic!("no session");
    };
    assert!(matches!(next(&mut host.c), SignalMessage::Incoming { .. }));
    let s = state(&e, &c);
    assert_eq!(s["sessions"][0]["id"], format!("{id:016x}"));
    assert_eq!(s["sessions"][0]["client_name"], "R3tr0");

    // The admin's name for a client: in the admin panel only, beside its own.
    assert_eq!(state(&e, &c)["clients"][0]["admin_name"], Value::Null);
    let name = |n: Value| {
        post(
            &e,
            &c,
            "/admin/api/name",
            json!({ "client": anna.id(), "name": n }),
        )
    };
    assert_eq!(name(json!(" Anna (home) ")).status, 200);
    let s = state(&e, &c);
    assert_eq!(s["clients"][0]["name"], "R3tr0");
    assert_eq!(s["clients"][0]["admin_name"], "Anna (home)");
    assert_eq!(s["access"][0]["client_name"], "R3tr0");
    assert_eq!(s["sessions"][0]["client_name"], "R3tr0");
    assert_eq!(name(json!("x".repeat(200))).status, 400);
    assert_eq!(name(json!("Анна")).status, 400, "Latin only");
    let nobody = post(
        &e,
        &c,
        "/admin/api/name",
        json!({ "client": 999, "name": "x" }),
    );
    assert_eq!(nobody.status, 400);
    assert_eq!(state(&e, &c)["clients"][0]["admin_name"], "Anna (home)");
    assert_eq!(name(json!("")).status, 200);
    assert_eq!(state(&e, &c)["clients"][0]["admin_name"], Value::Null);
    assert_eq!(name(json!("Anna")).status, 200);
    assert_eq!(name(Value::Null).status, 200);
    assert_eq!(state(&e, &c)["clients"][0]["admin_name"], Value::Null);
    assert_eq!(name(json!("Anna (home)")).status, 200);

    // The frame rate and the cap, each on its own: everyone's, then hers.
    let r = post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "limits": { "fps": 30, "mbps": null } }),
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        status(&mut anna.c, &[WPC]).1,
        Limits {
            fps: Some(30),
            mbps: None
        }
    );
    post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "client": anna.id(), "limits": { "fps": null, "mbps": 40 } }),
    );
    assert_eq!(
        status(&mut anna.c, &[WPC]).1,
        Limits {
            fps: Some(30),
            mbps: Some(40.0)
        }
    );
    let s = state(&e, &c);
    assert_eq!(s["global_override"], json!({ "fps": 30, "mbps": null }));
    assert_eq!(
        s["clients"][0]["override"],
        json!({ "fps": null, "mbps": 40.0 })
    );
    let bad = post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "limits": { "fps": 45, "mbps": null } }),
    );
    assert_eq!(bad.status, 400);
    post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "client": anna.id(), "limits": { "fps": null, "mbps": null } }),
    );
    post(
        &e,
        &c,
        "/admin/api/override",
        json!({ "limits": { "fps": null, "mbps": null } }),
    );
    assert_eq!(status(&mut anna.c, &[WPC]).1, Limits::default());

    // Files: off for her (the running session hears it), the speed cap, back
    // to everyone's.
    let off = |allowed, mbps| SignalMessage::SessionFiles {
        id,
        files: FilePolicy { allowed, mbps },
    };
    let r = post(
        &e,
        &c,
        "/admin/api/files",
        json!({ "client": anna.id(), "allowed": false }),
    );
    assert_eq!(r.status, 200);
    for c in [&mut host.c, &mut anna.c] {
        assert_eq!(next(c), off(false, 100));
    }
    assert_eq!(
        post(&e, &c, "/admin/api/files-speed", json!({ "mbps": 30 })).status,
        200
    );
    for c in [&mut host.c, &mut anna.c] {
        assert_eq!(next(c), off(false, 30));
    }
    post(
        &e,
        &c,
        "/admin/api/files",
        json!({ "client": anna.id(), "allowed": null }),
    );
    for c in [&mut host.c, &mut anna.c] {
        assert_eq!(next(c), off(true, 30));
    }
    post(&e, &c, "/admin/api/files", json!({ "allowed": false }));
    for c in [&mut host.c, &mut anna.c] {
        assert_eq!(next(c), off(false, 30));
    }
    let s = state(&e, &c);
    assert_eq!(s["files"], json!({ "allowed": false, "mbps": 30 }));
    assert_eq!(s["clients"][0]["files"], Value::Null);
    assert_eq!(
        post(&e, &c, "/admin/api/files-speed", json!({ "mbps": 0 })).status,
        400
    );
    assert_eq!(post(&e, &c, "/admin/api/files", json!({})).status, 400);
    post(&e, &c, "/admin/api/files", json!({ "allowed": true }));
    for c in [&mut host.c, &mut anna.c] {
        assert_eq!(next(c), off(true, 30));
    }

    // Unlink: the running session ends within the recheck period.
    let t = Instant::now();
    assert_eq!(post(&e, &c, "/admin/api/unlink", pair.clone()).status, 200);
    assert_eq!(next(&mut host.c), SignalMessage::SessionEnd { id });
    println!("session ended {:?} after the unlink", t.elapsed());
    assert!(t.elapsed() < Duration::from_secs(1));
    assert_eq!(
        status(&mut anna.c, &[WPC]).0[0]
            .host
            .as_ref()
            .unwrap()
            .access,
        Access::None
    );
    assert_eq!(post(&e, &c, "/admin/api/unlink", pair.clone()).status, 400);

    // Ask again, reject: it stays rejected.
    add_host(&mut anna.c, WPC);
    assert_eq!(post(&e, &c, "/admin/api/reject", pair.clone()).status, 200);
    assert_eq!(state(&e, &c)["access"][0]["status"], "rejected");
    assert_eq!(
        add_host(&mut anna.c, WPC).host.unwrap().access,
        Access::Rejected
    );

    // Unlink from all; remove the host: it is cut off.
    post(&e, &c, "/admin/api/unlink", pair.clone());
    add_host(&mut anna.c, WPC);
    post(&e, &c, "/admin/api/approve", pair);
    let r = post(
        &e,
        &c,
        "/admin/api/unlink-all",
        json!({ "client": anna.id() }),
    );
    assert_eq!(r.json()["unlinked"], 1);
    assert_eq!(
        post(&e, &c, "/admin/api/remove", json!({ "device": host.id() })).status,
        200
    );
    let t = Instant::now();
    while host.c.recv(Duration::from_millis(100)).is_ok() {
        assert!(t.elapsed() < Duration::from_secs(3), "host not cut off");
    }
    let s = state(&e, &c);
    assert_eq!(s["hosts"], json!([]));
    assert_eq!(s["access"], json!([]));
    drop(anna);
}

#[test]
fn serves_the_app() {
    let dir = std::env::temp_dir().join(format!("lumen-admin-web-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>Lumen</title>",
    )
    .unwrap();
    std::fs::write(dir.join("assets").join("app.js"), "console.log(1)").unwrap();
    std::fs::write(dir.parent().unwrap().join("lumen-secret.txt"), "secret").unwrap();
    let e = env_admin(Some(dir.clone()));
    for p in ["/admin", "/admin/", "/admin/?x=1"] {
        let r = http(&e, "GET", p, None, None);
        assert_eq!(r.status, 200, "{p}");
        assert!(
            String::from_utf8_lossy(&r.body).contains("<title>Lumen"),
            "{p}"
        );
    }
    // The rest of the domain is not the admin's (nginx's landing page).
    for p in [
        "/",
        "/api/state",
        "/assets/app.js",
        "/administrator",
        "/adminx/api/state",
    ] {
        assert_eq!(http(&e, "GET", p, None, None).status, 404, "{p}");
    }
    let r = http(&e, "GET", "/admin/", None, None);
    assert_eq!(r.status, 200);
    assert!(r.head.contains("text/html"));
    assert!(r.head.contains("Content-Security-Policy"));
    assert!(String::from_utf8_lossy(&r.body).contains("Lumen"));
    let r = http(&e, "GET", "/admin/assets/app.js", None, None);
    assert!(r.head.contains("text/javascript"));
    assert_eq!(r.body, b"console.log(1)");
    // Client-side routes and anything odd get the app, never other files.
    for p in [
        "/admin/requests",
        "/admin/../lumen-secret.txt",
        "/admin/assets/../../lumen-secret.txt",
        "/admin/%2e%2e/x",
    ] {
        let r = http(&e, "GET", p, None, None);
        assert_eq!(r.status, 200, "{p}");
        assert!(
            String::from_utf8_lossy(&r.body).contains("<title>Lumen"),
            "{p}"
        );
    }
    assert_eq!(
        http(&e, "GET", "/admin/api/nothing", None, None).status,
        401
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server in a local network: the landing page at the root, the admin under
/// `/admin/`, plain HTTP redirected to HTTPS.
#[test]
fn lan_serves_the_landing_page_and_redirects() {
    let dir = std::env::temp_dir().join(format!("lumen-admin-lan-{}", std::process::id()));
    let (web, site) = (dir.join("web"), dir.join("site"));
    std::fs::create_dir_all(&web).unwrap();
    std::fs::create_dir_all(site.join("fonts")).unwrap();
    std::fs::write(web.join("index.html"), "<title>Lumen admin</title>").unwrap();
    std::fs::write(
        site.join("index.html"),
        "<title>Landing</title><script>1</script>",
    )
    .unwrap();
    let setup: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(site.join("lumen-setup-9.9.9.exe"), &setup).unwrap();
    std::fs::write(site.join("fonts").join("inter.woff2"), "font").unwrap();
    std::fs::write(dir.join("secret.txt"), "secret").unwrap();
    let e = common::env_admin_lan(Some(web), site);

    for p in ["/", "/?lang=ru", "/index.html"] {
        let r = http(&e, "GET", p, None, None);
        assert_eq!(r.status, 200, "{p}");
        assert!(
            String::from_utf8_lossy(&r.body).contains("<title>Landing"),
            "{p}"
        );
        assert!(r.head.contains("script-src 'self' 'unsafe-inline'"), "{p}");
    }
    let r = http(&e, "GET", "/lumen-setup-9.9.9.exe", None, None);
    assert_eq!(r.status, 200);
    assert!(r.body == setup, "the installer, byte for byte");
    let r = http(&e, "GET", "/fonts/inter.woff2", None, None);
    assert!(r.head.contains("font/woff2"));
    for p in [
        "/missing.html",
        "/fonts/",
        "/../secret.txt",
        "/fonts/../../secret.txt",
        "//secret.txt",
        "/C:/secret.txt",
    ] {
        assert_eq!(http(&e, "GET", p, None, None).status, 404, "{p}");
    }
    assert_eq!(http(&e, "POST", "/", Some(&json!({})), None).status, 405);
    // The admin is as before, with its stricter policy.
    let r = http(&e, "GET", "/admin/", None, None);
    assert!(String::from_utf8_lossy(&r.body).contains("<title>Lumen admin"));
    assert!(!r.head.contains("script-src"), "{}", r.head);
    login(&e);

    let admin_port = e.server().admin_addr().unwrap().port();
    let plain = |req: &str| {
        let tcp = TcpStream::connect(e.server().redirect_addr().unwrap()).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        exchange(&mut &tcp, req.as_bytes())
    };
    for (host, to) in [
        ("192.168.1.10", "192.168.1.10"),
        ("192.168.1.10:80", "192.168.1.10"),
        ("lumen.lan", "lumen.lan"),
        ("[fd00::5]:80", "[fd00::5]"),
    ] {
        let r = plain(&format!("GET /admin/?x=1 HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert_eq!(r.status, 301, "{host}");
        assert!(
            r.head
                .contains(&format!("Location: https://{to}:{admin_port}/admin/?x=1")),
            "{host}: {}",
            r.head
        );
    }
    for req in [
        "GET / HTTP/1.1\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a b\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a\"b\r\n\r\n",
    ] {
        assert_eq!(plain(req).status, 400, "{req:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
