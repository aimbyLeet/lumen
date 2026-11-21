//! lumen-server: the signaling server and its admin commands (until the web
//! admin: requests, access, overrides from the command line).

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use lumen_server::db::{Db, access_name, now};
use lumen_server::{Config, Server, dev_cert, key_fingerprint, self_cert, tls_config_from_files};
use protocol::signal::{Access, Limits};

#[derive(Parser)]
#[command(about = "Lumen server: devices, access, sessions")]
struct Cli {
    /// Database file (also $LUMEN_DB).
    #[arg(long, env = "LUMEN_DB", default_value = "lumen.db", global = true)]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server.
    Serve(Box<ServeArgs>),
    /// Registered hosts (the machine; old rows may carry a Windows account).
    Hosts,
    /// Registered clients.
    Clients,
    /// Access of clients to hosts (requests waiting for the admin first).
    Access {
        /// Only these: pending, approved, rejected.
        #[arg(long)]
        status: Option<String>,
    },
    /// Let a client that asked connect to a host.
    Approve { client: u64, host: u64 },
    /// Refuse a request; the client cannot ask for that host again.
    Reject { client: u64, host: u64 },
    /// Take access back (or lift a rejection); a running session ends.
    Unlink { client: u64, host: u64 },
    /// Take back every access of a client.
    UnlinkAll { client: u64 },
    /// Remove a host registration or a client, with its access.
    Remove { device: u64 },
    /// Frame rate and cap set by the admin, each on its own: for everyone, or
    /// for one client (its own wins).
    Override {
        #[command(subcommand)]
        command: OverrideCommand,
    },
    /// File transfers: on or off for everyone or one client (wins), and
    /// their speed cap.
    Files {
        #[command(subcommand)]
        command: FilesCommand,
    },
    /// Bitrate caps of «auto» per picture size and frame rate.
    Cap {
        #[command(subcommand)]
        command: CapCommand,
    },
    /// Write a development CA (ca.pem, for devices' --server-ca) and a server
    /// certificate it signed (cert.pem, key.pem) for the given names.
    DevCert {
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// DNS names or IP addresses the certificate is for.
        #[arg(default_values_t = vec!["localhost".to_string(), "127.0.0.1".to_string()])]
        names: Vec<String>,
    },
    /// The server's own key and certificate (a local network: no domain, no
    /// Let's Encrypt) in a folder: the key is made once, the certificate is
    /// issued again when the names change. Prints the key's fingerprint,
    /// which devices show before trusting the server.
    SelfCert {
        #[arg(long)]
        dir: PathBuf,
        /// IP addresses or names devices use to reach the server.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// The fingerprint of a private key (PEM), as devices show it.
    Fingerprint { key: PathBuf },
}

#[derive(Args)]
struct ServeArgs {
    /// TCP address for signaling (TLS).
    #[arg(long, default_value = "0.0.0.0:7443")]
    listen: SocketAddr,
    /// Certificate chain, PEM.
    #[arg(long)]
    cert: PathBuf,
    /// Private key, PEM.
    #[arg(long)]
    key: PathBuf,
    /// TCP address of the web admin (HTTPS, the same certificate).
    #[arg(long, default_value = "0.0.0.0:443")]
    admin_listen: SocketAddr,
    /// The admin's login; without it (and a password) there is no web admin.
    #[arg(long, env = "LUMEN_ADMIN_USER")]
    admin_user: Option<String>,
    /// The admin's password (at least 8 characters).
    #[arg(long, env = "LUMEN_ADMIN_PASSWORD", hide_env_values = true)]
    admin_password: Option<String>,
    /// The built admin app (web/admin/dist).
    #[arg(long, env = "LUMEN_WEB")]
    web: Option<PathBuf>,
    /// The admin speaks plain HTTP to a reverse proxy on this machine
    /// (nginx with its own certificate): loopback connections only, the
    /// browser's address from `X-Real-IP`. Listen on 127.0.0.1 then.
    #[arg(long)]
    admin_behind_proxy: bool,
    /// The landing page (index.html, the installer), served at the admin's
    /// root: when no nginx does it.
    #[arg(long, env = "LUMEN_SITE")]
    site: Option<PathBuf>,
    /// Plain HTTP here sends browsers to the admin's HTTPS (e.g. 0.0.0.0:80).
    #[arg(long)]
    redirect_listen: Option<SocketAddr>,
    /// The folder with `lumen-setup-<version>.exe` that devices update to
    /// (the update door; in the image: /usr/share/lumen/update).
    #[arg(long, env = "LUMEN_UPDATE")]
    update_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum OverrideCommand {
    Show,
    /// Everyone: frame rate (30, 60) and cap (Mbit/s or «auto»). «off» or
    /// nothing: not set, the client chooses (`off 40`: only the cap).
    Global {
        fps: String,
        mbps: Option<String>,
    },
    /// One client: same arguments.
    Client {
        client: u64,
        fps: String,
        mbps: Option<String>,
    },
}

#[derive(Subcommand)]
enum FilesCommand {
    Show,
    /// Everyone: on or off (a client's own switch wins).
    Global {
        state: OnOff,
    },
    /// One client: on, off, or `default` (as everyone).
    Client {
        client: u64,
        state: String,
    },
    /// The speed cap of file data, Mbit/s (each side of a session).
    Speed {
        mbps: u16,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum CapCommand {
    List,
    /// Set the cap: tier (fhd, 2k, 4k), frame rate (30, 60), Mbit/s of video.
    Set {
        tier: String,
        fps: u16,
        mbps: f32,
    },
}

/// `<fps|off> [mbps|auto|off]`: «off» or nothing leaves that one to the client.
fn parse_limits(fps: &str, mbps: Option<&str>) -> Result<Limits> {
    Ok(Limits {
        fps: match fps {
            "off" => None,
            f => Some(f.parse()?),
        },
        mbps: match mbps {
            None | Some("off") => None,
            Some("auto") => Some(0.0),
            Some(m) => Some(m.parse()?),
        },
    })
}

fn limits_text(l: Limits) -> String {
    let fps = l
        .fps
        .map_or("client's fps".to_string(), |f| format!("{f} fps"));
    let cap = match l.mbps {
        None => "client's cap".to_string(),
        Some(m) if m <= 0.0 => "auto".to_string(),
        Some(m) => format!("{m} Mbit/s"),
    };
    if l.is_empty() {
        "-".into()
    } else {
        format!("{fps}, {cap}")
    }
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

fn ago(t: i64) -> String {
    if t == 0 {
        return "never".into();
    }
    let s = (now() - t).max(0);
    match s {
        0..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = || Db::open(&cli.db);
    match cli.command {
        Command::Serve(args) => serve(&cli.db, *args)?,
        Command::Hosts => hosts(&db()?)?,
        Command::Clients => clients(&db()?)?,
        Command::Access { status } => access(&db()?, status.as_deref())?,
        Command::Approve { client, host } => db()?.approve(client, host)?,
        Command::Reject { client, host } => db()?.reject(client, host)?,
        Command::Unlink { client, host } => {
            if !db()?.unlink(client, host)? {
                bail!("client {client} had no access to host {host}");
            }
        }
        Command::UnlinkAll { client } => {
            let n = db()?.unlink_all(client)?;
            println!("took back {n} access(es)");
        }
        Command::Remove { device } => {
            if !db()?.remove_device(device)? {
                bail!("no device {device}");
            }
        }
        Command::Override { command } => override_quality(&db()?, command)?,
        Command::Files { command } => files(&db()?, command)?,
        Command::Cap { command } => cap(&db()?, command)?,
        Command::DevCert { out, names } => write_dev_cert(&out, &names)?,
        Command::SelfCert { dir, names } => {
            let c = self_cert(&dir, &names)?;
            println!(
                "{} key, certificate for {names:?} in {}",
                if c.new_key { "new" } else { "the same" },
                dir.display()
            );
            println!("fingerprint: {}", c.fingerprint);
        }
        Command::Fingerprint { key } => {
            let pem = std::fs::read_to_string(&key)?;
            println!("{}", key_fingerprint(&pem)?);
        }
    }
    Ok(())
}

/// `serve`: the server until it is stopped.
fn serve(db: &std::path::Path, a: ServeArgs) -> Result<()> {
    let admin = match (a.admin_user, a.admin_password) {
        (Some(user), Some(password)) => Some(lumen_server::admin::AdminConfig {
            listen: a.admin_listen,
            user,
            password,
            web: a.web,
            behind_proxy: a.admin_behind_proxy,
            site: a.site,
            redirect: a.redirect_listen,
        }),
        (None, None) => None,
        _ => bail!("the web admin needs both --admin-user and --admin-password"),
    };
    let update = match &a.update_dir {
        Some(dir) => lumen_server::update::UpdateFile::find(dir)?,
        None => None,
    };
    match &update {
        Some(u) => {
            println!(
                "lumen-server: updates to Lumen {} ({}, {} bytes)",
                u.version,
                u.path.display(),
                u.size
            );
            if u.version != protocol::signal::APP_VERSION {
                println!(
                    "lumen-server: warning: the installer is {}, this server {}",
                    u.version,
                    protocol::signal::APP_VERSION
                );
            }
        }
        None => println!("lumen-server: no installer, devices are not updated"),
    }
    let server = Server::start(Config {
        admin,
        update,
        ..Config::new(
            a.listen,
            db.to_path_buf(),
            tls_config_from_files(&a.cert, &a.key, lumen_server::CERT_CHECK_EVERY)?,
        )
    })?;
    println!(
        "lumen-server: signaling on {} (TLS), admin {}, database {}",
        server.addr(),
        server
            .admin_addr()
            .map_or("off".to_string(), |at| if a.admin_behind_proxy {
                format!("http on {at} (behind a proxy)")
            } else {
                format!("https on {at}")
            }),
        db.display()
    );
    if let Some(at) = server.redirect_addr() {
        println!("lumen-server: http on {at} redirects to the admin");
    }
    server.wait();
    Ok(())
}

fn hosts(db: &Db) -> Result<()> {
    for h in db.hosts()? {
        println!(
            "{:>4}  {}  Lumen {}  (seen {})",
            h.id,
            h.label(),
            or_dash(&h.version),
            ago(h.last_seen)
        );
    }
    Ok(())
}

fn clients(db: &Db) -> Result<()> {
    for c in db.clients()? {
        println!(
            "{:>4}  {}  Lumen {}  override: {}  (seen {})",
            c.id,
            // As the admin panel shows it: the admin's name first.
            match &c.admin_name {
                Some(a) => format!("{a} [{}]", c.name),
                None => c.name.clone(),
            },
            or_dash(&c.version),
            limits_text(c.limits),
            ago(c.last_seen)
        );
    }
    Ok(())
}

/// `access`: requests waiting for the admin first.
fn access(db: &Db, status: Option<&str>) -> Result<()> {
    let status = match status {
        None => None,
        Some("pending") => Some(Access::Pending),
        Some("approved") => Some(Access::Approved),
        Some("rejected") => Some(Access::Rejected),
        Some(s) => bail!("unknown status {s:?}"),
    };
    let mut rows = db.access_list(status)?;
    rows.sort_by_key(|r| r.access != Access::Pending);
    for r in rows {
        println!(
            "{:<8}  client {:>3} {}  ->  host {:>3} {}  (asked {})",
            access_name(r.access),
            r.client,
            r.client_name,
            r.host,
            r.label(),
            ago(r.requested)
        );
    }
    Ok(())
}

fn override_quality(db: &Db, command: OverrideCommand) -> Result<()> {
    match command {
        OverrideCommand::Show => {
            println!("global: {}", limits_text(db.global_limits()?));
            for c in db.clients()?.into_iter().filter(|c| !c.limits.is_empty()) {
                println!("client {} {}: {}", c.id, c.name, limits_text(c.limits));
            }
        }
        OverrideCommand::Global { fps, mbps } => {
            db.set_global_limits(parse_limits(&fps, mbps.as_deref())?)?
        }
        OverrideCommand::Client { client, fps, mbps } => {
            db.set_client_limits(client, parse_limits(&fps, mbps.as_deref())?)?
        }
    }
    Ok(())
}

fn files(db: &Db, command: FilesCommand) -> Result<()> {
    match command {
        FilesCommand::Show => {
            let g = db.global_files()?;
            println!(
                "everyone: {}, {} Mbit/s",
                if g.allowed { "on" } else { "off" },
                g.mbps
            );
            for c in db.clients()?.into_iter().filter(|c| c.files.is_some()) {
                println!(
                    "client {} {}: {}",
                    c.id,
                    c.name,
                    if c.files == Some(true) { "on" } else { "off" }
                );
            }
        }
        FilesCommand::Global { state } => db.set_global_files(matches!(state, OnOff::On))?,
        FilesCommand::Client { client, state } => {
            let allowed = match state.as_str() {
                "on" => Some(true),
                "off" => Some(false),
                "default" => None,
                _ => bail!("on, off or default"),
            };
            db.set_client_files(client, allowed)?
        }
        FilesCommand::Speed { mbps } => db.set_files_mbps(mbps)?,
    }
    Ok(())
}

fn cap(db: &Db, command: CapCommand) -> Result<()> {
    match command {
        CapCommand::List => {
            for (tier, fps, mbps) in db.cap_table()? {
                println!("{tier:<4} {fps:>3} fps  {mbps} Mbit/s");
            }
        }
        CapCommand::Set { tier, fps, mbps } => db.set_cap(&tier, fps, mbps)?,
    }
    Ok(())
}

/// `dev-cert`: a development CA and a server certificate it signed.
fn write_dev_cert(out: &std::path::Path, names: &[String]) -> Result<()> {
    let c = dev_cert(names)?;
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("ca.pem"), c.ca_pem)?;
    std::fs::write(out.join("cert.pem"), c.cert_pem)?;
    std::fs::write(out.join("key.pem"), c.key_pem)?;
    println!(
        "wrote {0}/ca.pem (devices: --server-ca), {0}/cert.pem and {0}/key.pem for {names:?}",
        out.display()
    );
    Ok(())
}
