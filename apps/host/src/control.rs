//! The session's control thread, and the control datagrams.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use std::sync::mpsc::Receiver;

use desktop::files::Cmd;
use protocol::{ControlMessage, FileMsg, FilePacket, PacketHeader, PacketKind, handshake_datagram};
use transport::{
    Opener, PublicKey, ReliableSender, Responder, Sealer, SeqCounter, StaticKey, UdpLink,
    wall_clock_us,
};

use crate::remote_input::RemoteInput;

/// Copies of handshake message 1 this soon after our answer are the same
/// burst (the client resends no sooner than every 200 ms).
const SAME_BURST: Duration = Duration::from_millis(150);

/// Events from the control receiver thread.
pub(crate) enum Event {
    Hello,
    Keyframe {
        stream: u8,
        frame: u32,
    },
    Recovery {
        stream: u8,
        last_good: u32,
        lost: u32,
    },
    /// The client shows the streams of `streams` (a bit each), as of
    /// `Monitors` number `generation`.
    Show {
        generation: u32,
        streams: u8,
    },
    Stats(protocol::ClientStats),
    CursorShape {
        id: u32,
    },
    Pacing {
        rate_kbps: u32,
        burst_bytes: u32,
    },
    Fec {
        parity_permille: u16,
    },
    /// The client's answer to `MonitorWanted` number `ask`.
    MonitorChoice {
        ask: u32,
        width: u32,
        height: u32,
        scale: u16,
    },
    Bye,
}

/// The session's sending key. Every thread that sends seals with it; the
/// control thread replaces it when the client handshakes again.
pub(crate) type SharedSealer = Arc<RwLock<Sealer>>;

pub(crate) fn seal(sealer: &RwLock<Sealer>, datagram: &mut Vec<u8>) {
    sealer
        .read()
        .unwrap()
        .seal(datagram)
        .expect("a datagram we built has a header");
}

pub(crate) fn control_datagram(
    seq: &SeqCounter,
    session: u32,
    sealer: &RwLock<Sealer>,
    msg: &ControlMessage,
) -> Vec<u8> {
    let mut d = msg.datagram(session, seq.take());
    seal(sealer, &mut d);
    d
}

/// Reads handshake message 1 and admits the client if it has the key the
/// server announced for the session.
pub(crate) fn admit(key: &StaticKey, expected: &PublicKey, datagram: &[u8]) -> Option<Responder> {
    let (h, body) = PacketHeader::decode(datagram).ok()?;
    if h.kind != PacketKind::Handshake {
        return None;
    }
    // Not for our key, garbage, or not the announced client: dropped.
    Responder::read(key, body)
        .ok()
        .filter(|r| r.remote() == *expected)
}

/// A decrypted datagram from the client.
enum Incoming {
    Control(ControlMessage),
    File(FilePacket),
}

fn parse(datagram: &[u8]) -> Option<Incoming> {
    let (h, body) = PacketHeader::decode(datagram).ok()?;
    match h.kind {
        PacketKind::Control => ControlMessage::decode(body).ok().map(Incoming::Control),
        PacketKind::File => FilePacket::decode(body).ok().map(Incoming::File),
        _ => None,
    }
}

/// Control thread: decrypts what the client sends, answers repeated
/// handshakes and pings, passes session events to the main loop; input is
/// applied right here (not once per frame) and acknowledged through
/// `ack_link`, and the host's clipboard texts go out from here too.
pub(crate) struct ControlThread {
    pub(crate) link: UdpLink,
    pub(crate) ack_link: UdpLink,
    pub(crate) session: u32,
    pub(crate) seq: SeqCounter,
    pub(crate) input: RemoteInput,
    pub(crate) events: Sender<Event>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) key: StaticKey,
    /// The announced client and the session's psk (its handshakes again).
    pub(crate) client_key: PublicKey,
    pub(crate) psk: [u8; 32],
    pub(crate) opener: Opener,
    pub(crate) sealer: SharedSealer,
    /// The last handshake message 1 and our answer: a repeat (our answer or
    /// the stream start was lost) gets the same answer, keeping the keys.
    pub(crate) last_hello: Vec<u8>,
    pub(crate) last_reply: Vec<u8>,
    /// When `last_reply` was sent.
    pub(crate) replied_at: Instant,
    pub(crate) rejected: u64,
    /// When the client was last heard from (`wall_clock_us`).
    pub(crate) last_rx: Arc<AtomicU64>,
    /// The files thread: data from the client goes there.
    pub(crate) files: Sender<Cmd>,
    /// Its messages to the client, sent reliably from here.
    pub(crate) file_msgs: Receiver<FileMsg>,
    /// Its word to the clipboard (the client's sets).
    pub(crate) file_events: Receiver<desktop::files::Event>,
    pub(crate) file_out: ReliableSender<FileMsg>,
}

