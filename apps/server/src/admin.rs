//! The web admin: HTTPS on its own port (the same certificate as signaling),
//! a JSON API and the static files of the React app.
//! Behind a reverse proxy on the same machine (nginx of the VPS terminates
//! TLS) it speaks plain HTTP to loopback connections only. Without one (a
//! server in a local network) it also serves the landing page at the root and
//! sends plain HTTP on another port to itself.
//!
//! One administrator, login and password from the configuration. A login
//! gets a random session id in a cookie (`HttpOnly`, `Secure`,
//! `SameSite=Strict`, 12 h); changes are `POST` with a JSON body, which a
//! cross-site form cannot send. After [`MAX_FAILURES`] wrong passwords an
//! address is locked out for [`LOCKOUT`].
//!
//! HTTP/1.1 without keep-alive: one request per connection, a thread per
//! connection, like signaling. Changes go to the database; the server's
//! recheck thread ends sessions whose access is gone within `RECHECK_EVERY`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use blake2::{Blake2s256, Digest};
use protocol::signal::{Access, FPS_CHOICES, Limits, MBPS_PRESETS};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Hub;
use crate::db::{Db, access_name, now, random_bytes};

pub const MAX_FAILURES: u32 = 5;
pub const LOCKOUT: Duration = Duration::from_secs(60);
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const COOKIE: &str = "lumen_admin";
const MAX_HEAD: usize = 16 << 10;
const MAX_BODY: usize = 64 << 10;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone)]
pub struct AdminConfig {
    /// TCP address of the HTTPS admin.
    pub listen: SocketAddr,
    pub user: String,
    pub password: String,
    /// The built React app (`index.html` and its assets); `None`: API only.
    pub web: Option<PathBuf>,
    /// Plain HTTP for a reverse proxy on this machine, which terminates TLS:
    /// only loopback connections are served, and the browser's address (for
    /// the wrong-password lockout) is the proxy's `X-Real-IP`.
    pub behind_proxy: bool,
    /// The landing page at the root (`index.html`, the installer), when no
    /// nginx serves it (a server in a local network); `None`: 404 there.
    pub site: Option<PathBuf>,
    /// Plain HTTP here sends browsers to the admin's HTTPS (port 80 of a
    /// server in a local network, so the address alone opens the page).
    pub redirect: Option<SocketAddr>,
}

/// What [`start`] runs.
pub(crate) struct Started {
    pub addr: SocketAddr,
    pub redirect: Option<SocketAddr>,
    pub threads: Vec<JoinHandle<()>>,
}

struct Admin {
    cfg: AdminConfig,
    password_hash: [u8; 32],
    hub: Arc<Hub>,
    /// Session id → when it stops being valid.
    sessions: Mutex<HashMap<String, Instant>>,
    /// Address → wrong passwords in a row, and when the last one was.
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

fn hash(s: &str) -> [u8; 32] {
    Blake2s256::digest(s.as_bytes()).into()
}

/// Compares in constant time (the hashes, so the length does not leak either).
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Starts the admin's accept thread (and the redirect's, if any).
pub(crate) fn start(
    cfg: AdminConfig,
    hub: Arc<Hub>,
    tls: Arc<ServerConfig>,
    stop: Arc<AtomicBool>,
) -> Result<Started> {
    if cfg.user.is_empty() || cfg.password.len() < 8 {
        bail!("the admin needs a login and a password of at least 8 characters");
    }
    if cfg.behind_proxy && (cfg.site.is_some() || cfg.redirect.is_some()) {
        bail!("behind a proxy the proxy serves the landing page and redirects");
    }
    let listener = listen(cfg.listen, "admin")?;
    let addr = listener.local_addr()?;
    let redirect_listener = cfg.redirect.map(|at| listen(at, "redirect")).transpose()?;
    let redirect_addr = redirect_listener
        .as_ref()
        .map(|l| l.local_addr())
        .transpose()?;
    let admin = Arc::new(Admin {
        password_hash: hash(&cfg.password),
        cfg,
        hub,
        sessions: Mutex::new(HashMap::new()),
        failures: Mutex::new(HashMap::new()),
    });
    let mut threads = vec![spawn_accept(
        "admin",
        listener,
        stop.clone(),
        move |tcp, peer| {
            let _ = serve(tcp, peer, &admin, tls.clone());
        },
    )?];
    if let Some(l) = redirect_listener {
        let port = addr.port();
        threads.push(spawn_accept("redirect", l, stop, move |tcp, _| {
            let _ = redirect(tcp, port);
        })?);
    }
    Ok(Started {
        addr,
        redirect: redirect_addr,
        threads,
    })
}

