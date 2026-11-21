//! Client session: handshake with the host and a network thread (reassembly,
//! loss recovery, feedback) feeding access units to the render loop, which
//! decodes them itself right before drawing.
//!
//! Loss recovery: after a frame is lost nothing more goes to the decoder (the
//! last good picture stays on screen) and the host is asked for a recovery
//! frame that references only frames up to the last one decoded
//! (`RequestRecovery`, reference invalidation on the host). Decoding resumes
//! with the first frame flagged `RECOVERY` (or a keyframe). An IDR is
//! requested only when there is no good frame yet or the decoder failed.
//!
//! network thread ──access units──▶ render thread: NVDEC → display shader → present
//!
//! Decoding stays on the render thread on purpose: CUDA↔D3D11 interop
//! (map/unmap of the output textures) on another thread while the render
//! thread uses the immediate context lost the device within a few frames, even
//! with D3D11 multithread protection on.

mod checks;
mod connection;
mod files;
mod handshake;
mod recovery;
mod thread;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use decoder::Frame;
use protocol::{ClientStats, CursorShape, Monitor};
use transport::{PublicKey, StaticKey};

pub use connection::{Connection, Pick};
pub use files::FilesHook;
pub use handshake::Unreachable;

pub use crate::tune::{Fec, Pacing};

pub struct Decoded {
    pub frame: Frame,
    /// The host's monitor (video stream) it shows.
    pub stream: u8,
    pub number: u32,
    /// Host wall clock at capture, µs (same machine: comparable with ours).
    pub capture_us: u64,
    pub reassembled_us: u64,
    /// When its access unit went into the decoder.
    pub decode_started_us: u64,
    pub decoded_us: u64,
}

#[derive(Default, Clone, Copy)]
pub struct Counters {
    pub stats: ClientStats,
    pub bytes: u64,
    pub frames_lost: u32,
    pub decode_errors: u32,
    /// Times a stream's decode queue was dropped (`QUEUE_LIMIT`), and the
    /// access units it held.
    pub queue_drops: u32,
    pub queue_dropped: u64,
    pub inputs_sent: u64,
    pub inputs_resent: u64,
    pub inputs_in_flight: usize,
    /// Current input resend timeout (follows the RTT).
    pub input_rto: Duration,
    /// Datagrams that failed decryption (forged, replayed or stale).
    pub rejected: u64,
    /// RTT to the host and its clock offset (`Ping` / `Pong`).
    pub clock: crate::clock::ClockSync,
    /// CPU time of the network thread so far (user + kernel).
    pub net_cpu: Duration,
    /// The host's last second (`HostStats`), and when it came.
    pub host: Option<(protocol::HostStats, Instant)>,
    /// What was asked of the host last: pacing, kbit/s, and FEC parity, per
    /// mille (0: nothing asked, the host's own).
    pub pacing_kbps: u32,
    pub fec_permille: u16,
}

/// The host's monitors as it said last (`Monitors`: generation, list), shared
/// by the network thread and the render thread.
pub type HostMonitors = Arc<Mutex<Option<(u32, Vec<Monitor>)>>>;

/// Pointer updates from the host, for the render thread.
pub enum CursorEvent {
    Shape(CursorShape),
    Position {
        x: i32,
        y: i32,
        visible: bool,
        shape_id: u32,
    },
}

/// Session options.
#[derive(Clone)]
pub struct Options {
    /// This device's key; the server tells the host to expect it.
    pub key: StaticKey,
    /// The host's public key (Noise IK: the client must know it in advance; from the server).
    pub host_key: PublicKey,
    /// The handshake's pre-shared key, from the host's token (the server does not know it).
    pub psk: [u8; 32],
    /// Frame rate to ask the host for (30 or 60); 0 = the host's default.
    pub fps: u16,
    /// Ask the host not to lock Windows after the session (`--keep-unlocked`).
    pub keep_unlocked: bool,
    /// Fraction of incoming datagrams to drop on purpose (testing recovery).
    pub sim_loss: f64,
    /// Emulated downlink rate, bit/s (a bottleneck with a 1 MiB queue).
    pub sim_bandwidth_bps: Option<u64>,
    /// Emulated downlink delay (on the host's datagrams, acks included: the RTT grows by it).
    pub sim_delay: Duration,
    /// Emulated downlink stalls: every `.0`, for `.1` (`--sim-stall`).
    pub sim_stall: Option<(Duration, Duration)>,
    /// Write every datagram from the host here (`--packet-log`): loss and arrival analysis.
    pub packet_log: Option<std::path::PathBuf>,
    /// The host's monitors to ask for, a bit each (`--show-monitors`); none: all.
    pub show_monitors: Option<u8>,
    /// Frames of monitor 2 and on to `<path>.<stream>` (`--dump-streams`).
    pub dump_streams: Option<std::path::PathBuf>,
    /// What pacing to ask the host for (`--pace`).
    pub pacing: Pacing,
    /// What FEC parity to ask the host for (`--fec`).
    pub fec: Fec,
    /// The clipboard shared with the host (`--clipboard*`).
    pub clipboard: crate::clipboard::ClipOptions,
    /// Where the host's sound goes (none: not played).
    pub audio: Option<crate::audio::AudioTx>,
    /// File transfers (none: off).
    pub files: Option<FilesHook>,
}
