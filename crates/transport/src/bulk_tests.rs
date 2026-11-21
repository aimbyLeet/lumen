use super::*;
use crate::emulator::{LinkConfig, LinkEmulator};
use protocol::FilePacket;

/// A transfer's bytes: a pattern that tells offsets apart.
fn byte_at(transfer: u32, offset: u64) -> u8 {
    (offset.wrapping_mul(2_654_435_761) >> 13) as u8 ^ transfer as u8
}

fn fill(transfer: u32, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = byte_at(transfer, offset + i as u64);
    }
    Ok(())
}

struct Run {
    secs: f64,
    /// Data bytes a second, at the receiver.
    mbps: f64,
    stats: BulkStats,
    duplicates: u64,
    /// When each transfer was done (all acknowledged), from the start.
    done_at: HashMap<u32, Duration>,
}

/// Sends `transfers` (id, len) over an emulated path, in steps of 100 µs of
/// virtual time, and checks every byte that arrives.
fn run(
    cfg: BulkConfig,
    link: LinkConfig,
    transfers: &[(u32, u64)],
    limit: Duration,
    cancel: Option<(Duration, u32)>,
) -> Run {
    run_later(cfg, link, transfers, &[], limit, cancel)
}

/// [`run`], and `later` (at, id, len) started at their time, the sender idle
/// in between if it is done by then.
fn run_later(
    cfg: BulkConfig,
    link: LinkConfig,
    transfers: &[(u32, u64)],
    later: &[(Duration, u32, u64)],
    limit: Duration,
    cancel: Option<(Duration, u32)>,
) -> Run {
    let t0 = Instant::now();
    let mut now = t0;
    let back = LinkConfig {
        loss: 0.0,
        bandwidth_bps: None,
        seed: link.seed ^ 1,
        ..link.clone()
    };
    let mut path = LinkEmulator::new(link);
    let mut ack_path = LinkEmulator::new(back);
    let mut tx = BulkSender::new(cfg, now);
    let mut rx = BulkReceiver::new();
    for &(id, len) in transfers {
        tx.start(id, len);
    }
    let all: Vec<(u32, u64)> = transfers
        .iter()
        .copied()
        .chain(later.iter().map(|&(_, id, len)| (id, len)))
        .collect();
    let mut got: HashMap<u32, Vec<bool>> = all
        .iter()
        .map(|&(id, len)| {
            (
                id,
                vec![false; len.div_ceil(FILE_CHUNK_LEN as u64) as usize],
            )
        })
        .collect();
    let mut cancel = cancel;
    let mut later: Vec<(Duration, u32, u64)> = later.to_vec();
    let mut done_at = HashMap::new();
    let step = Duration::from_micros(100);
    while !tx.idle() || !later.is_empty() {
        assert!(now - t0 < limit, "not done in {limit:?}: {:?}", tx.stats());
        later.retain(|&(at, id, len)| {
            let due = now - t0 >= at;
            if due {
                tx.start(id, len);
            }
            !due
        });
        for &(id, _) in &all {
            if tx
                .progress(id)
                .is_some_and(|p| p.state == TransferState::Done)
            {
                done_at.entry(id).or_insert(now - t0);
            }
        }
        if let Some((at, id)) = cancel
            && now - t0 >= at
        {
            tx.cancel(id);
            cancel = None;
        }
        // Virtual time: the read takes none of it, whatever the real clock says.
        while let Some(d) = tx.poll_timed(now, &mut |t, o, b| (fill(t, o, b), Duration::ZERO)) {
            let mut body = Vec::new();
            FilePacket::Data(d).encode(&mut body);
            path.send(now, body);
        }
        while let Some(p) = path.poll(now) {
            let FilePacket::Data(d) = FilePacket::decode(&p).unwrap() else {
                unreachable!()
            };
            if rx.on_data(now, &d) {
                for (i, &b) in d.data.iter().enumerate() {
                    assert_eq!(b, byte_at(d.transfer, d.offset + i as u64), "{d:?}");
                }
                let seen = &mut got.get_mut(&d.transfer).unwrap()
                    [(d.offset / FILE_CHUNK_LEN as u64) as usize];
                assert!(!*seen, "a chunk handed over twice: {d:?}");
                *seen = true;
            }
        }
        if rx.ack_due(now) {
            let mut body = Vec::new();
            FilePacket::Ack(rx.ack(now)).encode(&mut body);
            ack_path.send(now, body);
        }
        while let Some(p) = ack_path.poll(now) {
            let FilePacket::Ack(a) = FilePacket::decode(&p).unwrap() else {
                unreachable!()
            };
            tx.on_ack(now, &a);
        }
        if std::env::var_os("BULK_TRACE").is_some()
            && (now - t0).as_micros().is_multiple_of(100_000)
        {
            let st = tx.stats();
            println!(
                "{:6.1} s rate {:5.1} delivery {:5.1} srtt {:5.1} ms in flight {:7} resent {}",
                (now - t0).as_secs_f64(),
                st.rate_bps as f64 / 1e6,
                st.delivery_bps as f64 / 1e6,
                st.srtt.as_secs_f64() * 1e3,
                st.in_flight,
                st.resent
            );
        }
        now += step;
    }
    for &(id, len) in &all {
        let p = tx.progress(id).unwrap();
        if p.state == TransferState::Done {
            done_at.entry(id).or_insert(now - t0);
            assert_eq!(p.acked, len);
            assert!(got[&id].iter().all(|&g| g), "transfer {id}: chunks missing");
        }
    }
    let secs = (now - t0).as_secs_f64();
    Run {
        secs,
        mbps: rx.bytes() as f64 * 8.0 / secs / 1e6,
        stats: tx.stats(),
        duplicates: rx.duplicates(),
        done_at,
    }
}