fn listen(at: SocketAddr, what: &str) -> Result<TcpListener> {
    let l = TcpListener::bind(at).with_context(|| format!("{what} on {at}"))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

/// A thread that accepts connections until `stop`, each served by `handle`
/// in a thread of its own (errors: browsers probe and drop connections, not
/// worth a line).
fn spawn_accept(
    what: &'static str,
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    handle: impl Fn(TcpStream, SocketAddr) + Send + Sync + 'static,
) -> io::Result<JoinHandle<()>> {
    let handle = Arc::new(handle);
    thread::Builder::new().name(what.into()).spawn(move || {
        let mut workers: Vec<JoinHandle<()>> = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((tcp, peer)) => {
                    let handle = handle.clone();
                    let spawned = thread::Builder::new()
                        .name(format!("{what} {peer}"))
                        .spawn(move || handle(tcp, peer));
                    if let Ok(t) = spawned {
                        workers.push(t);
                    }
                    workers.retain(|t| !t.is_finished());
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(e) => {
                    eprintln!("{what} accept: {e}");
                    thread::sleep(POLL);
                }
            }
        }
        for t in workers {
            let _ = t.join();
        }
    })
}

/// Plain HTTP: every request is sent to the same path over HTTPS on `port`.
fn redirect(tcp: TcpStream, port: u16) -> Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
    let Some(req) = read_request(&mut &tcp)? else {
        return Ok(());
    };
    let response = match req.host.as_deref().and_then(host_name) {
        Some(host) => {
            let at = if port == 443 {
                host
            } else {
                format!("{host}:{port}")
            };
            let mut r = Response::error(301, "moved to https");
            r.headers
                .push(format!("Location: https://{at}{}", req.path));
            r
        }
        None => Response::error(400, "no Host"),
    };
    write_response(&mut &tcp, &response)?;
    (&tcp).flush()?;
    Ok(())
}

/// The name in a `Host` header, without its port; `None` if it is not a
/// plain name or address (it goes into a header).
fn host_name(host: &str) -> Option<String> {
    let host = host.trim();
    let name = if host.starts_with('[') {
        &host[..=host.find(']')?]
    } else {
        host.split(':').next()?
    };
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-[]:".contains(c));
    ok.then(|| name.to_string())
}

struct Request {
    method: String,
    path: String,
    /// `X-Real-IP`: the browser's address as the reverse proxy saw it.
    real_ip: Option<IpAddr>,
    host: Option<String>,
    cookie: Option<String>,
    json: bool,
    body: Vec<u8>,
}

/// The admin's pages: scripts and styles from files only.
const ADMIN_CSP: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'";
/// The landing page keeps its script in the page.
const SITE_CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; \
                        script-src 'self' 'unsafe-inline'; frame-ancestors 'none'";

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    headers: Vec<String>,
    csp: &'static str,
}

impl Response {
    fn json(status: u16, v: &Value) -> Response {
        Response {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(v).expect("json"),
            headers: Vec::new(),
            csp: ADMIN_CSP,
        }
    }

    fn error(status: u16, message: &str) -> Response {
        Response::json(status, &json!({ "error": message }))
    }

