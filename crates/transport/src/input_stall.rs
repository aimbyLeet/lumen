//! Model of the input path (client `ReliableSender` → host
//! `ReliableReceiver` → cumulative `InputAck`) in virtual time: how long
//! input stalls at the host when datagrams are lost: input as ordinary
//! messages (resent after an RTO only, as up to v10) and as small ones
//! (copied after 10 ms and carried along with later ones, v11). Before v11
//! the copy and the carrying were weighed here apart and together (commit
//! 26c583d): together they were best. Not a check, a report:
//! `cargo test -p transport --lib input_stall -- --ignored --nocapture`.
//!
//! As in a session: the client looks at what is due every 2 ms, the host
//! acknowledges every input datagram it receives (also duplicates and early
//! ones) with the last id applied. Both directions lose datagrams on their
//! own (`loss`) and together during bursts (a Wi-Fi queue overflowing), and
//! add a random delay on top of the path.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use crate::reliable::{ReliableReceiver, ReliableSender};
use crate::rng::Rng;

/// The client's network loop: new events and resends go out at this step.
const POLL_MS: u64 = 2;
/// An event later than the path by more than this is a visible stall (a frame at 60 Hz).
const STALL_MS: u64 = 16;

#[derive(Clone, Copy)]
struct Path {
    one_way_ms: u64,
    /// Each datagram adds a uniform 0..=jitter on top of `one_way_ms`.
    jitter_ms: u64,
    /// Independent loss of each datagram.
    loss: f64,
    /// Bursts per minute during which everything sent, both ways, is lost.
    bursts_per_min: f64,
    burst_ms: u64,
}

struct Report {
    seconds: u64,
    /// Datagrams with input sent, per event.
    datagrams_per_event: f64,
    /// Events carried, per datagram.
    events_per_datagram: f64,
    /// Mouse moves: runs of consecutive moves later than [`STALL_MS`], the worst delay in each.
    move_stalls: Vec<u64>,
    /// Clicks (button down and up): delay beyond the path, sorted.
    clicks: Vec<u64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Move,
    Click,
}

/// The user's input: the mouse moves at 125 Hz for 2 s, stays still for 1 s
/// and clicks in the middle of that second (button down, up 80 ms later).
fn input(seconds: u64) -> Vec<(u64, Kind)> {
    let mut t = Vec::new();
    for cycle in 0..seconds / 3 {
        let start = cycle * 3000;
        t.extend((0..250).map(|k| (start + k * 8, Kind::Move)));
        t.push((start + 2500, Kind::Click));
        t.push((start + 2580, Kind::Click));
    }
    t
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Datagram {
    Input(Vec<(u32, u32)>),
    Ack { last: u32 },
}

/// Sorted, non-overlapping burst windows `[start, end)`.
fn bursts(path: Path, end: u64, rng: &mut Rng) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    if path.bursts_per_min <= 0.0 {
        return out;
    }
    let mean_gap = 60_000.0 / path.bursts_per_min;
    let mut t = 0.0;
    loop {
        t += -mean_gap * (1.0 - rng.next_f64()).ln();
        let start = t as u64;
        if start >= end {
            return out;
        }
        out.push((start, start + path.burst_ms));
        t += path.burst_ms as f64;
    }
}