impl ControlThread {
    /// A handshake on a running session: the client restarted, or our reply or
    /// the stream start got lost. Answers it and restarts the stream.
    fn handshake(&mut self, datagram: &[u8]) {
        let (_, body) = PacketHeader::decode(datagram).expect("checked by the caller");
        if body != self.last_hello.as_slice() {
            let Some(responder) = admit(&self.key, &self.client_key, datagram) else {
                return;
            };
            let Ok((reply, keys)) = responder.finish(&self.psk) else {
                return;
            };
            let (sealer, opener) = keys.split();
            *self.sealer.write().unwrap() = sealer;
            self.opener = opener;
            self.last_hello = body.to_vec();
            self.last_reply = handshake_datagram(self.session, &reply);
            println!("client handshake again: new session keys");
        } else if self.replied_at.elapsed() < SAME_BURST {
            // The client sends message 1 to all our addresses at once: the
            // copies that arrive at another address of this machine.
            return;
        } else {
            println!("repeated handshake: same answer, same keys");
        }
        let _ = self.ack_link.send(&self.last_reply);
        self.replied_at = Instant::now();
        let _ = self.events.send(Event::Hello);
    }

    /// Seals `msg` and sends it to the client.
    fn send(&self, msg: &ControlMessage) {
        let d = control_datagram(&self.seq, self.session, &self.sealer, msg);
        let _ = self.ack_link.send(&d);
    }

    /// The host's clipboard texts to the client: new pieces and resends.
    fn send_clipboard(&mut self) {
        let Some(c) = self.input.clipboard.as_mut() else {
            return;
        };
        for msg in c.due(Instant::now()) {
            self.send(&msg);
        }
    }

    /// The files thread's messages to the client: new ones and resends.
    fn send_files(&mut self) {
        for ev in self.file_events.try_iter() {
            if let Some(c) = self.input.clipboard.as_mut() {
                c.on_files_event(ev);
            }
        }
        for m in self.file_msgs.try_iter() {
            self.file_out.push(m);
        }
        for (id, msg) in self.file_out.due(Instant::now()) {
            self.send(&ControlMessage::File { id, msg });
        }
    }

    /// One message from the client. False: the main loop is gone, stop.
    fn on_message(&mut self, msg: ControlMessage) -> bool {
        let ev = match msg {
            ControlMessage::Input { id, event } => {
                if let Some(last) = self.input.accept(id, event) {
                    self.send(&ControlMessage::InputAck { last });
                }
                return true;
            }
            // New events with earlier unacknowledged ones: one acknowledgement.
            ControlMessage::Inputs { events } => {
                let mut last = None;
                for (id, event) in events {
                    last = self.input.accept(id, event).or(last);
                }
                if let Some(last) = last {
                    self.send(&ControlMessage::InputAck { last });
                }
                return true;
            }
            // Answered here, at once: the client measures RTT and clock offset.
            ControlMessage::Ping { id, time_us } => {
                self.send(&ControlMessage::Pong {
                    id,
                    time_us,
                    host_us: wall_clock_us(),
                });
                return true;
            }
            ControlMessage::ClipboardAck { last } => {
                if let Some(c) = self.input.clipboard.as_mut() {
                    c.ack(last, Instant::now());
                }
                return true;
            }
            ControlMessage::FileMsgAck { last } => {
                self.file_out.ack(last, Instant::now());
                return true;
            }
            ControlMessage::Hello { .. } => Event::Hello,
            ControlMessage::RequestKeyframe { stream, frame } => Event::Keyframe { stream, frame },
            ControlMessage::RequestRecovery {
                stream,
                last_good,
                lost,
            } => Event::Recovery {
                stream,
                last_good,
                lost,
            },
            ControlMessage::ShowMonitors {
                generation,
                streams,
            } => Event::Show {
                generation,
                streams,
            },
            ControlMessage::Stats(s) => Event::Stats(s),
            ControlMessage::RequestCursorShape { id } => Event::CursorShape { id },
            ControlMessage::SetPacing {
                rate_kbps,
                burst_bytes,
            } => Event::Pacing {
                rate_kbps,
                burst_bytes,
            },
            ControlMessage::SetFec { parity_permille } => Event::Fec { parity_permille },
            ControlMessage::MonitorChoice {
                ask,
                width,
                height,
                scale,
            } => Event::MonitorChoice {
                ask,
                width,
                height,
                scale,
            },
            ControlMessage::Bye => Event::Bye,
            _ => return true,
        };
        self.events.send(ev).is_ok()
    }

    pub(crate) fn run(mut self) -> (RemoteInput, u64) {
        let mut buf = vec![0u8; 2048];
        while !self.stop.load(Ordering::Relaxed) {
            self.send_clipboard();
            self.send_files();
            // Short: the clipboard's resends wait for this loop.
            let n = match self.link.recv_timeout(&mut buf, Duration::from_millis(20)) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(_) => break,
            };
            if PacketHeader::decode(&buf[..n]).is_ok_and(|(h, _)| h.kind == PacketKind::Handshake) {
                self.handshake(&buf[..n]);
                continue;
            }
            let n = match self.opener.open(&mut buf[..n]) {
                Ok(n) => {
                    self.last_rx.store(wall_clock_us(), Ordering::Relaxed);
                    n
                }
                Err(_) => {
                    self.rejected += 1;
                    continue;
                }
            };
            match parse(&buf[..n]) {
                Some(Incoming::Control(msg)) => {
                    if !self.on_message(msg) {
                        break;
                    }
                }
                Some(Incoming::File(p)) => {
                    let _ = self.files.send(Cmd::Packet(Instant::now(), p));
                }
                None => {}
            }
        }
        self.input.finish();
        (self.input, self.rejected)
    }
}
