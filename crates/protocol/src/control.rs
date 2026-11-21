//! Control messages: negotiation, feedback, input, cursor position.
//!
//! Each message fits in one datagram ([`PacketKind::Control`](crate::PacketKind)).
//! Body: `type: u8` followed by the fields in declaration order, little-endian.
//!
//! Input is delivered reliably and in order: every [`ControlMessage::Input`] carries
//! a per-session id (0, 1, 2, …); the host applies ids strictly in sequence,
//! buffers early ones, drops duplicates, and answers with a cumulative
//! [`ControlMessage::InputAck`]. The client resends unacknowledged events.
//! Small events (not clipboard or file pieces) also go in
//! [`ControlMessage::Inputs`] along with earlier unacknowledged ones (v11):
//! a lost datagram is made up for by the next one, not by a resend.
//! Clipboard text goes the same way in both directions (`clipboard`).

use crate::DecodeError;
use crate::clipboard::{CLIP_PIECE_LEN, ClipPiece};
use crate::file::FileMsg;
use crate::format::{Capabilities, VideoFormat};
use crate::wire::{Reader, WriteExt, wire_enum};

wire_enum! {
    pub enum MouseButton {
        Left = 1,
        Right = 2,
        Middle = 3,
        X1 = 4,
        X2 = 5,
    }
}

/// One monitor of the host (`ControlMessage::Monitors`): where it is on the
/// host's desktop (Windows' virtual screen: the primary monitor at 0, 0) and
/// which video stream carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    pub stream: u8,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Monitors in one `Monitors` message at most (= `MAX_STREAMS`).
pub const MAX_MONITORS: usize = crate::MAX_STREAMS as usize;

/// One client input event; pointer coordinates are on the host's desktop
/// (Windows' virtual screen, the primary monitor at 0, 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// Absolute pointer position.
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Wheel deltas in `WHEEL_DELTA` units (120 per notch); `dx` is the horizontal wheel.
    MouseWheel {
        dx: i16,
        dy: i16,
    },
    /// Keyboard event by hardware scan code (set 1), `extended` = E0 prefix.
    Key {
        scancode: u16,
        extended: bool,
        down: bool,
    },
    /// Ctrl+Alt+Del on the host (the client's own Ctrl+Alt+Del stays local:
    /// Windows takes it before any program). In order with the keys.
    SecureAttention,
    /// A piece of the text the client copied (in order with the keys: a
    /// Ctrl+V right after copying finds it on the host).
    Clipboard(ClipPiece),
    /// About a file transfer (protocol v8).
    File(FileMsg),
}

mod event_tag {
    pub const MOUSE_MOVE: u8 = 1;
    pub const MOUSE_BUTTON: u8 = 2;
    pub const MOUSE_WHEEL: u8 = 3;
    pub const KEY: u8 = 4;
    pub const SECURE_ATTENTION: u8 = 5;
    pub const CLIPBOARD: u8 = 6;
    pub const FILE: u8 = 7;
}

fn encode_piece(p: &ClipPiece, out: &mut Vec<u8>) {
    out.put_bool(p.last);
    out.put_u16(p.data.len() as u16);
    out.extend_from_slice(&p.data);
}

fn decode_piece(r: &mut Reader) -> Result<ClipPiece, DecodeError> {
    let last = r.bool()?;
    let len = usize::from(r.u16()?);
    if len > CLIP_PIECE_LEN {
        return Err(DecodeError::InvalidValue("ClipPiece"));
    }
    Ok(ClipPiece {
        last,
        data: r.take(len)?.to_vec(),
    })
}

