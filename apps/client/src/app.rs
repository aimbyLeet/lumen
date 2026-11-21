use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;

use crate::net::{self, Connection};
use crate::present::Presenter;
use crate::render::{
    Clear, Expected, InputScript, Mode, NetworkVideo, OpenSession, Reconnect, Renderer,
    TestPattern, VideoFile,
};
use crate::window::{self, Window};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};

#[derive(Parser)]
#[command(
    name = "lumen-client",
    about = "Lumen client (window + 1:1 presenter)",
    after_help = "Defaults come from client.ini in the data folder (next to client.key).\n\
                  A session: --server <host:port> --token <the host's token>; the first time the \
                  admin approves the request on the server.",
    args_override_self = true
)]
struct Args {
    /// Window client size in physical pixels, WxH (both even). Ignored with --play.
    #[arg(long, default_value = "1920x1080")]
    size: String,
    /// Borderless popup window at (0,0).
    #[arg(long)]
    borderless: bool,
    /// In a session: the overlay on from the start (Ctrl+Alt+Shift+O switches it).
    #[arg(long)]
    overlay: bool,
    /// In a session: system shortcuts (Win, Alt+Tab, Alt+Esc, Ctrl+Esc, ...) go to
    /// the host while the video window is in the foreground (`host`), or stay here (`local`).
    #[arg(long, value_enum, default_value_t = Shortcuts::Host)]
    shortcuts: Shortcuts,
    /// In a session: press system shortcuts with SendInput (never Win+L), check the
    /// window keeps the foreground, log the keys here (compare with lumen-host --input-log).
    /// With --shortcuts local only Win and Alt+Tab (they should escape).
    #[arg(long)]
    test_shortcuts: Option<PathBuf>,
    /// Exit after this many frames: in a session, pictures of the host shown (presents for the
    /// pointer alone do not count); else presents (default: run until closed / end of file).
    #[arg(long)]
    frames: Option<u64>,
    /// Draw the exact-code test pattern.
    #[arg(long)]
    test_pattern: bool,
    /// Play an HEVC Main10 Annex B file through the real decode + display path.
    #[arg(long)]
    play: Option<PathBuf>,
    /// In a session: every N-th shown frame, read the frame number the synthetic
    /// host source draws into the picture and check it (0 = off).
    #[arg(long, default_value_t = 0)]
    barcode_every: u64,
    /// In a session: send a deterministic sequence of N input events instead of the
    /// user's, logging them to --input-script-log (compare with lumen-host --input-log).
    #[arg(long, default_value_t = 0)]
    input_script: usize,
    #[arg(long, default_value = "client-input.log")]
    input_script_log: PathBuf,
    /// In a session with a synthetic host: every --verify-every frames, check that
    /// the host's pointer is drawn into the back buffer where expected.
    #[arg(long)]
    check_cursor: bool,
    /// In a session: share the clipboard's text with the host ("on"), only log
    /// the texts ("dry-run", --clipboard-log) or not ("off").
    #[arg(long, value_enum, default_value_t = crate::clipboard::ClipMode::On)]
    clipboard: crate::clipboard::ClipMode,
    /// Write "R|S <bytes> <hash>" per clipboard text received / sent (checks).
    #[arg(long)]
    clipboard_log: Option<PathBuf>,
    /// Checks: send this many generated texts to the host instead of the
    /// clipboard's (protocol::clipboard::script_text).
    #[arg(long, default_value_t = 0)]
    clipboard_script: u32,
    /// Checks: send these files (or folders) to the host once connected, as
    /// if copied and pasted there.
    #[arg(long)]
    send_file: Vec<PathBuf>,
    /// Received files go here instead of Downloads\Lumen (checks).
    #[arg(long)]
    downloads: Option<PathBuf>,
    /// The most our file data may take, Mbit/s, over the server's (checks).
    #[arg(long)]
    file_mbps: Option<f64>,
    /// Checks: as if these files (or folders) were copied here once connected.
    #[arg(long, hide = true)]
    clip_copy: Vec<PathBuf>,
    /// Checks: as if this PNG picture was copied here once connected.
    #[arg(long, hide = true)]
    clip_copy_image: Option<PathBuf>,
    /// Checks: this many seconds in, a paste in another program here
    /// (again for each one given).
    #[arg(long, hide = true)]
    test_paste: Vec<f64>,
    /// Checks: this many seconds in, Ctrl+V typed in the video window (again
    /// for each one given).
    #[arg(long, hide = true)]
    test_host_paste: Vec<f64>,
    /// Checks: this many seconds after the start, cancel every transfer
    /// running then (as the overlay's buttons do).
    #[arg(long, hide = true)]
    test_cancel_files: Option<f64>,
    /// The host's sound: played on the default output device, not at all, or
    /// taken at the pace of the clock and dropped ("null": checks without noise).
    #[arg(long, value_enum, default_value_t = crate::audio::AudioOut::On)]
    audio: crate::audio::AudioOut,
    /// Write the sound handed to the device (float WAV at its rate; checks).
    #[arg(long)]
    audio_dump: Option<PathBuf>,
    /// Checks: emulate the output device's clock being off by this many ppm
    /// (the player must keep its level by playing faster or slower).
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    sim_audio_drift: f64,
    /// In a session: drop this percentage of incoming datagrams (tests FEC and loss recovery).
    #[arg(long, default_value_t = 0.0)]
    sim_loss: f64,
    /// In a session: emulate a downlink of this many Mbit/s (0 = off).
    #[arg(long, default_value_t = 0.0)]
    sim_bandwidth: f64,
    /// In a session: delay incoming datagrams by this many ms (the RTT grows by it).
    #[arg(long, default_value_t = 0)]
    sim_delay: u64,
    /// In a session: every <ms> the downlink stalls for <ms>, then delivers what it
    /// held at once (`2000,55`: Wi-Fi hiccups seen through WG).
    #[arg(long, value_parser = parse_stall)]
    sim_stall: Option<(Duration, Duration)>,
    /// In a session: write "<frame> <hash>" of every decoded picture here
    /// (compare with lumen-decode --hashes of lumen-host --dump-stream).
    #[arg(long)]
    frame_hashes: Option<PathBuf>,
    /// In a session: write every decoded frame's times ("D <frame> <capture> <reassembled>
    /// <decoded> <decode started>", µs; capture on the host's clock), every first
    /// presentation ("P <frame> <time>") and every render + present call ("R <begin>
    /// <end>") here: where skipped frames and delays come from.
    #[arg(long)]
    frame_log: Option<PathBuf>,
    /// In a session: write every datagram from the host here ("V <seq> <arrival µs>
    /// <bytes> <frame> <block> <index> <data> <parity>" for video, "C <seq> <µs> <bytes>"
    /// for the rest): packet loss and arrival analysis.
    #[arg(long)]
    packet_log: Option<PathBuf>,
    /// Checks: the host's monitors to ask for, a bit each (1 = monitor 1
    /// only); by default all of them.
    #[arg(long)]
    show_monitors: Option<u8>,
    /// A host without monitors asks for ours: answer this at once,
    /// WIDTHxHEIGHT@SCALE (3440x1440@125) — checks; Lumen Client asks the user.
    #[arg(long)]
    monitor_choice: Option<crate::monitor_pick::MonitorPick>,
    /// Checks of several monitors: the frames of monitor 2 and on, as they
    /// came, to `<path>.<stream>` (compare with the host's `--dump-stream`).
    #[arg(long)]
    dump_streams: Option<PathBuf>,
    /// Checks of several monitors: start in this view once the host tells
    /// them (1: all in the window, 2: monitor 1 alone, 3: a window each).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
    view: Option<u8>,
    /// The machine's view remembered here (Lumen Client gives each card a
    /// file): with the same monitors at the host the session starts as the
    /// last one was (mode, the windows' places); written as it changes.
    #[arg(long)]
    view_memory: Option<PathBuf>,
    /// In a session, the host's pacing of frame data: "auto" (picked from how
    /// frames arrive), "host" (the host's own), or "<Mbit/s>,<KiB>" (bursts of at
    /// most that many KiB back to back; 0 = the host's own).
    #[arg(long, default_value = "auto", value_parser = parse_pace)]
    pace: net::Pacing,
    /// In a session, the host's FEC parity: "auto" (picked from the losses),
    /// "host" (the host's own), or a percent of data shards.
    #[arg(long, default_value = "auto", value_parser = parse_fec)]
    fec: net::Fec,
    /// Signaling server (host:port): it finds the host by its token, checks the
    /// admin approved this client, and hands out the host's addresses.
    #[arg(long)]
    server: Option<String>,
    /// CA of the server's certificate (development); default: the system trust store.
    #[arg(long)]
    server_ca: Option<PathBuf>,
    /// Trust only the server's key with this fingerprint (a server with its
    /// own certificate, `signal::pin`; Lumen Client keeps what the user trusted).
    #[arg(long)]
    server_pin: Option<String>,
    /// The host's token (shown by the host): which host, and the secret of the
    /// session's encryption.
    #[arg(long, env = "LUMEN_HOST_TOKEN", hide_env_values = true)]
    token: Option<transport::HostToken>,
    /// Read the host's token from the first line of stdin, instead of --token:
    /// other programs on this machine can read a command line (Lumen Client
    /// passes it this way).
    #[arg(long)]
    token_stdin: bool,
    /// This client's name on the server (registration; default: the Windows user).
    #[arg(long)]
    name: Option<String>,
    /// This client's key file (default: client.key in the data folder; created on first use).
    #[arg(long)]
    key: Option<PathBuf>,
    /// Frame rate to ask for, 30 or 60 (the admin may force another).
    #[arg(long, default_value_t = 60)]
    fps: u16,
    /// Ask the host not to lock Windows when this session ends (it still locks
    /// before another client's session).
    #[arg(long)]
    keep_unlocked: bool,
    /// Bitrate cap to ask for, Mbit/s; 0 = auto (the server's table by picture
    /// size and frame rate). The admin may force another.
    #[arg(long, default_value_t = 0.0)]
    max_mbps: f32,
    /// This client's credentials on the server (default: client.creds in the
    /// data folder; written when it registers). The Lumen Client app keeps one per server.
    #[arg(long)]
    creds: Option<PathBuf>,
    /// Window title (default: Lumen client).
    #[arg(long, default_value = "Lumen client")]
    title: String,
    /// Read the back buffer back every N-th frame and compare it with the CPU
    /// reference (`color` crate). 0 = only the first and last frame.
    #[arg(long, default_value_t = 0)]
    verify_every: u64,
    /// Present sync interval (1 = vsync).
    #[arg(long, default_value_t = 1)]
    sync: u32,
    /// Test: every 3 s hold the window in Windows' move loop for 1.5 s, as if
    /// it were dragged (the mouse cursor jumps to the title bar).
    #[arg(long)]
    test_move_loop: bool,
    /// Report the session's state as `@status {json}` lines on stdout (for the Lumen Client app).
    #[arg(long)]
    status_json: bool,
}

