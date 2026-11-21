//! Lumen transport layer.
//!
//! - [`Packetizer`] / [`Reassembler`]: payload ↔ datagrams with Reed–Solomon FEC.
//! - [`BulkSender`] / [`BulkReceiver`]: file transfer data, paced, resent, acked selectively.
//! - [`Pacer`]: token bucket that spreads datagrams over time.
//! - [`CapController`]: keeps an encoder's real bitrate under a session cap.
//! - [`Initiator`] / [`Responder`] / [`Sealer`] / [`Opener`]: Noise IKpsk2 handshake and
//!   per-datagram encryption between devices (`secure` module docs).
//! - [`HostToken`]: a host's token; the server's `lookup` and the handshake psk come from it.
//! - [`default_dir`], [`config_args`]: where a device keeps its keys and config.
//! - [`UdpLink`], [`sleep_until`]: connected UDP socket, precise waits.
//! - [`LinkEmulator`] / [`EmulatedSender`]: deterministic link model (loss bursts,
//!   delay, jitter, reordering, bottleneck queue) to test all of the above on loopback.

mod bulk;
mod clock;
mod config;
mod emulator;
mod fec;
#[cfg(test)]
mod input_stall;
mod net;
mod pacing;
mod rate;
mod reliable;
mod rng;
mod secure;
mod sender;
mod seq;
mod stats;
mod token;

pub use bulk::{BulkConfig, BulkReceiver, BulkSender, BulkStats, TransferProgress, TransferState};
pub use clock::wall_clock_us;
pub use config::{config_args, default_dir, with_config_args};
pub use emulator::{LinkConfig, LinkEmulator, LinkStats};
pub use fec::{
    Delivery, FecConfig, FecError, MAX_PAYLOAD, Packetizer, Payload, Reassembler, ReassemblyStats,
};
pub use net::{EmulatedSender, SOCKET_BUFFER, UdpLink, sleep_until};
pub use pacing::{Pacer, PacerConfig};
pub use rate::CapController;
pub use reliable::{COPY_AFTER, MAX_GROUP, MAX_REORDER, ReliableReceiver, ReliableSender};
pub use rng::Rng;
pub use secure::{
    Initiator, Opener, PublicKey, Responder, Sealer, SecureError, SessionKeys, StaticKey,
};
pub use sender::{NowSender, PacedHandle, PacedSender, SealPaced, SenderStats};
pub use seq::SeqCounter;
pub use stats::{RECENT_SAMPLES, percentile, push_recent};
pub use token::HostToken;