impl InputEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            InputEvent::MouseMove { x, y } => {
                out.put_u8(event_tag::MOUSE_MOVE);
                out.put_i32(x);
                out.put_i32(y);
            }
            InputEvent::MouseButton { button, down } => {
                out.put_u8(event_tag::MOUSE_BUTTON);
                out.put_u8(button as u8);
                out.put_bool(down);
            }
            InputEvent::MouseWheel { dx, dy } => {
                out.put_u8(event_tag::MOUSE_WHEEL);
                out.put_i16(dx);
                out.put_i16(dy);
            }
            InputEvent::Key {
                scancode,
                extended,
                down,
            } => {
                out.put_u8(event_tag::KEY);
                out.put_u16(scancode);
                out.put_bool(extended);
                out.put_bool(down);
            }
            InputEvent::SecureAttention => out.put_u8(event_tag::SECURE_ATTENTION),
            InputEvent::Clipboard(ref p) => {
                out.put_u8(event_tag::CLIPBOARD);
                encode_piece(p, out);
            }
            InputEvent::File(ref m) => {
                out.put_u8(event_tag::FILE);
                m.encode(out);
            }
        }
    }

    fn decode(r: &mut Reader) -> Result<Self, DecodeError> {
        Ok(match r.u8()? {
            event_tag::MOUSE_MOVE => InputEvent::MouseMove {
                x: r.i32()?,
                y: r.i32()?,
            },
            event_tag::MOUSE_BUTTON => InputEvent::MouseButton {
                button: MouseButton::try_from(r.u8()?)?,
                down: r.bool()?,
            },
            event_tag::MOUSE_WHEEL => InputEvent::MouseWheel {
                dx: r.i16()?,
                dy: r.i16()?,
            },
            event_tag::KEY => InputEvent::Key {
                scancode: r.u16()?,
                extended: r.bool()?,
                down: r.bool()?,
            },
            event_tag::SECURE_ATTENTION => InputEvent::SecureAttention,
            event_tag::CLIPBOARD => InputEvent::Clipboard(decode_piece(r)?),
            event_tag::FILE => InputEvent::File(FileMsg::decode(r)?),
            _ => return Err(DecodeError::InvalidValue("InputEvent")),
        })
    }
}

/// Cumulative receive-side counters since `StreamStart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientStats {
    pub packets_received: u32,
    pub packets_lost: u32,
    pub fec_recovered: u32,
    pub frames_decoded: u32,
    pub frames_dropped: u32,
    pub keyframes_requested: u32,
    pub recoveries_requested: u32,
}

