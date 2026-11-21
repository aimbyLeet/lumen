//! The client picks the host's FEC parity (`ControlMessage::SetFec`) from the
//! losses it sees; no setting.
//!
//! Every block that lost datagrams tells exactly what a lower parity would
//! have done: the host sends data shards, then parity in order, so with
//! fewer parity shards the block would have been the first `data + parity'`
//! of what was sent. Through WG over the owner's Wi-Fi (losses single, the
//! pacing keeping queues short) 5 % lost the same frames as 15 % in 6 of 7
//! runs, 2 % lost more (CLAUDE.md, «Подводные камни»).
//!
//! - A frame is lost that a higher level would have saved → the top at once,
//!   not at the next second (a lost frame is a freeze until the recovery
//!   frame, and a link going bad loses more within a second: through WG all
//!   the frames lost against a fixed 15 % were in the 2 s a run's link took
//!   to collapse, while the parity climbed a level a second).
//! - Every second: the next lower level would have lost no frame more than
//!   the current one over the last [`WINDOW`], and the level held for
//!   [`HOLD`] → one level down.
//!
//! Long bursts no level saves (a queue overflowing) are the pacing's job. The
//! top is the host's former fixed 15 %: a 2.5-minute WG run with 25 % on top
//! sent 15.8 thousand more parity datagrams there and saved no frame by them.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::arrival::FrameArrival;

/// Parity, per mille of data shards: the levels, and where it starts (the top).
pub const LEVELS: [u16; 4] = [30, 50, 100, 150];
const START: usize = 3;
const WINDOW: Duration = Duration::from_secs(20);
const HOLD: Duration = Duration::from_secs(10);
/// Datagrams in the window below which it tells too little to go down.
const MIN_PACKETS: u64 = 5000;

/// The host's parity for a block of `data` shards at `permille`
/// (`transport::FecConfig`: ceil, at least one).
fn parity_at(data: u16, permille: u16) -> u16 {
    (u32::from(data) * u32::from(permille))
        .div_ceil(1000)
        .max(1) as u16
}

/// A block that lost datagrams.
struct Loss {
    at: Instant,
    frame: u32,
    data: u16,
    /// Parity as sent.
    parity: u16,
    /// Lost shard indices, ascending.
    lost: Vec<u16>,
}

impl Loss {
    /// Whether the block would have been lost with parity at `permille`. Above
    /// the parity sent, the extra shards are lost as often as the sent ones.
    fn fails_at(&self, permille: u16) -> bool {
        let keep = parity_at(self.data, permille);
        let lost = if keep <= self.parity {
            let end = self.data + keep;
            self.lost.iter().filter(|&&i| i < end).count() as f64
        } else {
            let sent = f64::from(self.data + self.parity);
            (self.lost.len() as f64 * f64::from(self.data + keep) / sent).round()
        };
        lost > f64::from(keep)
    }

    fn failed(&self) -> bool {
        self.lost.len() > usize::from(self.parity)
    }
}

pub struct AutoFec {
    level: usize,
    losses: VecDeque<Loss>,
    /// Datagrams per second, for the window.
    packets: VecDeque<(Instant, u64)>,
    this_second: u64,
    started: Option<Instant>,
    changed: Option<Instant>,
}

impl Default for AutoFec {
    fn default() -> Self {
        AutoFec {
            level: START,
            losses: VecDeque::new(),
            packets: VecDeque::new(),
            this_second: 0,
            started: None,
            changed: None,
        }
    }
}

impl AutoFec {
    /// The parity to ask for, per mille.
    pub fn permille(&self) -> u16 {
        LEVELS[self.level]
    }

    /// A frame has arrived (as far as it will) at `now`; the top parity at
    /// once if it is lost and a higher level would have saved it.
    pub fn on_frame(&mut self, now: Instant, f: &FrameArrival) -> Option<(u16, String)> {
        self.this_second += u64::from(f.packets);
        let top = LEVELS.len() - 1;
        let mut raise = false;
        for b in f.blocks.iter().flatten() {
            let lost: Vec<u16> = b.lost().collect();
            if lost.is_empty() {
                continue;
            }
            let l = Loss {
                at: now,
                frame: f.number,
                data: b.data,
                parity: b.parity,
                lost,
            };
            raise |= self.level < top && l.failed() && !l.fails_at(LEVELS[top]);
            self.losses.push_back(l);
        }
        if !raise {
            return None;
        }
        self.level = top;
        self.changed = Some(now);
        Some((
            LEVELS[top],
            format!("frame {} lost, it would have saved it", f.number),
        ))
    }

    /// Frames of the window that would have been lost at `permille`.
    fn lost_frames_at(&self, permille: u16) -> usize {
        let mut frames: Vec<u32> = self
            .losses
            .iter()
            .filter(|l| l.fails_at(permille))
            .map(|l| l.frame)
            .collect();
        frames.dedup();
        frames.len()
    }