/// Errors that the same attempt would hit again: this client unknown to the
/// server, no host with this token, no approval, another version of Lumen on
/// the host — unless the host is older ([`host_updates`]). Anything else
/// (network, server restarting, host offline, busy or not on screen) is
/// worth retrying.
fn is_permanent(e: &anyhow::Error) -> bool {
    use signal::{ConnectError, ConnectFailure};
    match e.downcast_ref::<ConnectError>() {
        Some(ConnectError::Version(_)) => !host_updates(e),
        Some(ConnectError::Login(_)) => true,
        Some(ConnectError::Refused(r)) => !matches!(
            r,
            ConnectFailure::Offline | ConnectFailure::Busy | ConnectFailure::Inactive
        ),
        _ => false,
    }
}

/// The host runs an older Lumen: the server asked it to update to ours
/// — the session is worth waiting for.
fn host_updates(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<signal::ConnectError>(),
        Some(signal::ConnectError::Version(host)) if signal::update::newer(signal::APP_VERSION, host)
    )
}

/// How long a host older than this client is waited for: it updates and
/// comes back (as long as the server shows «updating»), or it does not
/// update by itself.
const HOST_UPDATE_WAIT: Duration = Duration::from_secs(5 * 60);

/// The first session through the server: retried every second until it
/// opens or fails for good (e.g. the network is not up yet after login).
fn first_session(open: &mut OpenSession) -> Result<(Connection, signal::ClientSession)> {
    let mut last = String::new();
    let started = std::time::Instant::now();
    loop {
        match open() {
            Ok(s) => return Ok(s),
            Err(e) if is_permanent(&e) => return Err(e),
            Err(e) if host_updates(&e) && started.elapsed() > HOST_UPDATE_WAIT => return Err(e),
            Err(e) => {
                crate::status::retrying(&e);
                let msg = format!("{e:#}");
                if msg != last {
                    println!("cannot connect yet ({msg}); retrying every second");
                    last = msg;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// Where the client keeps its server credentials.
fn credentials_path(arg: &Option<PathBuf>) -> PathBuf {
    arg.clone()
        .unwrap_or_else(|| transport::default_dir().join("client.creds"))
}

/// Held while this process has a session as the device of `creds`.
struct OneWindow {
    _mutex: desktop::Handle,
}

/// One video window per device: the server keeps one session per client and a
/// new one ends the old, so two copies would take the host from each other in
/// turn, each reconnecting. The second copy stops instead (a named mutex on the
/// credentials file, the device's identity).
fn one_window_per_device(creds: &std::path::Path) -> Result<OneWindow> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    // The file may not exist yet; its folder does.
    let path = match (creds.parent(), creds.file_name()) {
        (Some(dir), Some(file)) => std::fs::canonicalize(dir)
            .map(|d| d.join(file))
            .unwrap_or_else(|_| creds.to_path_buf()),
        _ => creds.to_path_buf(),
    };
    // A mutex name takes anything but a backslash.
    let name = format!(
        "Local\\lumen-client {}",
        path.display().to_string().to_lowercase().replace('\\', "/")
    );
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let handle = unsafe { CreateMutexW(None, false, windows::core::PCWSTR(wide.as_ptr())) }
        .context("CreateMutexW")?;
    // SAFETY: just made, ours alone. (No call in between: `GetLastError` below.)
    let guard = OneWindow {
        _mutex: unsafe { desktop::Handle::from_raw(handle) },
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        bail!(
            "another lumen-client window is already open as this device ({}): \
             close it first (Ctrl+Alt+Shift+Q)",
            creds.display()
        );
    }
    Ok(guard)
}

fn load_credentials(path: &std::path::Path) -> Result<Option<signal::Credentials>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(
            t.parse().map_err(|e| anyhow!("{}: {e}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Logs in (registering this client the first time), asks for access to the
/// host and waits until the admin approves it.
fn sign_up_and_wait(
    cfg: &signal::ServerConfig,
    key: &transport::StaticKey,
    name: &str,
    lookup: [u8; 32],
    path: &std::path::Path,
) -> Result<signal::Credentials> {
    use signal::{Access, ClientLink};
    let creds = load_credentials(path)?;
    let mut link = ClientLink::open(cfg, creds.as_ref(), name, key.public().0)
        .map_err(|e| anyhow!("{}: {e}", cfg.server))?;
    let creds = link.credentials();
    if load_credentials(path)?.is_none() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, format!("{creds}\n"))?;
        println!(
            "registered on {} as client {} ({name}); credentials in {}",
            cfg.server,
            creds.device,
            path.display()
        );
    }
    let (mut card, forced) = link.add_host(lookup)?;
    if let Some(fps) = forced.fps {
        println!("the admin sets this client's frame rate: {fps} fps");
    }
    if let Some(mbps) = forced.mbps {
        println!("the admin sets this client's cap: {}", cap_text(mbps));
    }
    let mut waiting = false;
    loop {
        let Some(host) = &card.host else {
            bail!(
                "no host has this token on {} (mistyped, or the host changed it)",
                cfg.server
            );
        };
        match host.access {
            Access::Approved => break,
            Access::Rejected => bail!(
                "the admin rejected this client for {} / {}",
                host.machine,
                host.account
            ),
            // Taken back while we waited: ask again.
            Access::None => card = link.add_host(lookup)?.0,
            Access::Pending => {
                if !waiting {
                    println!(
                        "asked for access to {} / {}: waiting for the admin to approve it",
                        host.machine, host.account
                    );
                    waiting = true;
                }
                std::thread::sleep(Duration::from_secs(2));
                card = link.status(&[lookup])?.0.remove(0);
            }
        }
    }
    link.close();
    Ok(creds)
}

fn cap_text(mbps: f32) -> String {
    if mbps <= 0.0 {
        "auto".into()
    } else {
        format!("{mbps} Mbit/s")
    }
}

/// Asks the server for a session with the host and opens it at whichever of
/// the host's addresses answers.
fn open_via_server(
    cfg: &signal::ServerConfig,
    creds: &signal::Credentials,
    token: &transport::HostToken,
    quality: signal::Quality,
    base: &net::Options,
) -> Result<(Connection, signal::ClientSession)> {
    let session = signal::connect_to_host(cfg, creds, token.lookup(), quality)?;
    let addrs: Vec<SocketAddr> = session
        .addrs
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    let mut opts = base.clone();
    if let Some(f) = opts.files.as_mut() {
        f.policy = session.files();
    }
    opts.host_key = transport::PublicKey(session.host_key);
    opts.psk = token.psk();
    opts.fps = session.quality.fps;
    let conn = Connection::open(&addrs, Duration::from_secs(10), opts)?;
    println!(
        "session {:016x}: {} fps, cap {}",
        session.id,
        session.quality.fps,
        cap_text(session.quality.mbps)
    );
    Ok((conn, session))
}

/// `--shortcuts`: where system shortcuts go in a session.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Shortcuts {
    Host,
    Local,
}

/// `--fec auto`, `host` or `5` (percent).
fn parse_fec(s: &str) -> Result<net::Fec, String> {
    match s.trim() {
        "auto" => Ok(net::Fec::Auto),
        "host" => Ok(net::Fec::Host),
        p => match p.parse::<f64>() {
            Ok(p) if (0.1..=100.0).contains(&p) => Ok(net::Fec::Fixed((p * 10.0).round() as u16)),
            _ => Err("--fec auto | host | <percent, 0.1..100>".into()),
        },
    }
}

/// `--pace auto`, `host` or `150,8` (kbit/s, bytes).
fn parse_pace(s: &str) -> Result<net::Pacing, String> {
    match s.trim() {
        "auto" => return Ok(net::Pacing::Auto),
        "host" => return Ok(net::Pacing::Host),
        _ => {}
    }
    let usage = "--pace auto | host | <Mbit/s>,<KiB>";
    let (rate, burst) = s.split_once(',').unwrap_or((s, "0"));
    let rate: f64 = rate.trim().parse().map_err(|_| usage)?;
    let burst: u32 = burst.trim().parse().map_err(|_| usage)?;
    Ok(net::Pacing::Fixed((rate * 1000.0) as u32, burst * 1024))
}

/// `--sim-stall 2000,55` (every ms, for ms).
fn parse_stall(s: &str) -> Result<(Duration, Duration), String> {
    let usage = "--sim-stall <every ms>,<length ms>";
    let (every, len) = s.split_once(',').ok_or(usage)?;
    let every: u64 = every.trim().parse().map_err(|_| usage)?;
    let len: u64 = len.trim().parse().map_err(|_| usage)?;
    if every == 0 || len >= every {
        return Err(usage.into());
    }
    Ok((Duration::from_millis(every), Duration::from_millis(len)))
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| anyhow!("--size must look like 1920x1080"))?;
    let w: u32 = w.parse().context("invalid width")?;
    let h: u32 = h.parse().context("invalid height")?;
    if w == 0 || h == 0 {
        bail!("--size must be non-zero");
    }
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        bail!("--size {w}x{h} rejected: both dimensions must be even (4:2:0 video)");
    }
    Ok((w, h))
}

#[derive(Default)]
struct Verification {
    frames: u64,
    pixels: u64,
    off_by_one: u64,
    beyond: u64,
}

impl Verification {
    /// Back buffer `got` of `frame` against the reference.
    fn check(&mut self, frame: u64, got: &[[u16; 3]], exp: &Expected) {
        let (mut off1, mut beyond) = (0u64, 0u64);
        for (a, b) in got.iter().zip(&exp.pixels) {
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            if d > exp.tolerance {
                beyond += 1;
            } else if d > 0 {
                off1 += 1;
            }
        }
        println!(
            "frame {frame}: {beyond} px beyond ±{} codes, {off1} px within tolerance but not exact",
            exp.tolerance
        );
        self.frames += 1;
        self.pixels += got.len() as u64;
        self.off_by_one += off1;
        self.beyond += beyond;
    }
}

/// How long the process may take to end after its session.
const EXIT_LIMIT: Duration = Duration::from_secs(5);

/// Ends the process [`EXIT_LIMIT`] from now if it has not ended by then: a
/// thread stuck in teardown kept a closed video window's process alive, and
/// Lumen Client's hub waited for it (2026-11-13).
fn exit_guard() {
    let _ = std::thread::Builder::new()
        .name("exit guard".into())
        .spawn(|| {
            desktop::nap::Nap::new().sleep(EXIT_LIMIT);
            eprintln!("the process did not end in {EXIT_LIMIT:?} after the session; ending it");
            std::process::exit(0);
        });
}

pub fn main() -> Result<()> {
    let config = transport::config_args(&transport::default_dir().join("client.ini"))?;
    let args = Args::parse_from(transport::with_config_args(
        std::env::args_os().collect(),
        config,
    ));
    window::init_process();
    if args.status_json {
        crate::status::enable();
    }

    // Multithread-protected device (CUDA interop touches it from inside the driver).
    let gpu = capture::GpuDevice::open_nvidia()?;
    let (device, ctx, adapter) = (gpu.device, gpu.context, gpu.adapter_name);
    println!("adapter: {adapter}");

    match &args.play {
        None if args.server.is_some() => {
            let _one = one_window_per_device(&credentials_path(&args.creds))?;
            // File transfers go on through reconnections: the service lives
            // as long as the window.
            let files = desktop::files::FileService::start();
            let mut video = connect(&args, &device, &files)?;
            let (mut win, presenter) = open_window(&args, &device, video.size())?;
            win.set_remote_input(true);
            let feed = crate::overlay::Feed::default();
            video.set_overlay(feed.clone());
            let mut memory = args.view_memory.as_deref().map(crate::memory::Memory::open);
            if let Some(v) = args.view {
                let mode = match v {
                    1 => Mode::All,
                    2 => Mode::One(0),
                    _ => Mode::Windows,
                };
                video.set_start_mode(None, mode);
            } else if let Some(v) = memory.as_ref().and_then(|m| m.remembered.as_ref()) {
                video.set_start_mode(Some(v.monitors.clone()), v.mode);
            }
            if args.status_json {
                crate::ui_api::start(win.hwnd(), feed, files.views_handle(), files.commands());
            }
            if args.overlay {
                crate::overlay::toggle();
            }
            win.capture_shortcuts(args.shortcuts == Shortcuts::Host)?;
            if let Some(log) = &args.test_shortcuts {
                let harmless_only = args.shortcuts == Shortcuts::Local;
                crate::shortcuts::test_shortcuts(win.hwnd(), log.clone(), harmless_only);
            }
            println!(
                "input goes to the host; Ctrl+Alt+Shift+Del: Ctrl+Alt+Del on the host, Ctrl+Alt+Shift+F: borderless on/off, Ctrl+Alt+Shift+O: the overlay on/off, Ctrl+Alt+Shift+Q closes the client"
            );
            if args.test_move_loop {
                window::test_move_loop(&win);
            }
            let mut cursor = args.check_cursor.then_some(crate::live::CursorCheck {
                every: args.verify_every,
                checks: 0,
                wrong: 0,
            });
            let frames = crate::live::run(
                &mut win,
                &presenter,
                &ctx,
                &mut video,
                args.frames,
                cursor.as_mut(),
                memory.as_mut(),
            );
            // The session is over: the process ends now, whatever hangs on the way.
            exit_guard();
            let frames = frames?;
            println!("frames presented: {frames}");
            if let Some(c) = cursor {
                println!("cursor: {} checks, {} wrong", c.checks, c.wrong);
            }
            video.report();
            Ok(())
        }
        Some(path) => {
            let mut video = VideoFile::open(&device, path)?;
            let size = video.size()?;
            println!("playing {path:?}: {} access units", video.len());
            show_local(&args, &device, &ctx, Box::new(video), size, true)
        }
        None => {
            let size = parse_size(&args.size)?;
            let renderer: Box<dyn Renderer> = if args.test_pattern {
                Box::new(TestPattern::new(&device, size.0, size.1)?)
            } else {
                Box::new(Clear)
            };
            show_local(&args, &device, &ctx, renderer, size, args.test_pattern)
        }
    }
}

/// `--clip-copy` and `--clip-copy-image`: the copies they stand for.
fn clip_copies(args: &Args) -> Result<Vec<desktop::clipboard::Content>> {
    use desktop::clipboard::{Content, ImageFormat};
    let mut copies = Vec::new();
    if !args.clip_copy.is_empty() {
        copies.push(Content::Files(args.clip_copy.clone()));
    }
    if let Some(p) = &args.clip_copy_image {
        let png = std::fs::read(p).with_context(|| format!("{}", p.display()))?;
        copies.push(Content::Image(vec![(ImageFormat::Png, png)]));
    }
    Ok(copies)
}

/// This client's key (made the first time).
fn client_key(args: &Args) -> Result<transport::StaticKey> {
    let key_file = args
        .key
        .clone()
        .unwrap_or_else(|| transport::default_dir().join("client.key"));
    let (key, created) = transport::StaticKey::load_or_create(&key_file)?;
    if created {
        println!("created client key {}", key_file.display());
    }
    Ok(key)
}

/// `--test-cancel-files`: this many seconds in, every running transfer is
/// cancelled here.
fn test_cancel_files(secs: f64, files: &desktop::files::FileService) {
    let (views, commands) = (files.views_handle(), files.commands());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(secs));
        let running: Vec<_> = views
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v.stage == desktop::files::Stage::Running)
            .map(|v| (v.outgoing, v.transfer, v.name.clone(), v.done))
            .collect();
        for (outgoing, transfer, name, done) in running {
            println!("files: test: cancelling #{transfer} {name} at {done} bytes");
            let _ = commands.send(desktop::files::Cmd::Cancel { outgoing, transfer });
        }
    });
}