/// The host's side of the last second (`ControlMessage::HostStats`): what
/// the client's overlay shows next to its own numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostStats {
    /// Frames encoded per second, ×10.
    pub fps_x10: u16,
    /// What the encoder produced, kbit/s.
    pub video_kbps: u32,
    /// What the cap's controller set the encoder to, kbit/s; 0: no cap (CQP).
    pub encoder_kbps: u32,
    /// The session's ceiling, kbit/s; 0: none.
    pub cap_kbps: u32,
    /// Mean QP, ×10.
    pub qp_x10: u16,
    /// Capture → encoded, µs.
    pub encode_p50_us: u32,
    pub encode_max_us: u32,
    /// Datagrams waiting in the host's pacer at the end of the second.
    pub queued: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    /// Client → host: first message of a session. `fps`: frame rate the user
    /// asked for (30 or 60); 0 = the host's default. `keep_unlocked`: the
    /// user asked the host not to lock Windows after this session.
    Hello {
        caps: Capabilities,
        fps: u16,
        keep_unlocked: bool,
    },
    /// Host → client: video stream `stream` (re)starts in this format; its
    /// next frame is a keyframe.
    StreamStart { stream: u8, format: VideoFormat },
    /// Client → host: a frame of `stream` could not be recovered, send an IDR.
    RequestKeyframe { stream: u8, frame: u32 },
    /// Client → host: frames after `last_good` (the newest frame the client
    /// decoded) are lost or depend on lost ones; `lost` is the newest frame the
    /// client knows it lost. The host stops referencing frames after
    /// `last_good` and marks the next frame with `Flags::RECOVERY` (or sends an
    /// IDR when `last_good` has left its reference buffer). Per `stream`.
    RequestRecovery {
        stream: u8,
        last_good: u32,
        lost: u32,
    },
    /// Acknowledges a packet sent with `ACK_REQUESTED`.
    Ack { seq: u64 },
    /// Client → host: RTT and clock probe (`time_us`: the client's clock).
    Ping { id: u32, time_us: u64 },
    /// Host → client, at once: the `Ping`'s values and the host's clock when
    /// answering. The client gets the RTT and the offset between the clocks.
    Pong { id: u32, time_us: u64, host_us: u64 },
    /// Client → host feedback.
    Stats(ClientStats),
    /// Client → host: input event number `id` of the session (see module docs).
    Input { id: u32, event: InputEvent },
    /// Client → host: several input events with their ids, in id order; ids
    /// need not be consecutive, already applied ones are dropped (protocol
    /// v11). At most [`MAX_INPUTS`].
    Inputs { events: Vec<(u32, InputEvent)> },
    /// Host → client: every input id `<= last` has been applied.
    InputAck { last: u32 },
    /// Host → client: where the pointer points (the shape's hotspot), on the
    /// host's desktop, and the shape it uses. Drawn at `(x - hot_x, y - hot_y)`.
    CursorPosition {
        x: i32,
        y: i32,
        visible: bool,
        shape_id: u32,
    },
    /// Client → host: shape `id` was not received, send it again.
    RequestCursorShape { id: u32 },
    /// Client → host: send frame data at `rate_kbps` with bursts of at most
    /// `burst_bytes` back to back; 0 keeps the host's own value. Repeated
    /// (unacknowledged); a host that does not know it ignores it.
    SetPacing { rate_kbps: u32, burst_bytes: u32 },
    /// Client → host: FEC parity, per mille of data shards; 0 keeps the host's
    /// own. Repeated (unacknowledged); a host that does not know it ignores it.
    SetFec { parity_permille: u16 },
    /// Host → client: piece number `id` of the host's clipboard texts,
    /// reliable and in order like the input (module docs).
    Clipboard { id: u32, piece: ClipPiece },
    /// Client → host: every clipboard piece `<= last` has been received.
    ClipboardAck { last: u32 },
    /// Host → client, once a second: the host's numbers (overlay).
    HostStats(HostStats),
    /// Host → client: message number `id` about file transfers, reliable
    /// and in order like the clipboard.
    File { id: u32, msg: FileMsg },
    /// Client → host: every `File` message `<= last` has been received.
    FileMsgAck { last: u32 },
    /// Host → client: the host's monitors, number 1 (the primary one) first.
    /// `generation` changes whenever they change (count, size, place): the
    /// client starts its layout over. Repeated until a `ShowMonitors` with
    /// this `generation` comes back.
    Monitors {
        generation: u32,
        monitors: Vec<Monitor>,
    },
    /// Client → host: encode the streams whose bit is set in `streams`
    /// (bit `n` — stream `n`), as of the host's `generation`; the others are
    /// not captured. Repeated (unacknowledged), like `SetPacing`.
    ShowMonitors { generation: u32, streams: u8 },
    /// Host → client (v13): Windows on the host has no monitor of its own
    /// and the host keeps no virtual monitors for this client yet: the client
    /// asks its user for one and answers with `MonitorChoice` of this `ask`.
    /// Repeated (unacknowledged) until the monitor is there (`Monitors`); a
    /// new `ask` — asked anew (the monitors went again).
    MonitorWanted { ask: u32 },
    /// Client → host (v13): the user's monitor for `ask`: resolution and
    /// scale, %. Sent again for each repeated `MonitorWanted` of that `ask`.
    MonitorChoice {
        ask: u32,
        width: u32,
        height: u32,
        scale: u16,
    },
    /// Host → client (v13): no monitor, and none can be made (no Lumen VDD):
    /// nothing to show. Repeated while it lasts; `Monitors` when a monitor
    /// comes.
    NoMonitors,
    /// Either side: session is over.
    Bye,
}

