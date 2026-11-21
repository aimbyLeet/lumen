//! What the client asks the host for about the link: pacing ([`crate::pace`])
//! and FEC parity ([`crate::autofec`]), picked from how frames arrive
//! ([`crate::arrival`]) or fixed from the command line.

use std::sync::Mutex;
use std::time::Instant;

use protocol::{ControlMessage, FragmentHeader};

use crate::arrival::Collector;
use crate::autofec::AutoFec;
use crate::pace::{self, AutoPace};

/// The host's pacing of frame data, as the client asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// Picked from how frames arrive ([`crate::pace`]).
    Auto,
    /// Nothing asked: the host's own.
    Host,
    /// kbit/s and burst bytes (0 = the host's own).
    Fixed(u32, u32),
}

/// The host's FEC parity, as the client asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fec {
    /// Picked from the losses ([`crate::autofec`]).
    Auto,
    /// Nothing asked: the host's own.
    Host,
    /// Per mille of data shards.
    Fixed(u16),
}

/// The automatic pacing last asked for in this process (one video window: one
/// host). A reconnection starts from it: from the top (150 Mbit/s) each new
/// session met a bad link with bursts of loss, lost its picture and
/// reconnected again, a dozen times in a row (2026-11-13).
static LAST_PACING: Mutex<Option<u32>> = Mutex::new(None);

pub struct LinkTuner {
    pacing: Pacing,
    fec: Fec,
    arrivals: Collector,
    auto_pace: Option<AutoPace>,
    auto_fec: Option<AutoFec>,
}

fn fec_change(old: u16, new: u16, why: &str) {
    println!(
        "FEC {:.1} -> {:.1} % ({why})",
        f64::from(old) / 10.0,
        f64::from(new) / 10.0
    );
}

impl LinkTuner {
    pub fn new(pacing: Pacing, fec: Fec) -> Self {
        LinkTuner {
            pacing,
            fec,
            arrivals: Collector::default(),
            auto_pace: (pacing == Pacing::Auto).then(|| match *LAST_PACING.lock().unwrap() {
                Some(kbps) => {
                    println!(
                        "pacing starts at {:.0} Mbit/s (as before the reconnection)",
                        f64::from(kbps) / 1000.0
                    );
                    AutoPace::starting_at(kbps)
                }
                None => AutoPace::default(),
            }),
            auto_fec: (fec == Fec::Auto).then(AutoFec::default),
        }
    }

    /// A video datagram of `bytes` arrived at `now_us`; a message for the host
    /// right away, if the parity must go up.
    pub fn on_video(
        &mut self,
        now_us: u64,
        stream: u8,
        f: &FragmentHeader,
        bytes: usize,
    ) -> Option<ControlMessage> {
        let done = self.arrivals.on_datagram(now_us, stream, f, bytes)?;
        if let Some(a) = self.auto_pace.as_mut() {
            a.on_frame(&done);
        }
        let a = self.auto_fec.as_mut()?;
        let old = a.permille();
        let (p, why) = a.on_frame(Instant::now(), &done)?;
        fec_change(old, p, &why);
        Some(ControlMessage::SetFec { parity_permille: p })
    }

    /// Once a second: what to ask the host for (repeated: the messages are unacknowledged).
    pub fn tick(&mut self, now: Instant) -> Vec<ControlMessage> {
        let mut out = Vec::new();
        let pacing = match (self.pacing, self.auto_pace.as_mut()) {
            (Pacing::Fixed(rate, burst), _) => Some((rate, burst)),
            (Pacing::Auto, Some(a)) => {
                let old = a.kbps();
                if let Some((kbps, why)) = a.tick(now) {
                    println!(
                        "pacing {:.0} -> {:.0} Mbit/s ({why})",
                        f64::from(old) / 1000.0,
                        f64::from(kbps) / 1000.0
                    );
                }
                *LAST_PACING.lock().unwrap() = Some(a.kbps());
                Some((a.kbps(), pace::BURST_BYTES))
            }
            _ => None,
        };
        if let Some((rate_kbps, burst_bytes)) = pacing {
            out.push(ControlMessage::SetPacing {
                rate_kbps,
                burst_bytes,
            });
        }
        let fec = match (self.fec, self.auto_fec.as_mut()) {
            (Fec::Fixed(p), _) => Some(p),
            (Fec::Auto, Some(a)) => {
                let old = a.permille();
                if let Some((p, why)) = a.tick(now) {
                    fec_change(old, p, &why);
                }
                Some(a.permille())
            }
            _ => None,
        };
        if let Some(parity_permille) = fec {
            out.push(ControlMessage::SetFec { parity_permille });
        }
        out
    }
}