    fn ok() -> Response {
        Response::json(200, &json!({ "ok": true }))
    }
}

fn serve(tcp: TcpStream, peer: SocketAddr, admin: &Admin, tls: Arc<ServerConfig>) -> Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
    if admin.cfg.behind_proxy {
        // Anyone else could forge `X-Real-IP` (and skip the proxy's TLS).
        if !peer.ip().is_loopback() {
            return Ok(());
        }
        return exchange(&mut &tcp, peer, admin);
    }
    let mut stream = StreamOwned::new(ServerConnection::new(tls)?, tcp);
    exchange(&mut stream, peer, admin)?;
    stream.conn.send_close_notify();
    stream.flush()?;
    Ok(())
}

/// One request and its response.
fn exchange(stream: &mut (impl Read + Write), peer: SocketAddr, admin: &Admin) -> Result<()> {
    let response = match read_request(stream) {
        Ok(Some(req)) => {
            let ip = match req.real_ip {
                Some(ip) if admin.cfg.behind_proxy => ip,
                _ => peer.ip(),
            };
            handle(admin, ip, req)
        }
        Ok(None) => return Ok(()),
        Err(e) => Response::error(400, &format!("{e:#}")),
    };
    write_response(stream, &response)?;
    stream.flush()?;
    Ok(())
}

fn read_request(stream: &mut impl Read) -> Result<Option<Request>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            bail!("request head too large");
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    };
    let mut headers = [httparse::EMPTY_HEADER; 48];
    let mut req = httparse::Request::new(&mut headers);
    req.parse(&buf[..head_end]).context("bad request")?;
    let method = req.method.context("no method")?.to_string();
    let path = req.path.context("no path")?.to_string();
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .and_then(|h| std::str::from_utf8(h.value).ok())
    };
    let length: usize = header("content-length")
        .map(|v| v.trim().parse())
        .transpose()
        .context("bad Content-Length")?
        .unwrap_or(0);
    if length > MAX_BODY {
        bail!("request body too large");
    }
    let cookie = header("cookie").and_then(|c| {
        c.split(';')
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == COOKIE)
            .map(|(_, v)| v.to_string())
    });
    let real_ip = header("x-real-ip").and_then(|v| v.trim().parse().ok());
    let host = header("host").map(str::to_string);
    let json = header("content-type").is_some_and(|t| t.starts_with("application/json"));
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("body cut short");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Some(Request {
        method,
        path,
        real_ip,
        host,
        cookie,
        json,
        body,
    }))
}

/// Where the admin lives: nginx passes the path as it is, the domain's root is
/// the landing page.
const PREFIX: &str = "/admin";

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    }
}

fn write_response(stream: &mut impl Write, r: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\
         Referrer-Policy: no-referrer\r\nStrict-Transport-Security: max-age=31536000\r\n\
         Content-Security-Policy: {}\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len(),
        r.csp
    );
    for h in &r.headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)
}