mod tag {
    pub const HELLO: u8 = 1;
    pub const STREAM_START: u8 = 2;
    pub const REQUEST_KEYFRAME: u8 = 3;
    pub const ACK: u8 = 4;
    pub const PING: u8 = 5;
    pub const PONG: u8 = 6;
    pub const STATS: u8 = 7;
    pub const INPUT: u8 = 8;
    pub const INPUT_ACK: u8 = 9;
    pub const CURSOR_POSITION: u8 = 12;
    pub const REQUEST_CURSOR_SHAPE: u8 = 13;
    pub const BYE: u8 = 14;
    pub const REQUEST_RECOVERY: u8 = 15;
    pub const SET_PACING: u8 = 16;
    pub const SET_FEC: u8 = 17;
    pub const CLIPBOARD: u8 = 18;
    pub const CLIPBOARD_ACK: u8 = 19;
    pub const HOST_STATS: u8 = 20;
    pub const FILE: u8 = 21;
    pub const FILE_MSG_ACK: u8 = 22;
    pub const MONITORS: u8 = 23;
    pub const SHOW_MONITORS: u8 = 24;
    pub const INPUTS: u8 = 25;
    pub const MONITOR_WANTED: u8 = 26;
    pub const MONITOR_CHOICE: u8 = 27;
    pub const NO_MONITORS: u8 = 28;
}

/// Most events in one [`ControlMessage::Inputs`].
pub const MAX_INPUTS: usize = 16;

fn stream(r: &mut Reader) -> Result<u8, DecodeError> {
    let s = r.u8()?;
    if s >= crate::MAX_STREAMS {
        return Err(DecodeError::InvalidValue("stream"));
    }
    Ok(s)
}

