//! What builds a session: the handshake answered, the control thread, input,
//! clipboard, files, sound, the sender and the streams.

use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use protocol::{ControlMessage, handshake_datagram};
use signal::{HostAgent, IncomingSession};
use transport::{
    FecConfig, Opener, PacedSender, PacerConfig, Packetizer, PublicKey, SeqCounter, UdpLink,
    wall_clock_us,
};

use super::Session;
use super::monitors::Monitors;
use super::pacing::{PACE_BURST, Pace};
use super::rate::{Ceiling, rate_control, session_fps};
use crate::app::{Args, ClipboardArg, InputArg};
use crate::audio::{AudioArg, AudioThread};
use crate::clipboard::HostClipboard;
use crate::control::{ControlThread, Event, SharedSealer, seal};
use crate::cursor::CursorFeed;
use crate::remote_input::{RemoteInput, Sink};
use crate::station::{Admitted, Station};
use crate::streamer::{SessionRate, Shared};

/// The client's handshake answered: the session's id and keys, and what the
/// client said about itself.
struct Answered {
    id: u32,
    client_key: PublicKey,
    caps: protocol::Capabilities,
    sealer: SharedSealer,
    /// What the control thread needs to answer a repeated handshake and to
    /// open the client's datagrams.
    control: ControlKeys,
}

struct ControlKeys {
    psk: [u8; 32],
    last_hello: Vec<u8>,
    last_reply: Vec<u8>,
    opener: Opener,
}

/// Answers the client's handshake (message 2 sent on `link`).
fn answer(adm: Admitted, link: &UdpLink) -> Result<Answered> {
    let Admitted {
        responder,
        hello: last_hello,
        peer,
        psk,
        ..
    } = adm;
    let Ok(ControlMessage::Hello { caps, .. }) = ControlMessage::decode(responder.payload()) else {
        bail!("client {peer}: handshake without Hello");
    };
    let client_key = responder.remote();
    let id = (wall_clock_us() as u32) ^ std::process::id().rotate_left(16);
    let (reply, keys) = responder.finish(&psk)?;
    let last_reply = handshake_datagram(id, &reply);
    link.send(&last_reply)?;
    let (sealer, opener) = keys.split();
    Ok(Answered {
        id,
        client_key,
        caps,
        sealer: Arc::new(RwLock::new(sealer)),
        control: ControlKeys {
            psk,
            last_hello,
            last_reply,
            opener,
        },
    })
}

/// The control thread of a session: what the client sends (input, its
/// requests, files) comes in there.
struct Control {
    thread: JoinHandle<(RemoteInput, u64)>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    last_rx: Arc<AtomicU64>,
}

/// Starts the control thread on `link`, applying the client's input to `input`.
fn spawn_control(
    st: &Station,
    link: &UdpLink,
    (id, client_key, sealer): (u32, PublicKey, &SharedSealer),
    keys: ControlKeys,
    seq: &SeqCounter,
    input: RemoteInput,
    files: FileChannels,
) -> Result<Control> {
    let (ev_tx, events) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let last_rx = Arc::new(AtomicU64::new(wall_clock_us()));
    let control = ControlThread {
        link: link.try_clone()?,
        ack_link: link.try_clone()?,
        session: id,
        seq: seq.clone(),
        input,
        events: ev_tx,
        stop: stop.clone(),
        key: st.key.clone(),
        client_key,
        psk: keys.psk,
        opener: keys.opener,
        sealer: sealer.clone(),
        last_hello: keys.last_hello,
        last_reply: keys.last_reply,
        replied_at: Instant::now(),
        rejected: 0,
        last_rx: last_rx.clone(),
        files: st.files.commands(),
        file_msgs: files.msgs,
        file_events: files.events,
        file_out: transport::ReliableSender::new(),
    };
    Ok(Control {
        thread: thread::spawn(move || control.run()),
        events,
        stop,
        last_rx,
    })
}

/// Where the client's input goes: not into this machine's own windows when
/// the client is here (`local`).
fn input_sink(st: &Station, local: bool) -> Sink {
    let sink = match st.args.input {
        InputArg::Auto if local => Sink::DryRun,
        InputArg::Auto | InputArg::Inject => RemoteInput::injector(),
        InputArg::DryRun => Sink::DryRun,
        InputArg::Off => Sink::Off,
    };
    println!(
        "input: {}",
        match sink {
            Sink::DryRun => "dry run (logged, not injected)",
            Sink::Inject(_) => "injected with SendInput",
            Sink::Off => "off",
        }
    );
    sink
}