    /// Once a second: a new parity if it changed, and why.
    pub fn tick(&mut self, now: Instant) -> Option<(u16, String)> {
        let started = *self.started.get_or_insert(now);
        self.packets
            .push_back((now, std::mem::take(&mut self.this_second)));
        while self
            .packets
            .front()
            .is_some_and(|&(t, _)| now.duration_since(t) > WINDOW)
        {
            self.packets.pop_front();
        }
        while self
            .losses
            .front()
            .is_some_and(|l| now.duration_since(l.at) > WINDOW)
        {
            self.losses.pop_front();
        }
        let old = self.level;
        let down = old > 0
            && now.duration_since(started) >= WINDOW
            && now.duration_since(self.changed.unwrap_or(started)) >= HOLD
            && self.packets.iter().map(|&(_, n)| n).sum::<u64>() >= MIN_PACKETS
            && self.lost_frames_at(LEVELS[old - 1]) == self.lost_frames_at(LEVELS[old]);
        if !down {
            return None;
        }
        self.level = old - 1;
        self.changed = Some(now);
        Some((
            LEVELS[self.level],
            format!("{} s: it would have lost no frame more", WINDOW.as_secs()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrival::{Collector, tests::feed};

    /// One second of 60 frames of `data` shards with the parity of the level
    /// asked for; frame `k` loses `lost(k)`. Then the second's decision.
    fn second(
        a: &mut AutoFec,
        c: &mut Collector,
        sec: u32,
        t0: Instant,
        data: u16,
        lost: impl Fn(u32) -> Vec<u16>,
    ) -> Option<(u16, String)> {
        let now = t0 + Duration::from_secs(u64::from(sec));
        let parity = parity_at(data, a.permille());
        for k in 0..=60 {
            // A frame of its own number finishes the second's last frame.
            let (number, l) = match k {
                60 => (u32::MAX, Vec::new()),
                _ => (sec * 60 + k, lost(k)),
            };
            let t0_us = u64::from(sec * 60 + k) * 16_667;
            for f in feed(c, number, t0_us, data, parity, 100.0, &l) {
                if let Some(raised) = a.on_frame(now, &f) {
                    return Some(raised);
                }
            }
        }
        a.tick(now)
    }

    fn levels(
        a: &mut AutoFec,
        c: &mut Collector,
        t0: Instant,
        secs: std::ops::Range<u32>,
        data: u16,
        lost: impl Fn(u32, u32) -> Vec<u16>,
    ) -> Vec<(u32, u16)> {
        secs.filter_map(|s| second(a, c, s, t0, data, |k| lost(s, k)).map(|(p, _)| (s, p)))
            .collect()
    }

    #[test]
    fn the_parity_formula_is_the_hosts() {
        assert_eq!(parity_at(80, 150), 12);
        assert_eq!(parity_at(80, 30), 3);
        assert_eq!(parity_at(1, 30), 1);
        assert_eq!(parity_at(256, 50), 13);
    }

    #[test]
    fn single_losses_go_down_to_the_lowest_level() {
        let (mut a, mut c, t0) = (AutoFec::default(), Collector::default(), Instant::now());
        // One frame in ten loses one datagram: every level saves it.
        let got = levels(&mut a, &mut c, t0, 0..60, 80, |_, k| {
            if k % 10 == 0 { vec![7] } else { vec![] }
        });
        assert_eq!(got, vec![(20, 100), (30, 50), (40, 30)]);
    }

    #[test]
    fn it_stays_where_a_lower_level_would_lose_frames() {
        let (mut a, mut c, t0) = (AutoFec::default(), Collector::default(), Instant::now());
        // Every 5 s a frame of 80 loses 4 datagrams: 5 % (4 parity) saves it, 3 % does not.
        let got = levels(&mut a, &mut c, t0, 0..80, 80, |s, k| {
            if s % 5 == 0 && k == 30 {
                vec![10, 20, 30, 40]
            } else {
                vec![]
            }
        });
        assert_eq!(got, vec![(20, 100), (30, 50)]);
    }

    #[test]
    fn a_lost_frame_a_higher_level_saves_raises_it_at_once() {
        let (mut a, mut c, t0) = (AutoFec::default(), Collector::default(), Instant::now());
        let clean = levels(&mut a, &mut c, t0, 0..45, 80, |_, _| vec![]);
        assert_eq!(a.permille(), 30, "{clean:?}");
        // 3 %: 3 parity; 5 lost, 15 % (12 parity) saves it → 15 % with that frame.
        let got = second(&mut a, &mut c, 45, t0, 80, |k| {
            if k == 0 {
                vec![1, 2, 40, 60, 70]
            } else {
                vec![]
            }
        });
        let (p, why) = got.expect("raised");
        assert_eq!(p, 150);
        assert!(why.contains(&format!("frame {} ", 45 * 60)), "{why}");
        // 10 % saves it too: down while the loss is in the window; 5 % only after.
        let got = levels(&mut a, &mut c, t0, 46..90, 80, |_, _| vec![]);
        assert_eq!(got, vec![(55, 100), (66, 50), (76, 30)]);
    }

    #[test]
    fn bursts_no_level_saves_change_nothing() {
        let (mut a, mut c, t0) = (AutoFec::default(), Collector::default(), Instant::now());
        // A queue overflowing: 30 of 80 in a row, every second.
        let burst: Vec<u16> = (40..70).collect();
        let got = levels(&mut a, &mut c, t0, 0..45, 80, |_, k| {
            if k == 0 { burst.clone() } else { vec![] }
        });
        assert_eq!(got, vec![(20, 100), (30, 50), (40, 30)]);
    }

    #[test]
    fn a_still_screen_tells_too_little_to_go_down() {
        let (mut a, mut c, t0) = (AutoFec::default(), Collector::default(), Instant::now());
        // Frames of one datagram and its parity: under MIN_PACKETS in 20 s.
        let got = levels(&mut a, &mut c, t0, 0..40, 1, |_, _| vec![]);
        assert!(got.is_empty(), "{got:?}");
    }
}