/// What the sessions' network threads need of the files thread.
fn files_hook(args: &Args, files: &desktop::files::FileService) -> Result<net::FilesHook> {
    let downloads = match &args.downloads {
        Some(d) => d.clone(),
        None => desktop::files::downloads(None)?,
    };
    if let Some(secs) = args.test_cancel_files {
        test_cancel_files(secs, files);
    }
    Ok(net::FilesHook {
        commands: files.commands(),
        temp: match &args.downloads {
            Some(d) => d.join("clip-temp"),
            None => std::env::temp_dir(),
        },
        folder: downloads,
        cap_mbps: args.file_mbps,
        policy: std::sync::Arc::new(std::sync::Mutex::new(signal::FilePolicy::default())),
        send_first: std::sync::Arc::new(std::sync::Mutex::new(args.send_file.clone())),
    })
}

/// A session's options from the command line (the host's key and the psk
/// come with each session from the server).
fn session_options(
    args: &Args,
    key: transport::StaticKey,
    audio: Option<crate::audio::AudioTx>,
    files: net::FilesHook,
) -> Result<net::Options> {
    Ok(net::Options {
        key,
        host_key: transport::PublicKey([0; 32]),
        psk: [0; 32],
        fps: args.fps,
        keep_unlocked: args.keep_unlocked,
        sim_loss: args.sim_loss / 100.0,
        sim_bandwidth_bps: (args.sim_bandwidth > 0.0).then_some((args.sim_bandwidth * 1e6) as u64),
        sim_delay: Duration::from_millis(args.sim_delay),
        sim_stall: args.sim_stall,
        packet_log: args.packet_log.clone(),
        show_monitors: args.show_monitors,
        dump_streams: args.dump_streams.clone(),
        pacing: args.pace,
        fec: args.fec,
        clipboard: crate::clipboard::ClipOptions {
            mode: args.clipboard,
            log: args.clipboard_log.clone(),
            script: args.clipboard_script,
            copies: clip_copies(args)?,
            paste_after: args.test_paste.clone(),
            host_paste_after: args.test_host_paste.clone(),
        },
        audio,
        files: Some(files),
    })
}

