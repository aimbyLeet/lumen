//! The network thread: datagrams from the host, frames to the decode queue,
//! input, requests and feedback to the host.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::{
    AudioPacket, ControlMessage, CursorShape, FilePacket, InputEvent, PacketHeader, PacketKind,
};
use transport::{
    Delivery, Opener, Reassembler, ReliableReceiver, ReliableSender, Sealer, SeqCounter, UdpLink,
    wall_clock_us,
};

use super::checks::{
    Checks, OtherStreams, Sim, TestPastes, emulator, log_datagram, packet_log, test_times,
};
use super::connection::{ToDecode, Wake};
use super::files::{Sharing, attach_files};
use super::recovery::{Ask, Recovery};
use super::{Counters, CursorEvent, HostMonitors, Options};
use crate::tune::LinkTuner;

const FRAME_TIMEOUT: Duration = Duration::from_millis(100);
const STATS_EVERY: Duration = Duration::from_secs(1);

/// Channels and flags the network thread shares with the render thread.
pub(super) struct NetShared {
    pub(super) out: Sender<ToDecode>,
    pub(super) inputs: Receiver<InputEvent>,
    pub(super) cursor: Sender<CursorEvent>,
    pub(super) requests: Receiver<ControlMessage>,
    pub(super) wake: Wake,
    pub(super) counters: Arc<Mutex<Counters>>,
    /// A bit per stream whose decoder failed (an IDR is asked for).
    pub(super) decode_failed: Arc<AtomicU8>,
    pub(super) stop: Arc<AtomicBool>,
    /// The host's monitors (`Monitors`), for the render thread.
    pub(super) monitors: HostMonitors,
    /// The monitors the window shows, a bit each (0: all).
    pub(super) shown: Arc<AtomicU8>,
}

pub(super) fn network(
    link: UdpLink,
    session: u32,
    shared: NetShared,
    opts: Options,
    sealer: Sealer,
    opener: Opener,
    seq: SeqCounter,
) {
    NetLoop::new(link, session, shared, opts, sealer, opener, seq).run();
}

/// The encrypted link to the host.
struct Wire {
    link: UdpLink,
    session: u32,
    sealer: Arc<Sealer>,
    opener: Opener,
    /// Our datagrams' sequence numbers (the nonce of their encryption),
    /// shared with the files thread.
    seq: SeqCounter,
}

/// Packet loss from gaps in the host's sequence numbers (one counter per session).
struct HostSeqs {
    lo: u64,
    hi: u64,
    seen: u64,
}

/// The host's monitors, and which of them the window shows.
struct MonitorState {
    /// The host's `Monitors` generation and how many monitors it has.
    monitors: Option<(u32, usize)>,
    /// The mask told to the host last.
    shown_told: Option<u8>,
    /// What the host says while it has no monitor.
    waiting: crate::monitor_pick::HostWaits,
}

/// A stream's slot in the per-stream arrays. The header parser refuses
/// `stream >= MAX_STREAMS`; the mask keeps a slip there from being a panic
/// here.
fn stream_index(stream: u8) -> usize {
    usize::from(stream) % protocol::MAX_STREAMS as usize
}

/// The network thread's state: reassembly, what the decoder waits for after
/// a loss, reliable input, what is asked of the host.
struct NetLoop {
    wire: Wire,
    shared: NetShared,
    /// Received (and through the emulator, if any), not handled yet.
    incoming: Vec<Vec<u8>>,
    input: ReliableSender<InputEvent>,
    r: Reassembler,
    deliveries: Vec<Delivery>,
    c: Counters,
    /// Each stream starts at a keyframe; after a loss, its frames wait for a recovery.
    rec: [Recovery; protocol::MAX_STREAMS as usize],
    /// The first ping goes out at once: the input resend timeout needs an RTT.
    last_stats: Option<Instant>,
    mon: MonitorState,
    ping_id: u32,
    host_seq: HostSeqs,
    tuner: LinkTuner,
    audio: Option<crate::audio::AudioTx>,
    share: Sharing,
    checks: Checks,
}

