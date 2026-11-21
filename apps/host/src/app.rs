//! Command line and entry point.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::files::load_or_create_token;

/// UDP port clients connect to, unless --port says otherwise.
pub(crate) const DEFAULT_PORT: u16 = 9000;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum InputArg {
    /// Inject unless the client is on this machine (it would click into its own window).
    Auto,
    DryRun,
    Inject,
    Off,
}

/// The clipboard (text) shared with the client.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ClipboardArg {
    /// Shared, unless the client is on this machine (one clipboard: a dry run).
    Auto,
    On,
    /// Texts only logged (--clipboard-log), the clipboard untouched.
    DryRun,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RcArg {
    /// QP never below --qp, bitrate never above the cap (--max-mbps): the streaming mode.
    Capped,
    /// Constant QP --qp, no cap (quality studies).
    Cqp,
}

/// `--static-qp`: the still regions' QP, or none (no map).
#[derive(Clone, Copy, Debug)]
pub(crate) struct StaticQp(pub(crate) Option<u32>);

fn parse_static_qp(s: &str) -> Result<StaticQp, String> {
    if s.eq_ignore_ascii_case("off") {
        return Ok(StaticQp(None));
    }
    match s.parse::<u32>() {
        Ok(q) if q <= 51 => Ok(StaticQp(Some(q))),
        _ => Err(format!("expected a QP 0..=51 or \"off\", got {s:?}")),
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SceneArg {
    Moving,
    Still,
    Mixed,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SourceArg {
    Desktop,
    Synthetic,
}

#[derive(Parser)]
#[command(
    about = "Lumen host: stream a monitor or a synthetic source to a client",
    after_help = "Defaults for streaming come from host.ini in the data folder. The host registers \
                  on --server by itself; give its token (`lumen-host token`) to the clients that may connect.",
    args_conflicts_with_subcommands = true,
    args_override_self = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: Args,
}

#[derive(Subcommand)]
enum Displays {
    /// The driver and its monitors.
    Info,
    /// The desktop's monitors: place, size, scale (no rights needed).
    List,
    /// Monitors from a settings file (JSON, as Lumen Host keeps them) up and
    /// arranged, until Enter.
    Up {
        settings: PathBuf,
        /// A few seconds later, change them to this file's.
        #[arg(long)]
        then: Option<PathBuf>,
    },
    /// Ask the service (as Lumen Host does): its virtual monitors; with
    /// --set, monitors from a settings file; with --reset, none and no settings.
    Service {
        #[arg(long)]
        set: Option<PathBuf>,
        #[arg(long)]
        reset: bool,
    },
    /// Place and/or scale of the virtual monitors on this session's desktop
    /// (the service runs it in the console session).
    Arrange {
        /// JSON from the service: the monitors up and their settings.
        #[arg(long)]
        plan: String,
        #[arg(long, value_enum)]
        part: crate::monitors::Part,
    },
    /// Probe for a host without monitors (administrator, free screen: the
    /// physical monitor goes off for a while and comes back).
    Probe {
        #[arg(long, default_value = r"bench-out\headless\probe.log")]
        log: PathBuf,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Print this host's token (give it to the clients that may connect),
    /// creating it on first use.
    Token {
        /// Make a new one: clients with the old token can no longer connect
        /// (approved clients stay approved once they enter the new one).
        #[arg(long)]
        new: bool,
    },
    /// List monitors: the `adapter` / `output` values for host.ini.
    Outputs,
    /// Virtual monitors of Lumen VDD, by hand (the driver
    /// opens only for administrators and SYSTEM).
    Displays {
        #[command(subcommand)]
        what: Displays,
    },
    /// Sound: the output devices, and what the default one plays, captured
    /// for a while (WASAPI loopback).
    AudioProbe {
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
        /// Write what was captured (float WAV; gaps in the stream as silence).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Check a test tone of this frequency, Hz, in what was captured.
        #[arg(long)]
        tone: Option<f64>,
        /// Only write the test tone (`--tone`, 3 s) to this WAV and exit.
        #[arg(long)]
        write_tone: Option<PathBuf>,
        /// Instead of capturing: decode a `--dump-audio` file (to `--out`,
        /// checked with `--tone`).
        #[arg(long)]
        decode: Option<PathBuf>,
        /// Instead of capturing: check the tone (`--tone`) in this float WAV
        /// (e.g. lumen-client --audio-dump).
        #[arg(long)]
        wav: Option<PathBuf>,
    },
    /// Files on the clipboard with delayed rendering (probe): logs who asks for them and answers as told.
    ClipProbe {
        #[arg(long, default_value_t = 120.0)]
        seconds: f64,
        /// What a paste gets: empty (a list of no files), null (nothing), or
        /// files (the paths in --file).
        #[arg(long, default_value = "empty")]
        render: String,
        #[arg(long)]
        file: Vec<PathBuf>,
        /// Answer after this many ms.
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        /// Without the formats that keep Windows' clipboard history away.
        #[arg(long)]
        no_exclude: bool,
        /// Let lower integrity programs ask (UIPI).
        #[arg(long)]
        allow_uipi: bool,
        /// Put the files on the clipboard again after each paste.
        #[arg(long)]
        again: bool,
        /// Instead: as SYSTEM, read the files copied in Explorer as the
        /// console user, write them to their Downloads\Lumen and %TEMP%, and
        /// put the %TEMP% copies on the clipboard.
        #[arg(long)]
        system: bool,
        /// Instead: the clipboard's formats and files, before and after
        /// OleInitialize.
        #[arg(long)]
        formats: bool,
        /// Instead: the clipboard helper (clip-agent) started as this user,
        /// a text through it and files back.
        #[arg(long)]
        agent: bool,
        /// Instead (as SYSTEM): the helper started as the signed-in user, as
        /// the service's host does; 30 s of copies seen through it.
        #[arg(long)]
        agent_system: bool,
    },
    /// The clipboard helper the service's host runs as the signed-in user
    /// (pipes `<base>-up` and `<base>-down`).
    #[command(hide = true)]
    ClipAgent {
        #[arg(long)]
        pipe: String,
    },
    /// The Windows service "Lumen Host" (run as administrator): it runs the
    /// machine's host (%ProgramData%\Lumen\host) as SYSTEM in the console session.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// The machine's registration in the service (what Lumen Host does).
    Registration {
        #[command(subcommand)]
        action: RegistrationAction,
    },
}

#[derive(Subcommand)]
enum RegistrationAction {
    /// Settings, token, the host's state and the end of its log.
    Show,
    /// Create or change the registration; the service (re)starts its host.
    Set {
        #[arg(long)]
        server: String,
        /// The server's CA certificate file (development servers).
        #[arg(long)]
        server_ca: Option<PathBuf>,
        /// The key's fingerprint of a server with its own certificate (a
        /// local network), as `lumen-server self-cert` printed it.
        #[arg(long, default_value = "")]
        server_pin: String,
        #[arg(long, default_value = "")]
        machine: String,
        /// Announce these `ip:port`, comma separated.
        #[arg(long, default_value = "")]
        addrs: String,
    },
    /// A new token (clients with the old one can no longer connect).
    NewToken,
    /// Remove the machine's registration, with its token.
    Remove,
    /// Remove the registration here and on the server; the token stays.
    Unregister,
    /// Forget the account on the server (the admin removed it): register anew.
    Reregister,
}

#[derive(Subcommand)]
enum ServiceAction {
    /// Install (or point at this program) and start the service.
    Install,
    /// Stop and remove the service.
    Uninstall,
    /// What the Service Control Manager runs.
    Run,
}

#[derive(clap::Args)]
pub(crate) struct Args {
    /// UDP port clients connect to (all interfaces); if taken, the next free
    /// one up to +9 (two Windows users streaming on one machine).
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub(crate) port: u16,
    /// Announce these `ip:port` instead of this machine's addresses (when the
    /// client reaches the host through a mapping).
    #[arg(long)]
    pub(crate) addr: Vec<SocketAddr>,
    /// What to stream: the monitor (--adapter / --output) at its own resolution,
    /// or a synthetic test picture (--size).
    #[arg(long, value_enum, default_value_t = SourceArg::Desktop)]
    pub(crate) source: SourceArg,
    /// Synthetic source size, WxH.
    #[arg(long, default_value = "3840x2160")]
    pub(crate) size: String,
    /// What the synthetic source shows (still / mixed: frame 100, mixed with a
    /// moving quarter window).
    #[arg(long, value_enum, default_value_t = SceneArg::Moving)]
    pub(crate) scene: SceneArg,
    /// Desktop source: adapter / output index (`lumen-host outputs`). Not
    /// given: the primary monitor (virtual monitors of Lumen VDD shift the
    /// output numbers).
    #[arg(long)]
    pub(crate) adapter: Option<u32>,
    #[arg(long)]
    pub(crate) output: Option<u32>,
    /// Frame rate when the server does not say.
    #[arg(long, default_value_t = 60)]
    pub(crate) fps: u32,
    #[arg(long, value_enum, default_value_t = RcArg::Capped)]
    pub(crate) rc: RcArg,
    /// Bitrate cap for --rc capped, Mbit/s, over what the session says (tests);
    /// 0 = the session's cap (the client's choice, the admin's, or the server's table).
    #[arg(long, default_value_t = 0.0)]
    pub(crate) max_mbps: f64,
    /// --rc cqp: the QP; --rc capped: the finest QP allowed.
    #[arg(long, default_value_t = 16)]
    pub(crate) qp: u32,
    /// Still regions are refined down to this QP while moving ones stay at
    /// --qp (a QP map from the changed regions);
    /// "off": one QP floor for the whole picture, as before 0.6.4.
    #[arg(long, default_value = "10", value_parser = parse_static_qp)]
    pub(crate) static_qp: StaticQp,
    /// NVENC preset 1..=7. P1: 1.3-1.5 ms faster than P4 at 4K for dE00 mean +0.007
    ///; chosen by the owner.
    #[arg(long, default_value_t = 1)]
    pub(crate) preset: u32,
    /// FEC parity, percent of data shards, until the client asks for its own
    /// (it picks it from the losses it sees).
    #[arg(long, default_value_t = 15.0)]
    pub(crate) parity: f64,
    /// Pacing rate, Mbit/s: a frame leaves no faster than this, until the
    /// client asks for its own (it lowers it below a slower link by itself).
    #[arg(long, default_value_t = 150.0)]
    pub(crate) pace_mbps: f64,
    /// Checks: the monitors' datagrams in one queue, frame after frame,
    /// instead of taking turns.
    #[arg(long)]
    pub(crate) no_interleave: bool,
    /// Stop after this many seconds of streaming and exit after that one
    /// session (0 = serve clients one after another).
    #[arg(long, default_value_t = 0.0)]
    pub(crate) seconds: f64,
    /// Signaling server (host:port): the host registers there by itself and
    /// waits for the sessions it announces.
    #[arg(long)]
    pub(crate) server: Option<String>,
    /// CA of the server's certificate (development); default: the system trust store.
    #[arg(long)]
    pub(crate) server_ca: Option<PathBuf>,
    /// Trust only the server's key with this fingerprint (a server with its
    /// own certificate, `signal::pin`).
    #[arg(long)]
    pub(crate) server_pin: Option<String>,
    /// This machine's name as clients see it (default: the computer name).
    #[arg(long)]
    pub(crate) machine: Option<String>,
    /// The Windows account it streams (default: the current user; none when
    /// run by the service: the host is the machine's).
    #[arg(long)]
    pub(crate) account: Option<String>,
    /// What to do with the client's mouse and keyboard.
    #[arg(long, value_enum, default_value_t = InputArg::Auto)]
    pub(crate) input: InputArg,
    /// Write the encoded stream (Annex B) here, for offline checks.
    #[arg(long)]
    pub(crate) dump_stream: Option<PathBuf>,
    /// Write every applied input event here (compare with lumen-client --input-script-log).
    #[arg(long)]
    pub(crate) input_log: Option<PathBuf>,
    /// What to do with the clipboard.
    #[arg(long, value_enum, default_value_t = ClipboardArg::Auto)]
    pub(crate) clipboard: ClipboardArg,
    /// The sound sent to the client.
    #[arg(long, value_enum, default_value_t = crate::audio::AudioArg::Auto)]
    pub(crate) audio: crate::audio::AudioArg,
    /// Write the Opus frames sent (`lumen-host audio-probe --decode` reads them).
    #[arg(long)]
    pub(crate) dump_audio: Option<PathBuf>,
    /// Checks: `--audio tone` as from a device at this rate (resampled to 48 kHz).
    #[arg(long, default_value_t = 48_000, hide = true)]
    pub(crate) audio_tone_rate: u32,
    /// Checks: `--audio tone` 2 s on, 2 s off (nothing comes, as loopback
    /// while nothing plays).
    #[arg(long, hide = true)]
    pub(crate) audio_tone_gaps: bool,
    /// Write "R|S <bytes> <hash>" per clipboard text received / sent (checks).
    #[arg(long)]
    pub(crate) clipboard_log: Option<PathBuf>,
    /// Checks: send this many generated texts to the client instead of the
    /// clipboard's (protocol::clipboard::script_text).
    #[arg(long, default_value_t = 0)]
    pub(crate) clipboard_script: u32,
    /// Checks: send these files (or folders) to the first client that
    /// connects, as if copied and pasted there.
    #[arg(long)]
    pub(crate) send_file: Vec<PathBuf>,
    /// Checks: as if these files (or folders) were copied here once a
    /// client connected.
    #[arg(long, hide = true)]
    pub(crate) clip_copy: Vec<PathBuf>,
    /// Checks: as if this PNG picture was copied here once a client connected.
    #[arg(long, hide = true)]
    pub(crate) clip_copy_image: Option<PathBuf>,
    /// Received files go here instead of the user's Downloads\Lumen (checks).
    #[arg(long)]
    pub(crate) downloads: Option<PathBuf>,
    /// Checks: file transfers as on a locked Windows (no transfers).
    #[arg(long, hide = true)]
    pub(crate) test_locked: bool,
    /// The most our file data may take, Mbit/s, over the server's (checks).
    #[arg(long)]
    pub(crate) file_mbps: Option<f64>,
    /// This host's key file (default: host.key in the data folder; created on first use).
    #[arg(long)]
    pub(crate) key: Option<PathBuf>,
    /// Report state changes as `@status {json}` lines on stdout (for the Lumen Host app).
    #[arg(long)]
    pub(crate) status_json: bool,
    /// Run by the service for the machine's registration:
    /// on screen whoever is signed in, the console locked after a remote
    /// session, and host.ini is ignored (the process is SYSTEM's).
    #[arg(long)]
    pub(crate) service: bool,
}

/// `--clip-copy` and `--clip-copy-image`: the copies they stand for.
pub(crate) fn clip_copies(args: &Args) -> Result<Vec<desktop::clipboard::Content>> {
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

/// With --status-json: one `@status {...}` line on stdout per change, for the
/// Lumen Host app (apps/host-app) that runs this process.
static STATUS_JSON: AtomicBool = AtomicBool::new(false);

pub(crate) fn status(v: serde_json::Value) {
    if STATUS_JSON.load(Ordering::Relaxed) {
        println!("@status {v}");
    }
}

pub fn main() -> Result<()> {
    let mut cli = Cli::parse();
    if cli.command.is_none() && !cli.run.service {
        // Streaming: the config file supplies defaults, the command line wins.
        let config = transport::config_args(&transport::default_dir().join("host.ini"))?;
        if !config.is_empty() {
            cli = Cli::parse_from(transport::with_config_args(
                std::env::args_os().collect(),
                config,
            ));
        }
    }
    match cli.command {
        Some(Command::Token { new }) => {
            let token = load_or_create_token(new)?;
            println!("{token}");
            if new {
                eprintln!(
                    "a new token: clients must enter it; a running lumen-host picks it up when restarted"
                );
            }
            Ok(())
        }
        Some(Command::Outputs) => {
            // Real pixel sizes, not DPI-scaled ones.
            capture::init_process();
            for o in capture::list_outputs()? {
                println!(
                    "adapter={} output={}   {}x{}   {}   [{}]",
                    o.adapter_index,
                    o.output_index,
                    o.width,
                    o.height,
                    o.device_name,
                    o.adapter_name
                );
            }
            Ok(())
        }
        Some(Command::Displays { what }) => match what {
            Displays::Info => crate::monitors::print_info(),
            Displays::List => crate::monitors::print_desktop(),
            Displays::Up { settings, then } => crate::monitors::hold(&settings, then.as_deref()),
            Displays::Arrange { plan, part } => crate::monitors::arrange_plan(&plan, part),
            Displays::Service { set, reset } => crate::monitors::ask_service(set.as_deref(), reset),
            Displays::Probe { log } => crate::headless_probe::run(&log),
        },
        Some(Command::AudioProbe {
            seconds,
            out,
            tone,
            write_tone,
            decode,
            wav,
        }) => match (write_tone, decode, wav) {
            (Some(path), ..) => crate::audio_probe::write_tone(&path, tone.unwrap_or(1000.0), 3.0),
            (None, Some(dump), _) => crate::audio_probe::decode(&dump, out.as_deref(), tone),
            (None, None, Some(w)) => crate::audio_probe::check_wav(&w, tone.unwrap_or(1000.0)),
            (None, None, None) => crate::audio_probe::run(seconds, out.as_deref(), tone),
        },
        Some(Command::ClipProbe {
            seconds,
            render,
            file,
            delay_ms,
            no_exclude,
            allow_uipi,
            again,
            system,
            formats,
            agent,
            agent_system,
        }) => {
            if agent_system {
                return crate::clip_agent::probe_system();
            }
            if agent {
                return crate::clip_agent::probe();
            }
            if formats {
                crate::user_files::formats_probe();
                return Ok(());
            }
            if system {
                return crate::user_files::system_probe(file);
            }
            use desktop::clip_probe::{Options, Render};
            let render = match render.as_str() {
                "empty" => Render::Empty,
                "null" => Render::Null,
                "files" if !file.is_empty() => Render::Files(file),
                _ => anyhow::bail!("--render empty|null|files (files with --file)"),
            };
            desktop::clip_probe::run(Options {
                seconds,
                render,
                delay: std::time::Duration::from_millis(delay_ms),
                exclude: !no_exclude,
                allow_uipi,
                again,
            })
            .map_err(anyhow::Error::msg)
        }
        Some(Command::ClipAgent { pipe }) => crate::clip_agent::serve(&pipe),
        Some(Command::Registration { action }) => registration(action),
        Some(Command::Service { action }) => match action {
            ServiceAction::Install => crate::service::install(),
            ServiceAction::Uninstall => crate::service::uninstall(),
            ServiceAction::Run => crate::service::run(),
        },
        None => {
            STATUS_JSON.store(cli.run.status_json, Ordering::Relaxed);
            crate::station::run(cli.run)
        }
    }
}

fn registration(action: RegistrationAction) -> Result<()> {
    use desktop::hostsvc::{Request, Settings, call};
    let ca = |p: &Option<PathBuf>| -> Result<String> {
        Ok(match p {
            Some(p) => {
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?
            }
            None => String::new(),
        })
    };
    let req = match action {
        RegistrationAction::Show => Request::State,
        RegistrationAction::Set {
            server,
            server_ca,
            server_pin,
            machine,
            addrs,
        } => Request::Set {
            settings: Settings {
                server,
                server_ca_pem: ca(&server_ca)?,
                server_fingerprint: server_pin,
                machine,
                manual_addrs: addrs,
            },
        },
        RegistrationAction::NewToken => Request::NewToken,
        RegistrationAction::Remove => Request::Remove,
        RegistrationAction::Unregister => Request::Unregister,
        RegistrationAction::Reregister => Request::Reregister,
    };
    let st = call(&req).map_err(anyhow::Error::msg)?;
    if !st.note.is_empty() {
        println!("{}", st.note);
    }
    if !st.registered {
        println!("no registration on this machine");
        return Ok(());
    }
    println!("server: {}", st.settings.server);
    println!("token: {}", st.token);
    let h = &st.status;
    println!(
        "host: {}{}",
        h.state,
        if h.detail.is_empty() {
            String::new()
        } else {
            format!(": {}", h.detail)
        }
    );
    for line in crate::registry::log_tail()
        .lines()
        .rev()
        .take(10)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        println!("  {line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn static_qp_takes_a_qp_or_off() {
        assert_eq!(parse_static_qp("10").unwrap().0, Some(10));
        assert_eq!(parse_static_qp("off").unwrap().0, None);
        assert_eq!(parse_static_qp("OFF").unwrap().0, None);
        assert!(parse_static_qp("52").is_err());
        assert!(parse_static_qp("-1").is_err());
        assert!(parse_static_qp("").is_err());
    }

    #[test]
    fn static_qp_is_on_by_default() {
        let cli = Cli::try_parse_from(["lumen-host"]).unwrap();
        assert_eq!(cli.run.static_qp.0, Some(10));
        let cli = Cli::try_parse_from(["lumen-host", "--static-qp", "off"]).unwrap();
        assert_eq!(cli.run.static_qp.0, None);
    }
}