/// Signs up on the server (or waits for the approval), opens the first
/// session with the host, and keeps what reconnecting needs.
/// The host's token: from stdin with --token-stdin, otherwise --token.
fn host_token(args: &Args) -> Result<transport::HostToken> {
    if args.token_stdin {
        return read_token(std::io::stdin().lock());
    }
    args.token
        .context("--server needs --token: the host's token (shown by the host)")
}

/// The token on the first line of `from`.
fn read_token(mut from: impl std::io::BufRead) -> Result<transport::HostToken> {
    let mut line = String::new();
    from.read_line(&mut line)
        .context("reading the host's token from stdin")?;
    line.parse()
        .map_err(|e: String| anyhow!("the host's token from stdin: {e}"))
}

fn connect(
    args: &Args,
    device: &ID3D11Device,
    files: &desktop::files::FileService,
) -> Result<NetworkVideo> {
    let quality = signal::Quality {
        fps: args.fps,
        mbps: args.max_mbps,
    };
    if !quality.is_valid() {
        bail!("--fps must be 30 or 60, --max-mbps 0 (auto) .. 1000");
    }
    let server = args.server.clone().context("--server is needed")?;
    let token = host_token(args)?;
    // The hub's commands from now on: its answer to a host without monitors
    // comes before the window.
    if args.status_json {
        crate::ui_api::listen();
    }
    if let Some(p) = args.monitor_choice {
        crate::monitor_pick::set(p);
    }
    let key = client_key(args)?;
    let cfg = signal::ServerConfig {
        server: server.clone(),
        trust: signal::Trust::choose(args.server_ca.as_deref(), args.server_pin.as_deref())?,
    };
    let name = args
        .name
        .clone()
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "client".into());
    let creds = sign_up_and_wait(
        &cfg,
        &key,
        &name,
        token.lookup(),
        &credentials_path(&args.creds),
    )?;
    let player =
        crate::audio::Player::start(args.audio, args.audio_dump.clone(), args.sim_audio_drift);
    let opts = session_options(
        args,
        key,
        player.as_ref().map(crate::audio::Player::sender),
        files_hook(args, files)?,
    )?;
    // The player and the files live with the sessions' opener: through every
    // reconnection.
    let mut open: OpenSession = Box::new(move || {
        let _ = &player;
        open_via_server(&cfg, &creds, &token, quality, &opts)
    });
    crate::status::connecting();
    let (conn, session) = first_session(&mut open)?;
    crate::status::streaming(&conn);
    let f = conn.format;
    println!(
        "host via {server}: {}x{} @ {}/{} fps, {:?} {:?} {} bit",
        f.width, f.height, f.fps_num, f.fps_den, f.codec, f.chroma, f.bit_depth
    );
    let script = (args.input_script > 0)
        .then(|| InputScript::new(&args.input_script_log, args.input_script, f.width, f.height))
        .transpose()?;
    NetworkVideo::new(
        device,
        conn,
        args.barcode_every,
        script,
        args.frame_hashes.as_deref(),
        args.frame_log.as_deref(),
        Some(Reconnect::new(open, session, is_permanent)),
    )
}