impl NetLoop {
    fn new(
        link: UdpLink,
        session: u32,
        shared: NetShared,
        opts: Options,
        sealer: Sealer,
        opener: Opener,
        seq: SeqCounter,
    ) -> Self {
        let sealer = Arc::new(sealer);
        let files = opts
            .files
            .as_ref()
            .map(|hook| attach_files(hook, &link, session, &seq, &sealer, &opts.host_key));
        let (files, file_msgs, file_events, file_policy) = match files {
            Some(f) => (
                Some(f.commands),
                Some(f.msgs),
                Some(f.events),
                opts.files.clone().map(|h| (h, f.policy)),
            ),
            None => (None, None, None, None),
        };
        let sim = Sim {
            loss: opts.sim_loss,
            rng: transport::Rng::new(0x9E37_79B9_7F4A_7C15),
            emulator: emulator(&opts),
        };
        let input = ReliableSender::new();
        let r = Reassembler::new(session, FRAME_TIMEOUT);
        let others = OtherStreams::new(opts.dump_streams.clone());
        let tuner = LinkTuner::new(opts.pacing, opts.fec);
        let packet_log = packet_log(opts.packet_log.as_deref());
        let clipboard = crate::clipboard::ClientClipboard::new(&opts.clipboard);
        let audio = opts.audio.inspect(|a| {
            // A new session: its frames are numbered from 0.
            let _ = a.send(crate::audio::AudioMsg::Reset);
        });
        let pastes = crate::shortcuts::pastes();
        let test_pastes = TestPastes {
            test_paste: test_times(&opts.clipboard.paste_after),
            test_host_paste: test_times(&opts.clipboard.host_paste_after),
        };
        NetLoop {
            wire: Wire {
                link,
                session,
                sealer,
                opener,
                seq,
            },
            shared,
            incoming: Vec::new(),
            input,
            r,
            deliveries: Vec::new(),
            c: Counters::default(),
            rec: Default::default(),
            last_stats: None,
            mon: MonitorState {
                monitors: None,
                shown_told: None,
                waiting: Default::default(),
            },
            ping_id: 0,
            host_seq: HostSeqs {
                lo: u64::MAX,
                hi: 0,
                seen: 0,
            },
            tuner,
            audio,
            share: Sharing {
                clipboard,
                files,
                file_msgs,
                file_in: ReliableReceiver::new(),
                file_events,
                file_policy,
                pastes,
                hold: crate::paste::Hold::default(),
            },
            checks: Checks {
                sim,
                packet_log,
                others,
                pastes: test_pastes,
            },
        }
    }

    fn run(mut self) {
        let mut buf = vec![0u8; 2048];
        while !self.shared.stop.load(Ordering::Relaxed) {
            self.receive(&mut buf);
            for datagram in std::mem::take(&mut self.incoming) {
                self.on_datagram(datagram);
            }
            self.deliver();
            for msg in self.shared.requests.try_iter().collect::<Vec<_>>() {
                self.send(&msg, false);
            }
            self.share.clipboard_sets(&mut self.checks.pastes);
            self.shown_changed();
            self.send_input();
            self.ask_for_frames();
            self.publish();
            if self.last_stats.is_none_or(|t| t.elapsed() >= STATS_EVERY) {
                self.every_second();
            }
        }
        self.send(&ControlMessage::Bye, false);
        self.checks.others.finish();
        if let Some(c) = self.share.clipboard.as_mut() {
            c.finish();
        }
        if let Some(f) = &self.share.files {
            let _ = f.send(desktop::files::Cmd::Detach(self.wire.session));
        }
    }

    /// Simulated loss (`--sim-loss`): true for that fraction of calls.
    #[inline]
    fn sim_drop(&mut self) -> bool {
        self.checks.sim.drop_one()
    }

    /// `dropped`: consume a sequence number but don't send (simulated loss).
    fn send(&mut self, msg: &ControlMessage, dropped: bool) {
        let seq = self.wire.seq.take();
        if !dropped {
            let mut d = msg.datagram(self.wire.session, seq);
            self.wire
                .sealer
                .seal(&mut d)
                .expect("a datagram we built has a header");
            let _ = self.wire.link.send(&d);
        }
    }

