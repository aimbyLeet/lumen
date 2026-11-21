//! A host without a physical monitor: what the
//! service's host tells the service — whether Windows has a physical monitor,
//! the client in session — and what the service answers (the client's own
//! monitors are up, or it is to be asked).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use desktop::hostsvc::{self, Request};
use protocol::ControlMessage;

/// How often the host looks whether a physical monitor is there.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// `no-physical.flag` next to the exe: the host acts as if Windows had no
/// physical monitor (checks on a machine that has one). Next to an installed
/// host only an administrator can put it.
fn flag() -> Option<PathBuf> {
    Some(
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("no-physical.flag"),
    )
}

pub(crate) fn simulated() -> bool {
    flag().is_some_and(|f| f.exists())
}

/// Whether Windows has a physical monitor (plugged in: one turned off by the
/// service for a client's own monitors still counts).
pub(crate) fn physical() -> bool {
    !simulated() && vdd::display::physical_connected().unwrap_or(true)
}

/// The monitors Windows has, with their kinds, for the log: whether another
/// virtual display driver's is taken for a physical one shows here.
fn described() -> String {
    match vdd::display::available() {
        Ok(all) if all.is_empty() => "no monitors".into(),
        Ok(all) => all
            .iter()
            .map(|a| {
                let bus = vdd::display::bus_of(&a.adapter_path);
                format!("{:?} «{}» ({bus}, {})", a.kind, a.name, monitor_id(&a.path))
            })
            .collect::<Vec<_>>()
            .join(", "),
        Err(e) => format!("? ({e})"),
    }
}