/// The session's clipboard (`local`: shared with the client already, a dry run).
fn host_clipboard(args: &Args, local: bool) -> Result<Option<HostClipboard>> {
    let real = match args.clipboard {
        ClipboardArg::Off => None,
        ClipboardArg::On => Some(true),
        ClipboardArg::Auto => Some(!local),
        ClipboardArg::DryRun => Some(false),
    };
    println!(
        "clipboard: {}",
        match real {
            Some(true) => "shared",
            Some(false) => "dry run (logged, the clipboard untouched)",
            None => "off",
        }
    );
    real.map(|real| HostClipboard::new(real, args.clipboard_log.as_deref(), args.clipboard_script))
        .transpose()
}

/// The session's sound (none when the client is here, unless asked), its
/// packets straight out through `sender`.
fn start_audio(
    args: &Args,
    local: bool,
    (session, sealer): (u32, &SharedSealer),
    seq: &SeqCounter,
    sender: &PacedSender,
) -> Result<Option<AudioThread>> {
    match (args.audio, local) {
        (AudioArg::Off, _) | (AudioArg::Auto, true) => {
            println!("audio: off");
            Ok(None)
        }
        (source, _) => crate::audio::start(
            source,
            crate::audio::Out {
                session,
                seq: seq.clone(),
                sealer: sealer.clone(),
                sender: sender.now_sender(),
            },
            args.dump_audio.as_deref(),
            args.audio_tone_rate,
            args.audio_tone_gaps,
        )
        .map(Some),
    }
}

/// What the files thread says, for the control thread to pass on.
struct FileChannels {
    msgs: Receiver<protocol::FileMsg>,
    events: Receiver<desktop::files::Event>,
}

impl Station {
    /// The files thread attached to this session: its data sealed out on
    /// `link`, its messages and clipboard events to the control thread.
    fn attach_files(
        &mut self,
        inc: &IncomingSession,
        a: &Answered,
        seq: &SeqCounter,
        link: UdpLink,
    ) -> FileChannels {
        let (msg_tx, msgs) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (id, seq, sealer) = (a.id, seq.clone(), a.sealer.clone());
        let args = &self.args;
        let (downloads, locked) = (args.downloads.clone(), args.test_locked);
        let _ = self.files.commands().send(desktop::files::Cmd::Attach {
            session: id,
            io: desktop::files::Io {
                data: Box::new(move |p| {
                    let mut d = p.datagram(id, seq.take());
                    seal(&sealer, &mut d);
                    let _ = link.send(&d);
                }),
                msg: Box::new(move |m| {
                    let _ = msg_tx.send(m);
                }),
                event: Box::new(move |e| {
                    let _ = event_tx.send(e);
                }),
            },
            peer: a.client_key.0.to_vec(),
            cap_bps: inc.files.cap_bps(args.file_mbps),
            place: Box::new(move || {
                if locked {
                    return desktop::files::Place::Locked;
                }
                crate::user_files::place(downloads.as_deref())
            }),
            allowed: inc.files.allowed,
        });
        FileChannels { msgs, events }
    }

    /// What `--clip-copy` gives: copied once, in the first session.
    fn first_copies(&self, input: &mut RemoteInput) -> Result<()> {
        if let Some(c) = input.clipboard.as_mut()
            && !self.files_sent
        {
            c.copies = crate::app::clip_copies(&self.args)?;
        }
        Ok(())
    }

    /// What `--send-file` gives: sent once, in the first session (attached).
    fn send_first_files(&mut self) {
        if !self.files_sent && !self.args.send_file.is_empty() {
            self.files
                .send(desktop::files::Cmd::Send(self.args.send_file.clone()));
        }
        self.files_sent = true;
    }
}

/// What applies the client's input: mouse and keys, clipboard, the first files.
fn start_input(st: &mut Station, local: bool) -> Result<RemoteInput> {
    let files = st.files.commands();
    let mut remote_input = RemoteInput::new(
        input_sink(st, local),
        st.args.input_log.as_deref(),
        host_clipboard(&st.args, local)?,
    )?;
    remote_input.files = Some(files.clone());
    if let Some(c) = remote_input.clipboard.as_mut() {
        c.files = Some(files.clone());
    }
    st.first_copies(&mut remote_input)?;
    Ok(remote_input)
}

/// The session's sender. Frames leave in the pacer's order (the monitors'
/// streams take turns): numbered and sealed then, the numbers on the wire stay
/// in order (the receiver's replay window drops datagrams far behind the newest).
fn paced_sender(
    link: UdpLink,
    (rate_bps, burst_bytes): (u64, usize),
    seq: &SeqCounter,
    sealer: &SharedSealer,
) -> PacedSender {
    let sender = PacedSender::new(
        link,
        PacerConfig {
            rate_bps,
            burst_bytes,
        },
    );
    let (seq, sealer) = (seq.clone(), sealer.clone());
    sender.seal_paced_with(Box::new(move |d: &mut Vec<u8>| {
        crate::streamer::renumber(d, seq.take());
        seal(&sealer, d);
    }));
    sender
}

