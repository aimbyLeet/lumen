//! Pacing and FEC parity, as the client asks for them.

use crate::streamer::Cmd;

use super::Session;

/// Frame data back to back at most (the pacer's bucket), unless the client
/// asks otherwise (it picks the pacing itself: apps/client/src/pace.rs). 64 KiB
/// let most of a 4K frame leave at LAN speed and overflow a queue on the way.
pub(super) const PACE_BURST: usize = 8 << 10;

/// Pacing and FEC parity, as the client asks.
pub(super) struct Pace {
    /// Pacing (rate bit/s, burst bytes): the host's own, and the current one.
    pub(super) default_pacing: (u64, usize),
    pub(super) pacing: (u64, usize),
    /// FEC parity of every stream now.
    pub(super) parity: f64,
}

impl Session<'_> {
    /// The client's pacing (0: the host's own).
    pub(super) fn set_pacing(&mut self, rate_kbps: u32, burst_bytes: u32) {
        let want = (
            match rate_kbps {
                0 => self.pace.default_pacing.0,
                r => u64::from(r) * 1000,
            },
            match burst_bytes {
                0 => self.pace.default_pacing.1,
                b => (b as usize).max(1500),
            },
        );
        if want != self.pace.pacing {
            self.pace.pacing = want;
            self.sender.set_pacing(want.0, want.1);
            println!(
                "pacing (asked by the client): {:.0} Mbit/s, bursts of {} KiB",
                want.0 as f64 / 1e6,
                want.1 / 1024
            );
        }
    }

    /// The client's FEC parity (0: the host's own), for every stream.
    pub(super) fn set_fec(&mut self, parity_permille: u16) {
        let want = match parity_permille {
            0 => self.st.args.parity / 100.0,
            p => f64::from(p) / 1000.0,
        };
        if want != self.pace.parity {
            self.pace.parity = want;
            self.cursor_packetizer.set_parity_ratio(want);
            self.streams.iter().for_each(|s| s.send(Cmd::Fec(want)));
            println!("FEC parity (asked by the client): {:.1} %", want * 100.0);
        }
    }
}
