//! Reliable, ordered delivery of small messages (client input, clipboard
//! pieces) over lossy UDP.
//!
//! Each message gets a consecutive id. The sender keeps it until a cumulative
//! acknowledgement covers it and resends it after a timeout that follows the
//! measured RTT (RFC 6298), doubling with each resend of the same message. The
//! receiver releases messages strictly in id order, buffers ones that arrive
//! early and drops duplicates, so a key-up can never overtake its key-down.
//!
//! Small messages (input events, [`ReliableSender::push_small`]) do not wait
//! for the timeout to make up for a lost datagram: each goes out once more
//! [`COPY_AFTER`] later unless acknowledged by then, and every datagram of
//! small messages also carries the oldest unacknowledged ones sent before
//! ([`ReliableSender::due_datagrams`]). A single loss then costs about 10 ms
//! instead of an RTO; a burst of losses, the time until the first datagram
//! after it (model and numbers: `input_stall`).

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Gives up buffering ids this far ahead of the next expected one (a bogus or
/// hostile id must not make the receiver allocate without bound).
pub const MAX_REORDER: u32 = 4096;

/// Resend timeout before the first RTT sample (RFC 6298): shorter would resend
/// everything sent in the first round trip of a long path, and the resent
/// messages give no RTT sample.
pub const INITIAL_RTO: Duration = Duration::from_secs(1);
/// Floor of the resend timeout: on a LAN the RTT alone would resend on any
/// scheduling hiccup of the host.
pub const MIN_RTO: Duration = Duration::from_millis(20);
/// Ceiling of the resend timeout, backoff included.
pub const MAX_RTO: Duration = Duration::from_secs(1);
/// A small message goes out once more this long after it was first sent,
/// unless acknowledged by then.
pub const COPY_AFTER: Duration = Duration::from_millis(10);
/// Most small messages in one datagram.
pub const MAX_GROUP: usize = protocol::MAX_INPUTS;

/// Smoothed RTT and the resend timeout derived from it (RFC 6298, section 2).
#[derive(Clone, Copy, Debug)]
pub struct RtoEstimator {
    srtt: Option<Duration>,
    rttvar: Duration,
    rto: Duration,
}

impl Default for RtoEstimator {
    fn default() -> Self {
        RtoEstimator {
            srtt: None,
            rttvar: Duration::ZERO,
            rto: INITIAL_RTO,
        }
    }
}

impl RtoEstimator {
    pub fn sample(&mut self, rtt: Duration) {
        let srtt = match self.srtt {
            None => {
                self.rttvar = rtt / 2;
                rtt
            }
            Some(srtt) => {
                self.rttvar = (self.rttvar * 3 + srtt.abs_diff(rtt)) / 4;
                (srtt * 7 + rtt) / 8
            }
        };
        self.srtt = Some(srtt);
        self.rto = (srtt + self.rttvar * 4).clamp(MIN_RTO, MAX_RTO);
    }

    pub fn rto(&self) -> Duration {
        self.rto
    }
}

struct Pending<T> {
    id: u32,
    msg: T,
    /// Copied after [`COPY_AFTER`] and carried along with later ones.
    small: bool,
    copied: bool,
    first_sent: Option<Instant>,
    last_sent: Option<Instant>,
    resends: u32,
}

pub struct ReliableSender<T> {
    next_id: u32,
    /// Unacknowledged messages in id order.
    pending: VecDeque<Pending<T>>,
    rtt: RtoEstimator,
    resent: u64,
}

impl<T: Clone> Default for ReliableSender<T> {
    fn default() -> Self {
        ReliableSender {
            next_id: 0,
            pending: VecDeque::new(),
            rtt: RtoEstimator::default(),
            resent: 0,
        }
    }
}

impl<T: Clone> ReliableSender<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a message; it goes out with the next [`due`](Self::due).
    pub fn push(&mut self, msg: T) -> u32 {
        self.queue(msg, false)
    }

    /// Queues a small message (an input event): see the module docs. Small
    /// messages must fit [`MAX_GROUP`] to a datagram.
    pub fn push_small(&mut self, msg: T) -> u32 {
        self.queue(msg, true)
    }

    fn queue(&mut self, msg: T, small: bool) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending.push_back(Pending {
            id,
            msg,
            small,
            copied: false,
            first_sent: None,
            last_sent: None,
            resends: 0,
        });
        id
    }

    /// Messages to send now: new ones and those unacknowledged for their
    /// timeout (the RTO, doubled for each resend so far, at most [`MAX_RTO`]).
    pub fn due(&mut self, now: Instant) -> Vec<(u32, T)> {
        self.due_within(now, usize::MAX, |_| 0)
    }

    /// Like [`due`](Self::due), with at most `window` bytes in flight as
    /// `cost` counts them (clipboard pieces: their text; input: nothing):
    /// a megabyte of text goes out window by window as it is acknowledged,
    /// not all at once. The window applies by position in the queue: the
    /// costs of everything ahead (resends included) count, and what is
    /// queued past the window waits (it is ordered behind it anyway). A
    /// message bigger than the window still goes if nothing ahead of it costs.
    pub fn due_within(
        &mut self,
        now: Instant,
        window: usize,
        cost: impl Fn(&T) -> usize,
    ) -> Vec<(u32, T)> {
        self.due_datagrams(now, window, cost)
            .into_iter()
            .flatten()
            .collect()
    }

    /// What to send now, by datagram: each message that is due alone, and
    /// the small ones that are due (new, resent, or copied after
    /// [`COPY_AFTER`]) together, at most [`MAX_GROUP`] to a datagram, with
    /// the oldest small ones sent before and not acknowledged in the room
    /// left (they hold back everything after them at the receiver). Each
    /// datagram is in id order. Otherwise as [`due_within`](Self::due_within).
    pub fn due_datagrams(
        &mut self,
        now: Instant,
        window: usize,
        cost: impl Fn(&T) -> usize,
    ) -> Vec<Vec<(u32, T)>> {
        let rto = self.rtt.rto();
        let mut out = Vec::new();
        // Positions in `pending` of the small messages to send.
        let mut small = Vec::new();
        let mut in_flight = 0usize;
        for (i, p) in self.pending.iter_mut().enumerate() {
            let c = cost(&p.msg);
            in_flight = in_flight.saturating_add(c);
            if in_flight > window && in_flight > c {
                break;
            }
            if let (Some(first), Some(last)) = (p.first_sent, p.last_sent) {
                let timeout = rto.saturating_mul(1 << p.resends.min(16)).min(MAX_RTO);
                if now.duration_since(last) < timeout {
                    // A copy leaves the resend timer and the RTT sample as
                    // they are: only if the first went missing is the sample
                    // longer, by up to COPY_AFTER.
                    if p.small && !p.copied && now.duration_since(first) >= COPY_AFTER {
                        p.copied = true;
                        small.push(i);
                    }
                    continue;
                }
                p.resends += 1;
                self.resent += 1;
            }
            p.first_sent.get_or_insert(now);
            p.last_sent = Some(now);
            if p.small {
                small.push(i);
            } else {
                out.push(vec![(p.id, p.msg.clone())]);
            }
        }
        if !small.is_empty() {
            self.group_small(small, now, &mut out);
        }
        out
    }

    /// Datagrams of the small messages at `positions` (in queue order), the
    /// room left in the last one taken by the oldest small ones sent before
    /// `now` and not acknowledged: they hold back everything after them at
    /// the receiver.
    fn group_small(&self, mut positions: Vec<usize>, now: Instant, out: &mut Vec<Vec<(u32, T)>>) {
        let room = (MAX_GROUP - positions.len() % MAX_GROUP) % MAX_GROUP;
        let riders: Vec<usize> = (0..self.pending.len())
            .filter(|&i| {
                let p = &self.pending[i];
                p.small && p.first_sent.is_some_and(|t| t < now) && !positions.contains(&i)
            })
            .take(room)
            .collect();
        positions.extend(riders);
        positions.sort_unstable();
        for chunk in positions.chunks(MAX_GROUP) {
            out.push(
                chunk
                    .iter()
                    .map(|&i| (self.pending[i].id, self.pending[i].msg.clone()))
                    .collect(),
            );
        }
    }

    /// Everything with id `<= last` has been delivered; the acknowledgement
    /// arrived at `now`. It gives an RTT sample (from the newest of them) only
    /// if none of them was resent (Karn): for a resent one it is unknown which
    /// copy got through, and the ones after it waited for it at the receiver.
    pub fn ack(&mut self, last: u32, now: Instant) {
        let mut sample = None;
        let mut resent = false;
        while let Some(p) = self.pending.front() {
            // Wrapping comparison: ids are consecutive, so "at or before `last`"
            // means within half the id space behind it.
            if last.wrapping_sub(p.id) >= u32::MAX / 2 {
                break;
            }
            resent |= p.resends > 0;
            sample = p.first_sent.map(|t| now.duration_since(t));
            self.pending.pop_front();
        }
        if let Some(rtt) = sample
            && !resent
        {
            self.rtt.sample(rtt);
        }
    }

    /// An RTT measured another way (ping / pong).
    pub fn rtt_sample(&mut self, rtt: Duration) {
        self.rtt.sample(rtt);
    }

    pub fn rto(&self) -> Duration {
        self.rtt.rto()
    }

    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    pub fn resent(&self) -> u64 {
        self.resent
    }
}