fn path(mbps: u64, delay_ms: u64, loss: f64) -> LinkConfig {
    LinkConfig {
        loss,
        delay: Duration::from_millis(delay_ms),
        bandwidth_bps: Some(mbps * 1_000_000),
        queue_limit_bytes: 512 << 10,
        ..LinkConfig::default()
    }
}

#[test]
fn fills_a_narrow_path() {
    // 50 Mbit/s, 20 ms each way, cap 100: the path is what limits.
    let r = run(
        BulkConfig::default(),
        path(50, 20, 0.0),
        &[(1, 40 << 20)],
        Duration::from_secs(20),
        None,
    );
    println!("{:.2} s, {:.1} Mbit/s, {:?}", r.secs, r.mbps, r.stats);
    assert!(r.mbps > 0.85 * 50.0 * 0.96, "{:.1} Mbit/s", r.mbps);
}

#[test]
fn keeps_to_the_cap() {
    let cfg = BulkConfig {
        cap_bps: 30_000_000,
        ..BulkConfig::default()
    };
    let r = run(
        cfg,
        path(200, 10, 0.0),
        &[(1, 30 << 20)],
        Duration::from_secs(20),
        None,
    );
    println!("{:.2} s, {:.1} Mbit/s, {:?}", r.secs, r.mbps, r.stats);
    // The cap counts the wire: the data is ~6 % less.
    assert!(
        r.mbps < 30.0 && r.mbps > 30.0 * 0.85,
        "{:.1} Mbit/s",
        r.mbps
    );
}

#[test]
fn losses_cost_resends_not_speed() {
    for loss in [0.01, 0.05] {
        let r = run(
            BulkConfig::default(),
            LinkConfig {
                seed: 7,
                ..path(100, 20, loss)
            },
            &[(1, 40 << 20)],
            Duration::from_secs(20),
            None,
        );
        println!(
            "loss {loss}: {:.2} s, {:.1} Mbit/s, dup {}, {:?}",
            r.secs, r.mbps, r.duplicates, r.stats
        );
        assert!(
            r.mbps > 0.85 * 100.0 * 0.96 * (1.0 - loss),
            "{:.1} Mbit/s",
            r.mbps
        );
    }
}

#[test]
fn several_transfers_and_a_cancel() {
    let r = run(
        BulkConfig::default(),
        LinkConfig {
            seed: 3,
            ..path(100, 5, 0.02)
        },
        &[
            (1, 5 << 20),
            (2, 1),
            (3, 0),
            (4, 12 << 20),
            (5, FILE_CHUNK_LEN as u64 * 3),
        ],
        Duration::from_secs(20),
        Some((Duration::from_millis(300), 4)),
    );
    println!("{:.2} s, {:.1} Mbit/s, {:?}", r.secs, r.mbps, r.stats);
}