impl ControlMessage {
    /// A control datagram carrying this message, to be sealed before sending.
    pub fn datagram(&self, session: u32, seq: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(64);
        crate::PacketHeader {
            kind: crate::PacketKind::Control,
            flags: crate::Flags::empty(),
            stream: 0,
            session,
            seq,
        }
        .encode(&mut d);
        self.encode(&mut d);
        d
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            ControlMessage::Hello {
                caps,
                fps,
                keep_unlocked,
            } => {
                out.put_u8(tag::HELLO);
                caps.encode(out);
                out.put_u16(fps);
                out.put_u8(u8::from(keep_unlocked));
            }
            ControlMessage::StreamStart { stream, format } => {
                out.put_u8(tag::STREAM_START);
                out.put_u8(stream);
                format.encode(out);
            }
            ControlMessage::RequestKeyframe { stream, frame } => {
                out.put_u8(tag::REQUEST_KEYFRAME);
                out.put_u8(stream);
                out.put_u32(frame);
            }
            ControlMessage::RequestRecovery {
                stream,
                last_good,
                lost,
            } => {
                out.put_u8(tag::REQUEST_RECOVERY);
                out.put_u8(stream);
                out.put_u32(last_good);
                out.put_u32(lost);
            }
            ControlMessage::Ack { seq } => {
                out.put_u8(tag::ACK);
                out.put_u64(seq);
            }
            ControlMessage::Ping { id, time_us } => {
                out.put_u8(tag::PING);
                out.put_u32(id);
                out.put_u64(time_us);
            }
            ControlMessage::Pong {
                id,
                time_us,
                host_us,
            } => {
                out.put_u8(tag::PONG);
                out.put_u32(id);
                out.put_u64(time_us);
                out.put_u64(host_us);
            }
            ControlMessage::Stats(s) => {
                out.put_u8(tag::STATS);
                out.put_u32(s.packets_received);
                out.put_u32(s.packets_lost);
                out.put_u32(s.fec_recovered);
                out.put_u32(s.frames_decoded);
                out.put_u32(s.frames_dropped);
                out.put_u32(s.keyframes_requested);
                out.put_u32(s.recoveries_requested);
            }
            ControlMessage::Input { id, ref event } => {
                out.put_u8(tag::INPUT);
                out.put_u32(id);
                event.encode(out);
            }
            ControlMessage::Inputs { ref events } => {
                out.put_u8(tag::INPUTS);
                out.put_u8(events.len() as u8);
                for (id, event) in events {
                    out.put_u32(*id);
                    event.encode(out);
                }
            }
            ControlMessage::InputAck { last } => {
                out.put_u8(tag::INPUT_ACK);
                out.put_u32(last);
            }
            ControlMessage::CursorPosition {
                x,
                y,
                visible,
                shape_id,
            } => {
                out.put_u8(tag::CURSOR_POSITION);
                out.put_i32(x);
                out.put_i32(y);
                out.put_bool(visible);
                out.put_u32(shape_id);
            }
            ControlMessage::RequestCursorShape { id } => {
                out.put_u8(tag::REQUEST_CURSOR_SHAPE);
                out.put_u32(id);
            }
            ControlMessage::SetPacing {
                rate_kbps,
                burst_bytes,
            } => {
                out.put_u8(tag::SET_PACING);
                out.put_u32(rate_kbps);
                out.put_u32(burst_bytes);
            }
            ControlMessage::SetFec { parity_permille } => {
                out.put_u8(tag::SET_FEC);
                out.put_u16(parity_permille);
            }
            ControlMessage::Clipboard { id, ref piece } => {
                out.put_u8(tag::CLIPBOARD);
                out.put_u32(id);
                encode_piece(piece, out);
            }
            ControlMessage::ClipboardAck { last } => {
                out.put_u8(tag::CLIPBOARD_ACK);
                out.put_u32(last);
            }
            ControlMessage::HostStats(s) => {
                out.put_u8(tag::HOST_STATS);
                out.put_u16(s.fps_x10);
                out.put_u32(s.video_kbps);
                out.put_u32(s.encoder_kbps);
                out.put_u32(s.cap_kbps);
                out.put_u16(s.qp_x10);
                out.put_u32(s.encode_p50_us);
                out.put_u32(s.encode_max_us);
                out.put_u32(s.queued);
            }
            ControlMessage::File { id, ref msg } => {
                out.put_u8(tag::FILE);
                out.put_u32(id);
                msg.encode(out);
            }
            ControlMessage::FileMsgAck { last } => {
                out.put_u8(tag::FILE_MSG_ACK);
                out.put_u32(last);
            }
            ControlMessage::Monitors {
                generation,
                ref monitors,
            } => {
                out.put_u8(tag::MONITORS);
                out.put_u32(generation);
                out.put_u8(monitors.len() as u8);
                for m in monitors {
                    out.put_u8(m.stream);
                    out.put_i32(m.x);
                    out.put_i32(m.y);
                    out.put_u32(m.width);
                    out.put_u32(m.height);
                }
            }
            ControlMessage::ShowMonitors {
                generation,
                streams,
            } => {
                out.put_u8(tag::SHOW_MONITORS);
                out.put_u32(generation);
                out.put_u8(streams);
            }
            ControlMessage::MonitorWanted { ask } => {
                out.put_u8(tag::MONITOR_WANTED);
                out.put_u32(ask);
            }
            ControlMessage::MonitorChoice {
                ask,
                width,
                height,
                scale,
            } => {
                out.put_u8(tag::MONITOR_CHOICE);
                out.put_u32(ask);
                out.put_u32(width);
                out.put_u32(height);
                out.put_u16(scale);
            }
            ControlMessage::NoMonitors => out.put_u8(tag::NO_MONITORS),
            ControlMessage::Bye => out.put_u8(tag::BYE),
        }
    }

    /// Decodes a decrypted control body. Trailing bytes are an error.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        let msg = match r.u8()? {
            tag::HELLO => ControlMessage::Hello {
                caps: Capabilities::decode(&mut r)?,
                fps: r.u16()?,
                keep_unlocked: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::InvalidValue("Hello")),
                },
            },
            tag::STREAM_START => ControlMessage::StreamStart {
                stream: stream(&mut r)?,
                format: VideoFormat::decode(&mut r)?,
            },
            tag::REQUEST_KEYFRAME => ControlMessage::RequestKeyframe {
                stream: stream(&mut r)?,
                frame: r.u32()?,
            },
            tag::REQUEST_RECOVERY => ControlMessage::RequestRecovery {
                stream: stream(&mut r)?,
                last_good: r.u32()?,
                lost: r.u32()?,
            },
            tag::ACK => ControlMessage::Ack { seq: r.u64()? },
            tag::PING => ControlMessage::Ping {
                id: r.u32()?,
                time_us: r.u64()?,
            },
            tag::PONG => ControlMessage::Pong {
                id: r.u32()?,
                time_us: r.u64()?,
                host_us: r.u64()?,
            },
            tag::STATS => ControlMessage::Stats(ClientStats {
                packets_received: r.u32()?,
                packets_lost: r.u32()?,
                fec_recovered: r.u32()?,
                frames_decoded: r.u32()?,
                frames_dropped: r.u32()?,
                keyframes_requested: r.u32()?,
                recoveries_requested: r.u32()?,
            }),
            tag::INPUT => ControlMessage::Input {
                id: r.u32()?,
                event: InputEvent::decode(&mut r)?,
            },
            tag::INPUTS => {
                let n = usize::from(r.u8()?);
                if n == 0 || n > MAX_INPUTS {
                    return Err(DecodeError::InvalidValue("Inputs"));
                }
                let mut events = Vec::with_capacity(n);
                for _ in 0..n {
                    events.push((r.u32()?, InputEvent::decode(&mut r)?));
                }
                ControlMessage::Inputs { events }
            }
            tag::INPUT_ACK => ControlMessage::InputAck { last: r.u32()? },
            tag::CURSOR_POSITION => ControlMessage::CursorPosition {
                x: r.i32()?,
                y: r.i32()?,
                visible: r.bool()?,
                shape_id: r.u32()?,
            },
            tag::REQUEST_CURSOR_SHAPE => ControlMessage::RequestCursorShape { id: r.u32()? },
            tag::SET_PACING => ControlMessage::SetPacing {
                rate_kbps: r.u32()?,
                burst_bytes: r.u32()?,
            },
            tag::SET_FEC => ControlMessage::SetFec {
                parity_permille: r.u16()?,
            },
            tag::CLIPBOARD => ControlMessage::Clipboard {
                id: r.u32()?,
                piece: decode_piece(&mut r)?,
            },
            tag::CLIPBOARD_ACK => ControlMessage::ClipboardAck { last: r.u32()? },
            tag::HOST_STATS => ControlMessage::HostStats(HostStats {
                fps_x10: r.u16()?,
                video_kbps: r.u32()?,
                encoder_kbps: r.u32()?,
                cap_kbps: r.u32()?,
                qp_x10: r.u16()?,
                encode_p50_us: r.u32()?,
                encode_max_us: r.u32()?,
                queued: r.u32()?,
            }),
            tag::FILE => ControlMessage::File {
                id: r.u32()?,
                msg: FileMsg::decode(&mut r)?,
            },
            tag::FILE_MSG_ACK => ControlMessage::FileMsgAck { last: r.u32()? },
            tag::MONITORS => {
                let generation = r.u32()?;
                let n = usize::from(r.u8()?);
                if n > MAX_MONITORS {
                    return Err(DecodeError::InvalidValue("Monitors"));
                }
                let mut monitors = Vec::with_capacity(n);
                for _ in 0..n {
                    monitors.push(Monitor {
                        stream: stream(&mut r)?,
                        x: r.i32()?,
                        y: r.i32()?,
                        width: r.u32()?,
                        height: r.u32()?,
                    });
                }
                ControlMessage::Monitors {
                    generation,
                    monitors,
                }
            }
            tag::SHOW_MONITORS => ControlMessage::ShowMonitors {
                generation: r.u32()?,
                streams: r.u8()?,
            },
            tag::MONITOR_WANTED => ControlMessage::MonitorWanted { ask: r.u32()? },
            tag::MONITOR_CHOICE => ControlMessage::MonitorChoice {
                ask: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                scale: r.u16()?,
            },
            tag::NO_MONITORS => ControlMessage::NoMonitors,
            tag::BYE => ControlMessage::Bye,
            t => return Err(DecodeError::UnknownMessage(t)),
        };
        r.finish()?;
        Ok(msg)
    }
}