pub struct ReliableReceiver<T> {
    next: u32,
    delivered_any: bool,
    early: BTreeMap<u32, T>,
    duplicates: u64,
}

impl<T> Default for ReliableReceiver<T> {
    fn default() -> Self {
        ReliableReceiver {
            next: 0,
            delivered_any: false,
            early: BTreeMap::new(),
            duplicates: 0,
        }
    }
}

impl<T> ReliableReceiver<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes message `id`; returns the messages now deliverable, in order.
    pub fn accept(&mut self, id: u32, msg: T) -> Vec<T> {
        let ahead = id.wrapping_sub(self.next);
        if ahead >= MAX_REORDER {
            // Already delivered (behind `next`) or absurdly far ahead.
            self.duplicates += 1;
            return Vec::new();
        }
        if self.early.insert(id, msg).is_some() {
            self.duplicates += 1;
        }
        let mut out = Vec::new();
        while let Some(m) = self.early.remove(&self.next) {
            out.push(m);
            self.next = self.next.wrapping_add(1);
            self.delivered_any = true;
        }
        out
    }

    /// Cumulative acknowledgement to send back: the last id delivered, if any.
    pub fn last_delivered(&self) -> Option<u32> {
        self.delivered_any.then(|| self.next.wrapping_sub(1))
    }

    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn in_order_without_loss() {
        let mut s = ReliableSender::new();
        let mut r = ReliableReceiver::new();
        let now = Instant::now();
        for i in 0..10 {
            s.push(i);
        }
        let mut got = Vec::new();
        for (id, m) in s.due(now) {
            got.extend(r.accept(id, m));
        }
        assert_eq!(got, (0..10).collect::<Vec<_>>());
        assert_eq!(r.last_delivered(), Some(9));
        s.ack(9, now);
        assert_eq!(s.in_flight(), 0);
        assert!(s.due(now + MAX_RTO * 2).is_empty());
    }

    #[test]
    fn resends_only_after_rto() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.push('a');
        assert_eq!(s.due(t0).len(), 1);
        assert!(s.due(t0 + INITIAL_RTO / 2).is_empty());
        assert_eq!(s.due(t0 + INITIAL_RTO), vec![(0, 'a')]);
        assert_eq!(s.resent(), 1);
    }

    #[test]
    fn resend_timeout_doubles_up_to_the_ceiling() {
        let mut s = ReliableSender::new();
        let mut t = Instant::now();
        s.push('a');
        s.due(t);
        let mut timeout = INITIAL_RTO;
        for _ in 0..8 {
            assert!(s.due(t + timeout - MS).is_empty());
            t += timeout;
            assert_eq!(s.due(t).len(), 1);
            timeout = (timeout * 2).min(MAX_RTO);
        }
        assert_eq!(timeout, MAX_RTO);
    }

    #[test]
    fn rto_follows_the_acknowledged_rtt() {
        let mut s = ReliableSender::new();
        let mut t = Instant::now();
        // A 400 ms path with a little jitter: once a ping has measured it,
        // each message goes out once.
        s.rtt_sample(400 * MS);
        for i in 0..50u32 {
            s.push(i);
            assert_eq!(s.due(t).len(), 1, "message {i}");
            let rtt = Duration::from_millis(400 + u64::from(i % 3) * 5);
            assert!(s.due(t + rtt - MS).is_empty(), "message {i} resent");
            s.ack(i, t + rtt);
            t += rtt;
        }
        assert_eq!(s.resent(), 0);
        assert!((400 * MS..450 * MS).contains(&s.rto()), "{:?}", s.rto());
        // A loopback path settles on the floor.
        let mut s = ReliableSender::<u32>::new();
        for _ in 0..50 {
            s.rtt_sample(MS / 2);
        }
        assert_eq!(s.rto(), MIN_RTO);
    }

    #[test]
    fn resent_messages_give_no_rtt_sample() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.push('a');
        s.due(t0);
        s.due(t0 + INITIAL_RTO);
        // Acknowledged right after the resend: the RTT is not known.
        s.ack(0, t0 + INITIAL_RTO + MS);
        assert_eq!(s.rto(), INITIAL_RTO);
    }

    #[test]
    fn joint_ack_after_a_resend_gives_no_sample() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.rtt_sample(5 * MS);
        let rto = s.rto();
        s.push('a');
        s.push('b');
        s.due(t0);
        s.ack(0, t0 + 5 * MS);
        // 'b' is lost; 'c' gets through but the receiver holds it back.
        s.push('c');
        s.due(t0 + 6 * MS);
        s.due(t0 + 6 * MS + rto); // resends 'b' (and 'c', also unacknowledged)
        s.ack(2, t0 + 6 * MS + rto + 5 * MS);
        assert_eq!(s.rto(), rto, "the held-back 'c' must not inflate the RTO");
    }

    #[test]
    fn reorders_and_drops_duplicates() {
        let mut r = ReliableReceiver::new();
        assert!(r.accept(2, 'c').is_empty());
        assert!(r.accept(1, 'b').is_empty());
        assert_eq!(r.last_delivered(), None);
        assert_eq!(r.accept(0, 'a'), vec!['a', 'b', 'c']);
        assert!(r.accept(1, 'b').is_empty());
        assert!(r.accept(2, 'c').is_empty());
        assert_eq!(r.duplicates(), 2);
        assert!(r.accept(MAX_REORDER + 3, 'z').is_empty()); // far ahead: refused
        assert_eq!(r.last_delivered(), Some(2));
    }

    #[test]
    fn ids_wrap_around() {
        let mut s = ReliableSender::new();
        let mut r = ReliableReceiver::new();
        s.next_id = u32::MAX - 1;
        r.next = u32::MAX - 1;
        let now = Instant::now();
        for i in 0..4 {
            s.push(i);
        }
        let mut got = Vec::new();
        for (id, m) in s.due(now) {
            got.extend(r.accept(id, m));
        }
        assert_eq!(got, vec![0, 1, 2, 3]);
        s.ack(r.last_delivered().unwrap(), now);
        assert_eq!(s.in_flight(), 0);
    }

    #[test]
    fn window_limits_what_is_in_flight() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        let cost = |m: &usize| *m;
        for _ in 0..10 {
            s.push(100usize); // ten pieces of 100 bytes
        }
        s.push(0); // an input event behind them
        let sent = s.due_within(t0, 250, cost);
        assert_eq!(sent.iter().map(|m| m.0).collect::<Vec<_>>(), [0, 1]);
        assert!(
            s.due_within(t0, 250, cost).is_empty(),
            "nothing new until acked"
        );
        s.ack(1, t0 + MS);
        let sent = s.due_within(t0 + MS, 250, cost);
        assert_eq!(sent.iter().map(|m| m.0).collect::<Vec<_>>(), [2, 3]);
        // A piece bigger than the window still goes, alone.
        let mut s = ReliableSender::new();
        s.push(1000usize);
        s.push(1);
        assert_eq!(s.due_within(t0, 250, cost).len(), 1);
        // Without costs it is `due`.
        let mut s = ReliableSender::new();
        for i in 0..100usize {
            s.push(i);
        }
        assert_eq!(s.due_within(t0, 0, |_| 0).len(), 100);
    }

    #[test]
    fn window_counts_resent_pieces_too() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        let cost = |m: &usize| *m;
        for _ in 0..4 {
            s.push(100usize);
        }
        let ids = |v: Vec<(u32, usize)>| v.iter().map(|m| m.0).collect::<Vec<_>>();
        assert_eq!(ids(s.due_within(t0, 250, cost)), [0, 1]);
        // Nothing acknowledged, the RTO long passed: the two go again, and
        // they still take the window, so piece 2 does not.
        let later = t0 + Duration::from_secs(2);
        assert_eq!(ids(s.due_within(later, 250, cost)), [0, 1]);
        assert_eq!(s.resent(), 2);
    }

    fn ids(v: &[Vec<(u32, char)>]) -> Vec<Vec<u32>> {
        v.iter().map(|d| d.iter().map(|m| m.0).collect()).collect()
    }

    #[test]
    fn small_message_is_copied_once_unless_acknowledged() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.rtt_sample(40 * MS);
        s.push_small('a');
        assert_eq!(ids(&s.due_datagrams(t0, usize::MAX, |_| 0)), [[0]]);
        assert!(
            s.due_datagrams(t0 + COPY_AFTER - MS, usize::MAX, |_| 0)
                .is_empty()
        );
        assert_eq!(
            ids(&s.due_datagrams(t0 + COPY_AFTER, usize::MAX, |_| 0)),
            [[0]]
        );
        assert!(s.due_datagrams(t0 + 30 * MS, usize::MAX, |_| 0).is_empty());
        assert_eq!(s.resent(), 0, "a copy is not a resend");
        // The RTT sample is taken (from the first send).
        let before = s.rto();
        s.ack(0, t0 + 40 * MS);
        assert!(s.rto() < before, "{:?}", s.rto());
        // Acknowledged before COPY_AFTER: no copy.
        s.push_small('b');
        let t1 = t0 + 50 * MS;
        assert_eq!(s.due_datagrams(t1, usize::MAX, |_| 0).len(), 1);
        s.ack(1, t1 + 5 * MS);
        assert!(s.due_datagrams(t1 + 20 * MS, usize::MAX, |_| 0).is_empty());
    }

    #[test]
    fn new_small_messages_carry_the_oldest_unacknowledged() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.rtt_sample(200 * MS);
        for (k, c) in ['a', 'b', 'c'].into_iter().enumerate() {
            s.push_small(c);
            let sent = s.due_datagrams(t0 + k as u32 * 2 * MS, usize::MAX, |_| 0);
            let want: Vec<u32> = (0..=k as u32).collect();
            assert_eq!(ids(&sent), [want]);
        }
        s.ack(1, t0 + 5 * MS);
        s.push_small('d');
        assert_eq!(
            ids(&s.due_datagrams(t0 + 6 * MS, usize::MAX, |_| 0)),
            [[2, 3]]
        );
        // More new ones than a datagram takes: several datagrams, the room
        // left goes to the oldest earlier ones, all in id order.
        for _ in 0..MAX_GROUP + 2 {
            s.push_small('x');
        }
        let sent = s.due_datagrams(t0 + 8 * MS, usize::MAX, |_| 0);
        let all: Vec<u32> = ids(&sent).concat();
        assert_eq!(all, (2..2 + 2 + MAX_GROUP as u32 + 2).collect::<Vec<_>>());
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].len(), MAX_GROUP);
        assert!(sent.iter().all(|d| d.windows(2).all(|w| w[0].0 < w[1].0)));
    }

    #[test]
    fn other_messages_go_alone_and_are_not_copied() {
        let mut s = ReliableSender::new();
        let t0 = Instant::now();
        s.rtt_sample(200 * MS);
        s.push('p'); // a clipboard piece
        s.push_small('k');
        s.push('q');
        assert_eq!(
            ids(&s.due_datagrams(t0, usize::MAX, |_| 0)),
            vec![vec![0], vec![2], vec![1]]
        );
        s.push_small('m');
        // 'm' carries 'k' along, never 'p' or 'q'; at COPY_AFTER only the
        // small ones are copied ('k', with 'm' along).
        assert_eq!(
            ids(&s.due_datagrams(t0 + 2 * MS, usize::MAX, |_| 0)),
            [[1, 3]]
        );
        assert_eq!(
            ids(&s.due_datagrams(t0 + COPY_AFTER, usize::MAX, |_| 0)),
            [[1, 3]]
        );
        assert_eq!(
            ids(&s.due_datagrams(t0 + 2 * MS + COPY_AFTER, usize::MAX, |_| 0)),
            [[1, 3]]
        );
    }

    /// Lossy, duplicating, reordering link in both directions: every message
    /// arrives exactly once and in order.
    #[test]
    fn lossy_link_delivers_everything_in_order() {
        for (loss, small) in [0.01, 0.1, 0.3]
            .into_iter()
            .flat_map(|l| [(l, false), (l, true)])
        {
            let mut rng = Rng::new(7);
            let mut s = ReliableSender::new();
            let mut r = ReliableReceiver::new();
            let mut got = Vec::new();
            let t0 = Instant::now();
            let mut in_air: Vec<(u32, u32)> = Vec::new();
            let n = 5000u32;
            let mut step = 0u32;
            while got.len() < n as usize {
                let now = t0 + Duration::from_millis(u64::from(step));
                step += 1;
                if step <= n {
                    if small {
                        s.push_small(step - 1);
                    } else {
                        s.push(step - 1);
                    }
                }
                for d in s.due_datagrams(now, usize::MAX, |_| 0) {
                    if rng.next_f64() >= loss {
                        in_air.extend(&d);
                        if rng.next_f64() < 0.05 {
                            in_air.extend(&d); // duplicate
                        }
                    }
                }
                // Deliver in a shuffled order.
                for i in (1..in_air.len()).rev() {
                    in_air.swap(i, rng.below_inclusive(i as u64) as usize);
                }
                for (id, m) in in_air.drain(..) {
                    got.extend(r.accept(id, m));
                }
                if let Some(last) = r.last_delivered()
                    && rng.next_f64() >= loss
                {
                    s.ack(last, now);
                }
                assert!(step < n + 100_000, "stuck at loss {loss}");
            }
            assert_eq!(got, (0..n).collect::<Vec<_>>(), "loss {loss}");
            eprintln!(
                "loss {:.0} %, small {small}: {n} delivered in order, {} resent, {} duplicates dropped",
                loss * 100.0,
                s.resent(),
                r.duplicates()
            );
        }
    }
}