    /// Waits a little for a datagram; takes what the emulator lets through.
    fn receive(&mut self, buf: &mut [u8]) {
        match self.wire.link.recv_timeout(buf, Duration::from_millis(2)) {
            Ok(Some(_)) if self.sim_drop() => {}
            Ok(Some(n)) => match self.checks.sim.emulator.as_mut() {
                Some(e) => e.send(Instant::now(), buf[..n].to_vec()),
                None => self.incoming.push(buf[..n].to_vec()),
            },
            Ok(None) => {}
            Err(e) => {
                eprintln!("network: {e}");
                self.shared.stop.store(true, Ordering::Relaxed);
            }
        }
        if let Some(e) = self.checks.sim.emulator.as_mut() {
            while let Some(d) = e.poll(Instant::now()) {
                self.incoming.push(d);
            }
        }
    }

    fn on_datagram(&mut self, mut datagram: Vec<u8>) {
        self.c.bytes += datagram.len() as u64;
        // Repeated handshake replies: expected and harmless.
        if PacketHeader::decode(&datagram).is_ok_and(|(h, _)| h.kind == PacketKind::Handshake) {
            return;
        }
        match self.wire.opener.open(&mut datagram) {
            Ok(len) => datagram.truncate(len),
            Err(_) => {
                self.c.rejected += 1;
                return;
            }
        }
        let Ok((h, body)) = PacketHeader::decode(&datagram) else {
            return;
        };
        if h.session != self.wire.session {
            return;
        }
        let t = wall_clock_us();
        let n = datagram.len();
        let video = (h.kind == PacketKind::Video)
            .then(|| protocol::FragmentHeader::decode(body).ok())
            .flatten()
            .map(|(f, _)| f);
        if let Some(msg) = video.and_then(|f| self.tuner.on_video(t, h.stream, &f, n)) {
            self.asked(&msg);
            self.send(&msg, false);
        }
        if let Some(log) = self.checks.packet_log.as_mut() {
            log_datagram(log, h.seq, t, n, video);
        }
        self.host_seq.lo = self.host_seq.lo.min(h.seq);
        self.host_seq.hi = self.host_seq.hi.max(h.seq);
        self.host_seq.seen += 1;
        match h.kind {
            PacketKind::Control => {
                if let Ok(msg) = ControlMessage::decode(body) {
                    self.on_control(msg);
                }
            }
            PacketKind::File => {
                if let Some(f) = &self.share.files
                    && let Ok(p) = FilePacket::decode(body)
                {
                    let _ = f.send(desktop::files::Cmd::Packet(Instant::now(), p));
                }
            }
            PacketKind::Audio => {
                if let Some(a) = &self.audio
                    && let Ok(p) = AudioPacket::decode(body)
                {
                    let _ = a.send(crate::audio::AudioMsg::Packet(p, self.c.clock.offset_us()));
                }
            }
            _ => {
                let _ = self.r.push(Instant::now(), &h, body, &mut self.deliveries);
            }
        }
    }