/// A monitor's device path as Device Manager shows it: without `\\?\` and
/// the interface GUID (`DISPLAY#SAM7303#5&x&0&UID4352`).
fn monitor_id(path: &str) -> &str {
    let p = path.strip_prefix(r"\\?\").unwrap_or(path);
    p.rsplit_once("#{").map_or(p, |(id, _)| id)
}

pub(crate) fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the host tells the service (only the service's host: `on`).
pub(crate) struct Reporter {
    on: bool,
    /// What was told last: a physical monitor, the client, cut off.
    told: Option<(bool, Option<[u8; 32]>, bool)>,
    looked: Option<Instant>,
}

impl Reporter {
    pub(crate) fn new(on: bool) -> Reporter {
        if on && simulated() {
            println!("monitors: no-physical.flag: acting as if Windows had no physical monitor");
        }
        Reporter {
            on,
            told: None,
            looked: None,
        }
    }

    /// Tells the service now; its answer about the monitors.
    pub(crate) fn tell(
        &mut self,
        client: Option<&[u8; 32]>,
        lost: bool,
    ) -> Option<hostsvc::Monitors> {
        if !self.on {
            return None;
        }
        let physical = physical();
        if self.told.is_none_or(|t| t.0 != physical) {
            println!("monitors: Windows has {}", described());
        }
        self.looked = Some(Instant::now());
        self.told = Some((physical, client.copied(), lost));
        let req = Request::HostMonitors {
            physical,
            client: client.map(hex),
            lost,
        };
        let t = Instant::now();
        match hostsvc::call(&req) {
            Ok(state) => {
                println!(
                    "monitors: told the service physical {physical}, client {}{} ({} ms): {}",
                    client.map_or("none".into(), |k| hex(k)[..8].to_string()),
                    if lost { " (cut off)" } else { "" },
                    t.elapsed().as_millis(),
                    match &state.monitors.client {
                        None if physical => "the machine's monitors".to_string(),
                        None => "none (nobody in session)".to_string(),
                        Some(c) if c.parked => {
                            "the last client's parked (nobody in session)".into()
                        }
                        Some(c) if c.set.is_none() => "the client is to be asked".into(),
                        Some(_) => "the client's own monitors".into(),
                    }
                );
                Some(state.monitors)
            }
            Err(e) => {
                println!("monitors: the service: {e}");
                None
            }
        }
    }

    /// Tells the service if a physical monitor came or went (looked at once
    /// a second) or the client changed.
    pub(crate) fn look(
        &mut self,
        client: Option<&[u8; 32]>,
        lost: bool,
    ) -> Option<hostsvc::Monitors> {
        if !self.on || self.looked.is_some_and(|t| t.elapsed() < LOOK_EVERY) {
            return None;
        }
        self.looked = Some(Instant::now());
        let now = (physical(), client.copied(), lost);
        if self.told == Some(now) {
            return None;
        }
        self.tell(client, lost)
    }
}

/// Why a session has no stream: what the client is told until a monitor comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// `MonitorWanted` number `n`: the client's own monitor is to be made.
    Ask(u32),
    /// `NoMonitors`: none, and none can be made.
    Nothing,
}

/// How often the question (or `NoMonitors`) goes again.
const WAITING_AGAIN: Duration = Duration::from_millis(500);

/// A session without a monitor to stream: the questions to its client and
/// the answers taken.
pub(crate) struct Asking {
    /// The service has no set of this client's: it is asked when there is
    /// no monitor.
    pub(crate) to_ask: bool,
    waiting: Option<Waiting>,
    sent: Option<Instant>,
    /// The last question's number; the one whose answer went to the service.
    asks: u32,
    answered: Option<u32>,
}

impl Asking {
    pub(crate) fn new(to_ask: bool) -> Asking {
        Asking {
            to_ask,
            waiting: None,
            sent: None,
            asks: 0,
            answered: None,
        }
    }

    /// No monitor: the client is asked (the same question while it stands)
    /// or told there is none. What it is told now, if that changed.
    pub(crate) fn wait(&mut self) -> Option<Waiting> {
        let w = match (self.to_ask, self.waiting) {
            (true, Some(Waiting::Ask(n))) => Waiting::Ask(n),
            (true, _) => {
                self.asks += 1;
                Waiting::Ask(self.asks)
            }
            (false, _) => Waiting::Nothing,
        };
        let changed = self.waiting != Some(w);
        self.waiting = Some(w);
        changed.then_some(w)
    }

    /// The streams started: nothing to tell.
    pub(crate) fn stop(&mut self) {
        self.waiting = None;
    }

    /// Time to tell the client again.
    pub(crate) fn due(&self) -> bool {
        self.waiting.is_some() && self.sent.is_none_or(|t| t.elapsed() >= WAITING_AGAIN)
    }

    /// What to send the client now (counted as sent).
    pub(crate) fn message(&mut self) -> Option<ControlMessage> {
        let msg = match self.waiting? {
            Waiting::Ask(ask) => ControlMessage::MonitorWanted { ask },
            Waiting::Nothing => ControlMessage::NoMonitors,
        };
        self.sent = Some(Instant::now());
        Some(msg)
    }

    /// The client answered question `ask`: whether to pass it on (the
    /// question stands, its answer has not gone yet).
    pub(crate) fn take_answer(&mut self, ask: u32) -> bool {
        if self.waiting != Some(Waiting::Ask(ask)) || self.answered == Some(ask) {
            return false;
        }
        self.answered = Some(ask);
        true
    }
}

/// The client is to be asked for its monitor (the service has no set of its).
pub(crate) fn to_ask(m: &hostsvc::Monitors) -> bool {
    m.client.as_ref().is_some_and(|c| c.set.is_none())
}

/// The monitor the client chose, to the service: its set from now on.
pub(crate) fn choose(client: &[u8; 32], width: u32, height: u32, scale: u16) -> bool {
    let t = Instant::now();
    let r = hostsvc::call(&Request::ClientChoice {
        client: hex(client),
        width,
        height,
        scale,
    });
    match r {
        Ok(_) => {
            println!(
                "monitors: the client chose {width}x{height} at {scale} %: up in {} ms",
                t.elapsed().as_millis()
            );
            true
        }
        Err(e) => {
            println!("monitors: the client chose {width}x{height} at {scale} %: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_stands_until_answered() {
        let mut a = Asking::new(true);
        assert!(!a.due(), "nothing to tell yet");
        assert_eq!(a.wait(), Some(Waiting::Ask(1)));
        assert_eq!(a.wait(), None, "the same question");
        assert!(a.due());
        assert_eq!(a.message(), Some(ControlMessage::MonitorWanted { ask: 1 }));
        assert!(!a.due(), "just sent");
        assert!(!a.take_answer(2), "not this question's");
        assert!(a.take_answer(1));
        assert!(!a.take_answer(1), "once");
        // Answered, the monitor came: streams.
        a.to_ask = false;
        a.stop();
        assert!(!a.due());
        assert_eq!(a.message(), None);
        // Gone again, the client has a set by now: none can be made.
        assert_eq!(a.wait(), Some(Waiting::Nothing));
        assert_eq!(a.message(), Some(ControlMessage::NoMonitors));
        // Asked anew: a new question, its answer taken again.
        a.to_ask = true;
        assert_eq!(a.wait(), Some(Waiting::Ask(2)));
        assert!(a.take_answer(2));
    }
}