/// `small`: input as small messages (v11), otherwise ordinary ones (v10).
fn run(path: Path, small: bool, seconds: u64, seed: u64) -> Report {
    let events = input(seconds);
    let mut rng = Rng::new(seed);
    let end = seconds * 1000 + 5000;
    let bursts = bursts(path, end, &mut rng);
    let in_burst = |t: u64| {
        let i = bursts.partition_point(|b| b.1 <= t);
        bursts.get(i).is_some_and(|b| b.0 <= t)
    };
    let mut sender = ReliableSender::<u32>::new();
    let mut receiver = ReliableReceiver::<u32>::new();
    let mut air: BinaryHeap<Reverse<(u64, Datagram)>> = BinaryHeap::new();
    let mut delivered = vec![None::<u64>; events.len()];
    let mut next_event = 0usize;
    let (mut datagrams, mut carried) = (0u64, 0u64);
    let t0 = Instant::now();
    let delay = |rng: &mut Rng| path.one_way_ms + rng.below_inclusive(path.jitter_ms);
    let mut t = 0u64;
    while t < end {
        let now = t0 + Duration::from_millis(t);
        while let Some(Reverse((at, _))) = air.peek() {
            if *at > t {
                break;
            }
            let Reverse((_, d)) = air.pop().unwrap();
            match d {
                Datagram::Input(list) => {
                    for (id, event) in list {
                        for e in receiver.accept(id, event) {
                            delivered[e as usize] = Some(t);
                        }
                    }
                    if let Some(last) = receiver.last_delivered()
                        && rng.next_f64() >= path.loss
                        && !in_burst(t)
                    {
                        air.push(Reverse((t + delay(&mut rng), Datagram::Ack { last })));
                    }
                }
                Datagram::Ack { last } => sender.ack(last, now),
            }
        }
        if t.is_multiple_of(POLL_MS) {
            while next_event < events.len() && events[next_event].0 <= t {
                if small {
                    sender.push_small(next_event as u32);
                } else {
                    sender.push(next_event as u32);
                }
                next_event += 1;
            }
            let out = sender.due_datagrams(now, usize::MAX, |_| 0);
            for list in out {
                datagrams += 1;
                carried += list.len() as u64;
                if rng.next_f64() >= path.loss && !in_burst(t) {
                    air.push(Reverse((t + delay(&mut rng), Datagram::Input(list))));
                }
            }
        }
        t += 1;
    }
    let late: Vec<(u64, Kind)> = events
        .iter()
        .zip(&delivered)
        .map(|(&(at, kind), d)| {
            let d = d.expect("every event delivered");
            ((d - at).saturating_sub(path.one_way_ms), kind)
        })
        .collect();
    let mut move_stalls = Vec::new();
    let mut run_max = 0u64;
    for &(x, kind) in &late {
        if kind == Kind::Click {
            continue;
        }
        if x > STALL_MS {
            run_max = run_max.max(x);
        } else if run_max > 0 {
            move_stalls.push(run_max);
            run_max = 0;
        }
    }
    if run_max > 0 {
        move_stalls.push(run_max);
    }
    move_stalls.sort_unstable();
    let mut clicks: Vec<u64> = late
        .iter()
        .filter(|l| l.1 == Kind::Click)
        .map(|l| l.0)
        .collect();
    clicks.sort_unstable();
    Report {
        seconds,
        datagrams_per_event: datagrams as f64 / events.len() as f64,
        events_per_datagram: carried as f64 / datagrams as f64,
        move_stalls,
        clicks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WG: Path = Path {
        one_way_ms: 20,
        jitter_ms: 10,
        loss: 0.0,
        bursts_per_min: 0.0,
        burst_ms: 0,
    };

    #[test]
    fn lossless_path_adds_nothing() {
        let path = Path { jitter_ms: 0, ..WG };
        for small in [false, true] {
            let r = run(path, small, 30, 1);
            assert!(r.move_stalls.is_empty(), "small {small}");
            assert_eq!(*r.clicks.last().unwrap(), 0, "small {small}");
        }
    }

    #[test]
    #[ignore = "a report, not a check"]
    fn input_stall_report() {
        eprintln!(
            "RTT jit loss  bursts       variant     | dgram/ev ev/dgram | move stalls/min >33ms/min  p50  max | clicks late>16ms  p99  max"
        );
        let paths = [
            (0.001, 0.0, 0),
            (0.01, 0.0, 0),
            (0.05, 0.0, 0),
            (0.0, 6.0, 20),
            (0.0, 6.0, 60),
            (0.0, 6.0, 150),
        ];
        for one_way in [20, 40] {
            for (loss, bursts_per_min, burst_ms) in paths {
                let path = Path {
                    one_way_ms: one_way,
                    loss,
                    bursts_per_min,
                    burst_ms,
                    ..WG
                };
                for small in [false, true] {
                    let r = run(path, small, 3600, 7);
                    let s = &r.move_stalls;
                    let c = &r.clicks;
                    let late = c.iter().filter(|&&x| x > STALL_MS).count();
                    eprintln!(
                        "{:3} {:3} {:4.1}% {:2}/min {:3}ms {:11} | {:8.2} {:8.2} | {:15.2} {:10.2} {:4} {:4} | {:6} {:8.2}% {:4} {:4}",
                        one_way * 2,
                        path.jitter_ms,
                        loss * 100.0,
                        bursts_per_min,
                        burst_ms,
                        if small { "v11" } else { "v10" },
                        r.datagrams_per_event,
                        r.events_per_datagram,
                        s.len() as f64 * 60.0 / r.seconds as f64,
                        s.iter().filter(|&&x| x > 2 * STALL_MS).count() as f64 * 60.0
                            / r.seconds as f64,
                        s.get(s.len() / 2).copied().unwrap_or(0),
                        s.last().copied().unwrap_or(0),
                        c.len(),
                        late as f64 * 100.0 / c.len() as f64,
                        c[(c.len() - 1) * 99 / 100],
                        c.last().unwrap(),
                    );
                }
                eprintln!();
            }
        }
    }
}