/// `--dump-stream`: a file per monitor (monitor 1's at the path itself).
fn open_dumps(
    path: Option<&std::path::Path>,
    screens: usize,
) -> Result<Vec<Option<BufWriter<File>>>> {
    let Some(p) = path else {
        return Ok(Vec::new());
    };
    (0..screens)
        .map(|i| {
            let path = match i {
                0 => p.to_path_buf(),
                i => p.with_extension(i.to_string()),
            };
            let f = File::create(&path).with_context(|| format!("create {path:?}"))?;
            Ok(Some(BufWriter::new(f)))
        })
        .collect()
}

impl<'a> Session<'a> {
    /// Answers the client's handshake, sets up the control thread and a
    /// stream per monitor, and sends `Monitors` and the `StreamStart`s.
    pub(super) fn start(
        st: &'a mut Station,
        link: UdpLink,
        adm: Admitted,
        agent: &'a HostAgent,
        inc: &'a IncomingSession,
    ) -> Result<Self> {
        let (peer, local) = (adm.peer.clone(), adm.local);
        let a = answer(adm, &link)?;
        // Without a physical monitor the service puts up the client's own
        // (or it is to be asked) before the screens open.
        let asking = crate::headless::Asking::new(
            st.monitors
                .tell(Some(&inc.client_key), false)
                .is_some_and(|m| crate::headless::to_ask(&m)),
        );
        if st.screens.is_empty() || st.layout_changed() {
            st.reopen_screens()?;
        }
        for s in &mut st.screens {
            s.restart()?;
        }
        let fps = session_fps(&st.args, inc);
        let (rate_control, cap_control, cap_bps) = rate_control(st, inc, fps);
        println!(
            "client {peer}, session {:08x}: {fps} fps, HEVC Main10 {rate_control:?}, pacing {} Mbit/s",
            a.id, st.args.pace_mbps
        );
        let parity = st.args.parity / 100.0;
        let cursor_packetizer = Packetizer::new(
            FecConfig {
                parity_ratio: parity,
                ..FecConfig::default()
            },
            a.id,
        )?;
        let seq = cursor_packetizer.seq_counter();

        let remote_input = start_input(st, local)?;
        let file_channels = st.attach_files(inc, &a, &seq, link.try_clone()?);
        st.send_first_files();
        let control = spawn_control(
            st,
            &link,
            (a.id, a.client_key, &a.sealer),
            a.control,
            &seq,
            remote_input,
            file_channels,
        )?;

        let default_pacing = ((st.args.pace_mbps * 1e6) as u64, PACE_BURST);
        let sender = paced_sender(link, default_pacing, &seq, &a.sealer);
        let audio = start_audio(&st.args, local, (a.id, &a.sealer), &seq, &sender)?;
        let dumps = open_dumps(st.args.dump_stream.as_deref(), st.screens.len())?;
        let shared = Shared {
            session: a.id,
            seq,
            sealer: a.sealer,
            out: sender.paced_sender(),
            cursor: Arc::new(Mutex::new(CursorFeed::new())),
            rate: Arc::new(SessionRate::new(cap_control)),
            stop: Arc::new(AtomicBool::new(false)),
            stale: Arc::new(AtomicBool::new(false)),
        };
        let now = Instant::now();
        let deadline =
            (st.args.seconds > 0.0).then(|| now + Duration::from_secs_f64(st.args.seconds));
        let mut s = Session {
            agent,
            inc,
            id: a.id,
            events: control.events,
            control: Some(control.thread),
            stop: control.stop,
            last_rx: control.last_rx,
            streams: Vec::new(),
            caps: a.caps,
            shared,
            fps,
            ceiling: Ceiling {
                rate_control,
                cap_bps,
                files_sending: st.files.sending(),
                video_low: false,
            },
            pace: Pace {
                default_pacing,
                pacing: default_pacing,
                parity,
            },
            cursor_packetizer,
            audio,
            sender,
            dumps,
            win_start: now,
            deadline,
            monitors: Monitors {
                monitors_msg: ControlMessage::Bye,
                start_msgs: Vec::new(),
                monitors_acked: false,
                monitors_sent: now,
                shown: 0,
                stream_pixels: Vec::new(),
                next_frames: [0; protocol::MAX_STREAMS as usize],
                layout_checked: now,
                asking,
                choosing: None,
            },
            st,
        };
        if let Err(e) = s.start_streams(&a.caps) {
            s.finish(&Err(anyhow!("streams did not start")));
            return Err(e);
        }
        Ok(s)
    }
}