fn handle(admin: &Admin, ip: IpAddr, req: Request) -> Response {
    let full = req.path.split('?').next().unwrap_or("");
    let Some(path) = full
        .strip_prefix(PREFIX)
        .filter(|p| p.is_empty() || p.starts_with('/'))
        .map(str::to_string)
    else {
        return match (&admin.cfg.site, req.method.as_str()) {
            (Some(site), "GET" | "HEAD") => site_file(site, full),
            (Some(_), _) => Response::error(405, "method not allowed"),
            (None, _) => Response::error(404, "not found"),
        };
    };
    if !path.starts_with("/api/") {
        return match req.method.as_str() {
            "GET" | "HEAD" => static_file(admin.cfg.web.as_deref(), &path),
            _ => Response::error(405, "method not allowed"),
        };
    }
    if req.method == "POST" && !req.json {
        return Response::error(400, "send JSON (Content-Type: application/json)");
    }
    if (req.method.as_str(), path.as_str()) == ("POST", "/api/login") {
        return admin.login(ip, &req.body);
    }
    if !admin.authorized(req.cookie.as_deref()) {
        return Response::error(401, "log in");
    }
    match admin.api(&req.method, &path, &req.body, req.cookie.as_deref()) {
        Ok(r) => r,
        Err(e) => Response::error(400, &format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct Login {
    user: String,
    password: String,
}

#[derive(Deserialize)]
struct Pair {
    client: u64,
    host: u64,
}

#[derive(Deserialize)]
struct ClientId {
    client: u64,
}

#[derive(Deserialize)]
struct DeviceId {
    device: u64,
}

/// The frame rate and the cap, each on its own; `null`: not set.
#[derive(Deserialize)]
struct LimitsJson {
    fps: Option<u16>,
    /// 0: auto.
    mbps: Option<f32>,
}

#[derive(Deserialize)]
struct AdminName {
    client: u64,
    /// `null` or blank: cleared (the client's own name is shown).
    name: Option<String>,
}

#[derive(Deserialize)]
struct FilesSwitch {
    /// Absent: everyone (then `allowed` is not null).
    client: Option<u64>,
    /// `null` for a client: as everyone.
    allowed: Option<bool>,
}

#[derive(Deserialize)]
struct FilesSpeed {
    mbps: u16,
}

#[derive(Deserialize)]
struct Override {
    /// Absent: everyone's.
    client: Option<u64>,
    limits: LimitsJson,
}

fn limits_json(l: Limits) -> Value {
    json!({ "fps": l.fps, "mbps": l.mbps })
}

impl Admin {
    fn login(&self, ip: IpAddr, body: &[u8]) -> Response {
        {
            let f = self.failures.lock().unwrap();
            if let Some((n, last)) = f.get(&ip)
                && *n >= MAX_FAILURES
                && last.elapsed() < LOCKOUT
            {
                return Response::error(429, "too many wrong passwords: wait a minute");
            }
        }
        let Ok(login) = serde_json::from_slice::<Login>(body) else {
            return Response::error(400, "user and password expected");
        };
        let ok = same(&hash(&login.user), &hash(&self.cfg.user))
            & same(&hash(&login.password), &self.password_hash);
        if !ok {
            let mut f = self.failures.lock().unwrap();
            let e = f.entry(ip).or_insert((0, Instant::now()));
            if e.1.elapsed() >= LOCKOUT {
                e.0 = 0;
            }
            e.0 += 1;
            e.1 = Instant::now();
            println!("admin: wrong login from {ip} ({} in a row)", e.0);
            return Response::error(401, "wrong login or password");
        }
        self.failures.lock().unwrap().remove(&ip);
        let id: String = random_bytes::<32>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        {
            let mut s = self.sessions.lock().unwrap();
            s.retain(|_, until| *until > Instant::now());
            s.insert(id.clone(), Instant::now() + SESSION_TTL);
        }
        println!("admin: logged in from {ip}");
        let mut r = Response::ok();
        r.headers.push(format!(
            "Set-Cookie: {COOKIE}={id}; Path={PREFIX}; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
            SESSION_TTL.as_secs()
        ));
        r
    }

    fn authorized(&self, cookie: Option<&str>) -> bool {
        let Some(id) = cookie else {
            return false;
        };
        let s = self.sessions.lock().unwrap();
        s.get(id).is_some_and(|until| *until > Instant::now())
    }

    fn api(&self, method: &str, path: &str, body: &[u8], cookie: Option<&str>) -> Result<Response> {
        let db = Db::open(&self.hub.db_path)?;
        let parse = |what| move |e: serde_json::Error| anyhow::anyhow!("{what} expected: {e}");
        Ok(match (method, path) {
            ("GET", "/api/state") => Response::json(200, &self.state(&db)?),
            ("POST", "/api/logout") => {
                if let Some(id) = cookie {
                    self.sessions.lock().unwrap().remove(id);
                }
                let mut r = Response::ok();
                r.headers.push(format!(
                    "Set-Cookie: {COOKIE}=; Path={PREFIX}; HttpOnly; Secure; SameSite=Strict; Max-Age=0"
                ));
                r
            }
            ("POST", "/api/approve") => {
                let p: Pair = serde_json::from_slice(body).map_err(parse("client and host"))?;
                db.approve(p.client, p.host)?;
                println!("admin: approved client {} for host {}", p.client, p.host);
                Response::ok()
            }
            ("POST", "/api/reject") => {
                let p: Pair = serde_json::from_slice(body).map_err(parse("client and host"))?;
                db.reject(p.client, p.host)?;
                println!("admin: rejected client {} for host {}", p.client, p.host);
                Response::ok()
            }
            ("POST", "/api/unlink") => {
                let p: Pair = serde_json::from_slice(body).map_err(parse("client and host"))?;
                if !db.unlink(p.client, p.host)? {
                    bail!("client {} has no access to host {}", p.client, p.host);
                }
                println!("admin: unlinked client {} from host {}", p.client, p.host);
                Response::ok()
            }
            ("POST", "/api/unlink-all") => {
                let c: ClientId = serde_json::from_slice(body).map_err(parse("client"))?;
                let n = db.unlink_all(c.client)?;
                println!("admin: unlinked client {} from {n} host(s)", c.client);
                Response::json(200, &json!({ "ok": true, "unlinked": n }))
            }
            ("POST", "/api/remove") => {
                let d: DeviceId = serde_json::from_slice(body).map_err(parse("device"))?;
                if !db.remove_device(d.device)? {
                    bail!("no device {}", d.device);
                }
                println!("admin: removed device {}", d.device);
                Response::ok()
            }
            ("POST", "/api/override") => {
                let o: Override = serde_json::from_slice(body).map_err(parse("limits"))?;
                let l = Limits {
                    fps: o.limits.fps,
                    mbps: o.limits.mbps,
                };
                match o.client {
                    Some(c) => db.set_client_limits(c, l)?,
                    None => db.set_global_limits(l)?,
                }
                Response::ok()
            }
            ("POST", "/api/files") => {
                let f: FilesSwitch = serde_json::from_slice(body).map_err(parse("files"))?;
                match (f.client, f.allowed) {
                    (Some(c), a) => db.set_client_files(c, a)?,
                    (None, Some(a)) => db.set_global_files(a)?,
                    (None, None) => bail!("everyone's switch is on or off"),
                }
                Response::ok()
            }
            ("POST", "/api/name") => {
                let n: AdminName =
                    serde_json::from_slice(body).map_err(parse("client and name"))?;
                db.set_admin_name(n.client, n.name.as_deref())?;
                println!("admin: named client {}", n.client);
                Response::ok()
            }
            ("POST", "/api/files-speed") => {
                let f: FilesSpeed = serde_json::from_slice(body).map_err(parse("files speed"))?;
                db.set_files_mbps(f.mbps)?;
                Response::ok()
            }
            (_, p) if p.starts_with("/api/") => Response::error(404, "no such call"),
            _ => Response::error(404, "not found"),
        })
    }

    fn state(&self, db: &Db) -> Result<Value> {
        let online_hosts: HashMap<u64, bool> = self.hub.online_hosts().into_iter().collect();
        let online_clients = self.hub.online_clients();
        let updating = self.hub.updating_hosts();
        let hosts: Vec<Value> = db
            .hosts()?
            .into_iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "machine": h.machine,
                    "account": h.account,
                    "online": online_hosts.contains_key(&h.id),
                    "active": online_hosts.get(&h.id).copied().unwrap_or(false),
                    "last_seen": h.last_seen,
                    "version": h.version,
                    "updating": updating.contains(&h.id),
                    "update_failed": h.update_failed,
                })
            })
            .collect();
        let clients: Vec<Value> = db
            .clients()?
            .into_iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "name": c.name,
                    "admin_name": c.admin_name,
                    "override": limits_json(c.limits),
                    "files": c.files,
                    "online": online_clients.contains(&c.id),
                    "last_seen": c.last_seen,
                    "version": c.version,
                })
            })
            .collect();
        let access: Vec<Value> = db
            .access_list(None)?
            .into_iter()
            .map(|a| {
                json!({
                    "client": a.client,
                    "client_name": a.client_name,
                    "host": a.host,
                    "machine": a.machine,
                    "account": a.account,
                    "status": access_name(a.access),
                    "requested": a.requested,
                    "decided": a.decided,
                })
            })
            .collect();
        let sessions: Vec<Value> = self
            .hub
            .sessions()
            .into_iter()
            .map(|s| {
                json!({
                    // As text: JavaScript numbers lose u64 precision.
                    "id": format!("{:016x}", s.id),
                    "client": s.client,
                    "client_name": s.client_name,
                    "host": s.host,
                    "fps": s.quality.fps,
                    "mbps": s.quality.mbps,
                    "seconds": s.duration.as_secs(),
                })
            })
            .collect();
        let pending = db.access_list(Some(Access::Pending))?.len();
        Ok(json!({
            "user": self.cfg.user,
            "now": now(),
            "fps_choices": FPS_CHOICES,
            "mbps_presets": MBPS_PRESETS,
            "global_override": limits_json(db.global_limits()?),
            "files": {
                "allowed": db.global_files()?.allowed,
                "mbps": db.global_files()?.mbps,
            },
            "pending": pending,
            "hosts": hosts,
            "clients": clients,
            "access": access,
            "sessions": sessions,
        }))
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// A file of the app, or its `index.html` for any other path (client-side
/// routes). Only plain relative paths: no `..`, no backslashes.
fn static_file(web: Option<&Path>, path: &str) -> Response {
    let Some(web) = web else {
        return Response {
            status: 200,
            content_type: "text/plain; charset=utf-8",
            body: b"Lumen server: the admin app is not installed (--web).".to_vec(),
            headers: Vec::new(),
            csp: ADMIN_CSP,
        };
    };
    let mut file = web.join(safe_rel(path).unwrap_or("index.html"));
    if !file.is_file() {
        file = web.join("index.html");
    }
    file_response(&file, ADMIN_CSP)
}