/// The video window for a picture of `size`, and its swapchain.
fn open_window(
    args: &Args,
    device: &ID3D11Device,
    size: (u32, u32),
) -> Result<(Window, Presenter)> {
    let win = Window::new(&args.title, size.0, size.1, args.borderless)?;
    let (cw, ch) = win.client_size();
    println!("client size: {cw}x{ch}");
    let presenter = Presenter::new(device, win.hwnd(), window::current_layout())?;
    println!("swapchain: {}", presenter.describe()?);
    Ok((win, presenter))
}

/// A file or a test picture, frame by frame at the swapchain's pace; with
/// `verify`, the back buffer is compared with the CPU reference.
fn show_local(
    args: &Args,
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    mut renderer: Box<dyn Renderer>,
    size: (u32, u32),
    verify: bool,
) -> Result<()> {
    let (mut win, presenter) = open_window(args, device, size)?;
    if args.test_move_loop {
        window::test_move_loop(&win);
    }
    let mut intervals: Vec<f64> = Vec::new();
    let mut last_present: Option<Instant> = None;
    let mut frame: u64 = 0;
    let mut v = Verification::default();

    while win.pump(|_, _, _| {}) {
        if args.frames.is_some_and(|n| frame >= n) {
            break;
        }
        let layout = window::current_layout();
        if layout != presenter.layout() {
            presenter.set_layout(layout)?;
        }
        presenter.wait_for_frame();
        let rtv = presenter.render_target()?;
        if !renderer.draw(ctx, &rtv, frame)? {
            break;
        }
        let is_last = args.frames.is_some_and(|n| frame + 1 == n);
        let due = frame == 0
            || is_last
            || (args.verify_every > 0 && frame.is_multiple_of(args.verify_every));
        if verify
            && due
            && let Some(exp) = renderer.expected(ctx)?
        {
            v.check(frame, &presenter.read_back(ctx)?, &exp);
        }
        drop(rtv);
        presenter.present(ctx, args.sync)?;
        let now = Instant::now();
        if let Some(prev) = last_present {
            intervals.push(now.duration_since(prev).as_secs_f64() * 1000.0);
        }
        last_present = Some(now);
        frame += 1;
    }

    println!("frames presented: {frame}");
    if v.frames > 0 {
        println!(
            "verified {} frames, {} px: {} beyond tolerance, {} off by 1 ({:.4} %)",
            v.frames,
            v.pixels,
            v.beyond,
            v.off_by_one,
            v.off_by_one as f64 * 100.0 / v.pixels as f64
        );
    }
    intervals.sort_by(|a, b| a.total_cmp(b));
    if !intervals.is_empty() {
        println!(
            "present interval ms: p50 {:.3}  p99 {:.3}  max {:.3}",
            transport::percentile(&intervals, 0.5),
            transport::percentile(&intervals, 0.99),
            intervals.last().copied().unwrap_or(0.0)
        );
    }
    if v.beyond != 0 {
        bail!("back buffer differs from the reference");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read_token;

    #[test]
    fn the_token_is_read_from_the_first_line() {
        let token = transport::HostToken::generate();
        let got = read_token(format!("{token}\r\nthe rest\n").as_bytes()).unwrap();
        assert!(got == token);
    }

    #[test]
    fn no_token_on_stdin_is_an_error() {
        assert!(read_token(&b""[..]).is_err());
        assert!(read_token(&b"\n"[..]).is_err());
        assert!(read_token(&b"not a token\n"[..]).is_err());
    }
}