#[test]
fn a_transfer_after_a_pause_starts_as_fast() {
    // As copies through the clipboard go: one, a pause, the next (WG to the
    // owner: 100 Mbit/s, ~35 ms round trip).
    let r = run_later(
        BulkConfig::default(),
        LinkConfig {
            seed: 5,
            ..path(100, 17, 0.0)
        },
        &[(1, 5 << 20)],
        &[(Duration::from_secs(5), 2, 8 << 20)],
        Duration::from_secs(30),
        None,
    );
    let first = r.done_at[&1].as_secs_f64();
    let second = r.done_at[&2].as_secs_f64() - 5.0;
    println!("first {first:.2} s, second {second:.2} s, {:?}", r.stats);
    assert_eq!(r.stats.timeouts, 0, "a timeout out of nothing");
    // 8 MB on 100 Mbit/s: ~0.7 s once going, the start a few round trips.
    assert!(second < 1.5, "the second took {second:.2} s");
}

#[test]
fn small_copies_with_pauses_keep_their_speed() {
    // The owner's session (2026-10-27): small sets 7–17 s apart, then 8 MB
    // that took 14 s.
    let r = run_later(
        BulkConfig::default(),
        LinkConfig {
            seed: 9,
            ..path(100, 17, 0.0)
        },
        &[(1, 160 << 10)],
        &[
            (Duration::from_secs(17), 2, 1 << 20),
            (Duration::from_secs(25), 3, 270 << 10),
            (Duration::from_secs(38), 4, 22 << 10),
            (Duration::from_secs(59), 5, 8 << 20),
        ],
        Duration::from_secs(90),
        None,
    );
    let last = r.done_at[&5].as_secs_f64() - 59.0;
    println!("8 MB after the small ones: {last:.2} s, {:?}", r.stats);
    assert!(last < 1.5, "8 MB took {last:.2} s");
}

/// The path of the owner's session 2026-11-01: the round trip drifts between
/// 70 and 190 ms over seconds, whatever is sent; no losses.
fn drifting(mbps: u64) -> LinkConfig {
    LinkConfig {
        wander: Duration::from_millis(60),
        ..path(mbps, 35, 0.0)
    }
}

#[test]
fn a_round_trip_that_drifts_is_not_a_queue() {
    // Before: 11 -> 1 Mbit/s within seconds, 20 MB in minutes.
    // Fast drift (200 ms of round trip a second) looks like a queue half of the
    // time: slower, but not the bottom.
    for (period, least) in [(1, 5.0), (3, 40.0), (7, 40.0), (20, 40.0)] {
        let r = run(
            BulkConfig::default(),
            LinkConfig {
                wander_period: Duration::from_secs(period),
                ..drifting(100)
            },
            &[(1, 20 << 20)],
            Duration::from_secs(300),
            None,
        );
        println!(
            "drift over {period} s: {:.2} s, {:.1} Mbit/s, {:?}",
            r.secs, r.mbps, r.stats
        );
        assert!(r.mbps > least, "over {period} s: {:.1} Mbit/s", r.mbps);
    }
}

#[test]
fn a_drift_with_hiccups_is_not_a_queue() {
    // And the link stops for 100 ms every 2 s (Wi-Fi): delays come in steps.
    let r = run(
        BulkConfig::default(),
        LinkConfig {
            stall_every: Some(Duration::from_secs(2)),
            stall_len: Duration::from_millis(100),
            ..drifting(100)
        },
        &[(1, 20 << 20)],
        Duration::from_secs(300),
        None,
    );
    println!("{:.2} s, {:.1} Mbit/s, {:?}", r.secs, r.mbps, r.stats);
    assert!(r.mbps > 30.0, "{:.1} Mbit/s", r.mbps);
}

#[test]
fn a_drifting_path_is_still_followed() {
    // The narrow link is what limits, and nothing but the rate says so: the
    // queue must not stay full (it drops above 512 KiB: 200 ms at 20 Mbit/s).
    let r = run(
        BulkConfig::default(),
        drifting(20),
        &[(1, 30 << 20)],
        Duration::from_secs(120),
        None,
    );
    println!("{:.2} s, {:.1} Mbit/s, {:?}", r.secs, r.mbps, r.stats);
    assert!(r.mbps > 0.75 * 20.0 * 0.96, "{:.1} Mbit/s", r.mbps);
    let sent = r.stats.sent_bytes as f64 / (FILE_CHUNK_LEN + OVERHEAD) as f64;
    assert!(
        (r.stats.resent as f64) < sent * 0.03,
        "resent {} of {sent:.0}",
        r.stats.resent
    );
}