/// A file of the landing page; a folder's `index.html`. No fallback: an
/// unknown path is 404.
fn site_file(site: &Path, path: &str) -> Response {
    let rel = match safe_rel(path) {
        Some(rel) if !path.ends_with('/') => rel.to_string(),
        Some(rel) => format!("{rel}/index.html"),
        None if path.trim_start_matches('/').is_empty() => "index.html".into(),
        None => return Response::error(404, "not found"),
    };
    let file = site.join(rel);
    if !file.is_file() {
        return Response::error(404, "not found");
    }
    file_response(&file, SITE_CSP)
}

/// A path's plain relative part: no `..`, `.`, empty parts, backslashes or
/// drive letters; `None` for those and for the root.
fn safe_rel(path: &str) -> Option<&str> {
    let rel = path.trim_start_matches('/').trim_end_matches('/');
    let safe = rel
        .split('/')
        .all(|c| !c.is_empty() && c != ".." && c != "." && !c.contains('\\') && !c.contains(':'));
    (safe && !rel.is_empty()).then_some(rel)
}

fn file_response(file: &Path, csp: &'static str) -> Response {
    match std::fs::read(file) {
        Ok(body) => Response {
            status: 200,
            content_type: content_type(file),
            body,
            headers: Vec::new(),
            csp,
        },
        Err(_) => Response::error(404, "not found"),
    }
}