    fn on_control(&mut self, msg: ControlMessage) {
        match msg {
            ControlMessage::Bye => {
                eprintln!("host ended the session");
                self.shared.stop.store(true, Ordering::Relaxed);
            }
            ControlMessage::InputAck { last } => self.input.ack(last, Instant::now()),
            ControlMessage::HostStats(s) => self.c.host = Some((s, Instant::now())),
            ControlMessage::File { id, msg } => {
                for m in self.share.file_in.accept(id, msg) {
                    if let Some(f) = &self.share.files {
                        let _ = f.send(desktop::files::Cmd::Msg(m));
                    }
                }
                if let Some(last) = self.share.file_in.last_delivered() {
                    let dropped = self.sim_drop();
                    self.send(&ControlMessage::FileMsgAck { last }, dropped);
                }
            }
            ControlMessage::Clipboard { id, piece } => {
                if let Some(last) = self
                    .share
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.on_piece(id, piece))
                {
                    let dropped = self.sim_drop();
                    self.send(&ControlMessage::ClipboardAck { last }, dropped);
                }
            }
            ControlMessage::Pong {
                time_us, host_us, ..
            } => {
                let now_us = wall_clock_us();
                self.c.clock.sample(time_us, host_us, now_us);
                if let Some(rtt) = now_us.checked_sub(time_us) {
                    self.input.rtt_sample(Duration::from_micros(rtt));
                }
            }
            ControlMessage::Monitors {
                generation,
                monitors,
            } => {
                if self.mon.monitors.is_some_and(|(g, _)| g != generation) {
                    // The host started its streams over (their frame numbers go
                    // on): from the IDR they start with.
                    self.rec = Default::default();
                }
                self.mon.monitors = Some((generation, monitors.len()));
                self.mon.waiting = Default::default();
                *self.shared.monitors.lock().unwrap() = Some((generation, monitors));
                self.show_monitors();
                self.shared.wake.set();
            }
            // The host's monitors went: its picture
            // stays as it was until new ones come (`Monitors`).
            m @ (ControlMessage::MonitorWanted { .. } | ControlMessage::NoMonitors) => {
                if let Some(answer) = self.mon.waiting.heard(&m, true) {
                    let dropped = self.sim_drop();
                    self.send(&answer, dropped);
                }
            }
            ControlMessage::CursorPosition {
                x,
                y,
                visible,
                shape_id,
            } => {
                let _ = self.shared.cursor.send(CursorEvent::Position {
                    x,
                    y,
                    visible,
                    shape_id,
                });
                self.shared.wake.set();
            }
            _ => {}
        }
    }

    /// Complete frames to the decoder (unless it waits for a keyframe or a
    /// recovery frame), cursor shapes to the render thread; losses noted.
    fn deliver(&mut self) {
        self.r.expire(Instant::now(), &mut self.deliveries);
        let now_us = wall_clock_us();
        for d in std::mem::take(&mut self.deliveries) {
            match d {
                Delivery::Complete(p) if p.kind == PacketKind::Video => {
                    if p.stream != 0 {
                        self.checks.others.other_stream(&p, &self.c.clock);
                    }
                    let rec = &mut self.rec[stream_index(p.stream)];
                    if !rec.on_frame(p.frame, p.keyframe, p.recovery) {
                        self.c.stats.frames_dropped += 1;
                        continue;
                    }
                    let _ = self.shared.out.send(ToDecode {
                        payload: p,
                        reassembled_us: now_us,
                        queued: Instant::now(),
                    });
                    self.shared.wake.set();
                }
                Delivery::Complete(p) => match CursorShape::decode(&p.data) {
                    Ok(s) => {
                        let _ = self.shared.cursor.send(CursorEvent::Shape(s));
                        self.shared.wake.set();
                    }
                    Err(e) => eprintln!("bad cursor shape {}: {e}", p.frame),
                },
                Delivery::Lost {
                    kind: PacketKind::Video,
                    stream,
                    last,
                    ..
                } => self.rec[stream_index(stream)].on_lost(last),
                // A lost cursor shape is requested again by the render thread.
                Delivery::Lost { .. } => {}
            }
        }
    }

    /// New input events and resends of unacknowledged ones.
    fn send_input(&mut self) {
        // The clipboard first: keys typed right after a copy (Ctrl+V) find it.
        if let Some(c) = self.share.clipboard.as_mut() {
            match c.outgoing(self.share.files.as_ref()) {
                crate::clipboard::Outgoing::Text(pieces) => {
                    for p in pieces {
                        self.input.push(InputEvent::Clipboard(p));
                    }
                }
                crate::clipboard::Outgoing::Set(set) => self.share.hold.sent(set),
                crate::clipboard::Outgoing::Nothing => {}
            }
        }
        let fresh: Vec<InputEvent> = self.shared.inputs.try_iter().collect();
        self.c.inputs_sent += fresh.len() as u64;
        self.share.hold.extend(fresh);
        for ev in self.share.hold.release(Instant::now()) {
            self.input.push_small(ev);
        }
        if let Some(rx) = &self.share.file_msgs {
            for m in rx.try_iter() {
                self.input.push(InputEvent::File(m));
            }
        }
        let cost = |ev: &InputEvent| match ev {
            InputEvent::Clipboard(p) => p.data.len(),
            _ => 0,
        };
        for mut datagram in self
            .input
            .due_datagrams(Instant::now(), crate::clipboard::WINDOW, cost)
        {
            let msg = if datagram.len() == 1 {
                let (id, event) = datagram.pop().unwrap();
                ControlMessage::Input { id, event }
            } else {
                ControlMessage::Inputs { events: datagram }
            };
            let dropped = self.sim_drop();
            self.send(&msg, dropped);
        }
        self.c.inputs_resent = self.input.resent();
        self.c.inputs_in_flight = self.input.in_flight();
        self.c.input_rto = self.input.rto();
    }

    /// After a loss or a decode error: asks (again) for a recovery frame or
    /// an IDR, for each stream that needs one.
    fn ask_for_frames(&mut self) {
        let failed = self.shared.decode_failed.swap(0, Ordering::Relaxed);
        for stream in 0..protocol::MAX_STREAMS {
            let rec = &mut self.rec[usize::from(stream)];
            if failed & (1 << stream) != 0 {
                rec.on_decode_failed();
            }
            if !rec.may_ask() {
                continue;
            }
            let msg = match rec.ask(Instant::now()) {
                Some(Ask::Keyframe { frame }) => {
                    self.c.stats.keyframes_requested += 1;
                    ControlMessage::RequestKeyframe { stream, frame }
                }
                Some(Ask::Recovery { last_good, lost }) => {
                    self.c.stats.recoveries_requested += 1;
                    ControlMessage::RequestRecovery {
                        stream,
                        last_good,
                        lost,
                    }
                }
                None => continue,
            };
            self.send(&msg, false);
        }
    }

    /// The counters, for the render thread.
    fn publish(&mut self) {
        let rs = self.r.stats();
        let c = &mut self.c;
        c.stats.packets_received = self.host_seq.seen as u32;
        c.stats.packets_lost = (self.host_seq.hi + 1)
            .saturating_sub(self.host_seq.lo)
            .saturating_sub(self.host_seq.seen) as u32;
        c.stats.fec_recovered = rs.frames_recovered as u32;
        c.frames_lost = rs.frames_lost as u32;
        let mut shared = self.shared.counters.lock().unwrap();
        c.stats.frames_decoded = shared.stats.frames_decoded;
        c.decode_errors = shared.decode_errors;
        *shared = *c;
    }

    /// Remembers the pacing / parity asked of the host (the overlay shows them).
    fn asked(&mut self, msg: &ControlMessage) {
        match *msg {
            ControlMessage::SetPacing { rate_kbps, .. } => self.c.pacing_kbps = rate_kbps,
            ControlMessage::SetFec { parity_permille } => self.c.fec_permille = parity_permille,
            _ => {}
        }
    }

    /// The monitors the window shows, as of the host's last `Monitors`
    /// (repeated each second, and at once when they change).
    fn show_monitors(&mut self) {
        if let Some((generation, count)) = self.mon.monitors {
            let all = (1u16 << count).wrapping_sub(1) as u8;
            let streams = match self.shared.shown.load(Ordering::Relaxed) & all {
                0 => all,
                some => some,
            };
            self.mon.shown_told = Some(streams);
            self.send(
                &ControlMessage::ShowMonitors {
                    generation,
                    streams,
                },
                false,
            );
        }
    }

    /// The window shows other monitors now: the host hears it at once.
    fn shown_changed(&mut self) {
        let Some((_, count)) = self.mon.monitors else {
            return;
        };
        let all = (1u16 << count).wrapping_sub(1) as u8;
        let now = match self.shared.shown.load(Ordering::Relaxed) & all {
            0 => all,
            some => some,
        };
        if self.mon.shown_told != Some(now) {
            self.show_monitors();
        }
    }

    /// Stats, pacing and parity to the host; a ping (RTT, clock offset).
    fn every_second(&mut self) {
        self.show_monitors();
        self.c.net_cpu = desktop::cpu::cpu_time(true);
        self.send(&ControlMessage::Stats(self.c.stats), false);
        for msg in self.tuner.tick(Instant::now()) {
            self.asked(&msg);
            self.send(&msg, false);
        }
        self.ping_id = self.ping_id.wrapping_add(1);
        let ping = ControlMessage::Ping {
            id: self.ping_id,
            time_us: wall_clock_us(),
        };
        self.send(&ping, false);
        self.last_stats = Some(Instant::now());
    }
}