#[test]
fn cancel_states_and_floor() {
    let now = Instant::now();
    let mut tx = BulkSender::new(BulkConfig::default(), now);
    tx.start(1, 10 * FILE_CHUNK_LEN as u64);
    tx.start(2, 10 * FILE_CHUNK_LEN as u64);
    let mut sent = Vec::new();
    while let Some(d) = tx.poll(now, &mut fill) {
        sent.push(d);
    }
    assert!(!sent.is_empty());
    tx.cancel(1);
    assert_eq!(tx.progress(1).unwrap().state, TransferState::Cancelled);
    // Only transfer 2 is left; the next chunk's floor skips transfer 1's.
    let later = now + Duration::from_millis(50);
    let d = tx.poll(later, &mut fill).unwrap();
    assert_eq!(d.transfer, 2);
    let first_of_2 = sent.iter().find(|d| d.transfer == 2).unwrap().chunk;
    assert_eq!(d.floor, first_of_2);
}

#[test]
fn receiver_ranges() {
    let mut rx = BulkReceiver::new();
    let now = Instant::now();
    let chunk = |n: u64, floor: u64| FileData {
        chunk: n,
        floor,
        transfer: 1,
        offset: n * 10,
        sent_us: n as u32,
        data: vec![1; 10],
    };
    for n in [0, 1, 5, 6, 9, 3] {
        assert!(rx.on_data(now, &chunk(n, 0)));
    }
    assert!(!rx.on_data(now, &chunk(5, 0)), "a duplicate");
    let a = rx.ack(now);
    assert_eq!(a.below, 2);
    assert_eq!(a.ranges, [(3, 4), (5, 7), (9, 10)]);
    assert_eq!(a.gap, 0);
    assert_eq!(a.bytes, 60);
    // The sender gave up on 2 and 4: the floor moves `below` past them.
    assert!(rx.on_data(now, &chunk(7, 5)));
    let a = rx.ack(now);
    assert_eq!(a.below, 8);
    assert_eq!(a.ranges, [(9, 10)]);
    assert!(!rx.on_data(now, &chunk(4, 5)), "below the floor: old");
    // More ranges than fit: the lowest and the newest, the gap between.
    for n in (0..200).map(|i| 20 + 2 * i) {
        rx.on_data(now, &chunk(n, 0));
    }
    let a = rx.ack(now);
    assert_eq!(a.ranges.len(), MAX_ACK_RANGES);
    assert_eq!(a.gap as usize, ACK_LOW_RANGES);
    assert_eq!(a.ranges[0], (9, 10));
    assert_eq!(*a.ranges.last().unwrap(), (418, 419));
    assert!(a.ranges.windows(2).all(|w| w[0].1 < w[1].0));
    let mut body = Vec::new();
    FilePacket::Ack(a.clone()).encode(&mut body);
    assert_eq!(FilePacket::decode(&body).unwrap(), FilePacket::Ack(a));
}

/// The owner's session 2026-11-02: no chunk was lost in a set, 3305 were sent
/// again (the round trip jumped: all in flight looked lost), and the set took
/// 6 s instead of 2.6.
fn jumpy(mbps: u64, every_ms: u64, len_ms: u64) -> LinkConfig {
    LinkConfig {
        stall_every: Some(Duration::from_millis(every_ms)),
        stall_len: Duration::from_millis(len_ms),
        wander: Duration::from_millis(15),
        ..path(mbps, 19, 0.0)
    }
}

#[test]
fn a_round_trip_that_jumps_is_not_a_loss() {
    for (every, len) in [(1000, 60), (1000, 100), (2000, 150), (500, 80)] {
        let r = run(
            BulkConfig::default(),
            jumpy(70, every, len),
            &[(1, 20 << 20)],
            Duration::from_secs(60),
            None,
        );
        let chunks = (20u64 << 20).div_ceil(FILE_CHUNK_LEN as u64);
        println!(
            "{len} ms every {every} ms: {:.2} s, {:.1} Mbit/s, duplicates {} of {chunks}, resent {}",
            r.secs, r.mbps, r.duplicates, r.stats.resent
        );
        assert!(
            r.duplicates < chunks / 25,
            "{len} ms every {every} ms: {} duplicates of {chunks}",
            r.duplicates
        );
    }
}
