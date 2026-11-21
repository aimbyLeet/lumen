use std::time::{Duration, Instant};

use protocol::{AUTH_TAG_LEN, DEFAULT_MAX_DATAGRAM, Flags, PacketHeader, PacketKind};

use crate::fec::*;
use crate::rng::Rng;
use crate::{LinkConfig, LinkEmulator};

const SESSION: u32 = 7;
const TIMEOUT: Duration = Duration::from_millis(100);

fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    (0..len).map(|_| r.next_u64() as u8).collect()
}

fn packetize(p: &mut Packetizer, frame: u32, data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    p.packetize(
        PacketKind::Video,
        0,
        frame,
        if frame == 0 {
            Flags::KEYFRAME
        } else {
            Flags::RECOVERY
        },
        1000 + u64::from(frame),
        data,
        &mut out,
    )
    .unwrap();
    out
}

fn feed(r: &mut Reassembler, now: Instant, dgrams: &[Vec<u8>]) -> Vec<Delivery> {
    let mut out = Vec::new();
    for d in dgrams {
        let (h, body) = PacketHeader::decode(d).unwrap();
        r.push(now, &h, body, &mut out).unwrap();
    }
    out
}

fn complete(d: &Delivery) -> &Payload {
    match d {
        Delivery::Complete(p) => p,
        other => panic!("expected a frame, got {other:?}"),
    }
}

/// Splits datagrams of one payload into its FEC blocks.
fn blocks(dgrams: &[Vec<u8>]) -> Vec<Vec<(usize, u16, u16)>> {
    let mut out: Vec<Vec<(usize, u16, u16)>> = Vec::new();
    for (i, d) in dgrams.iter().enumerate() {
        let (_, body) = PacketHeader::decode(d).unwrap();
        let (f, _) = protocol::FragmentHeader::decode(body).unwrap();
        if out.len() <= usize::from(f.block) {
            out.resize(usize::from(f.block) + 1, Vec::new());
        }
        out[usize::from(f.block)].push((i, f.data_shards, f.parity_shards));
    }
    out
}

#[test]
fn roundtrip_sizes_without_loss() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    let sizes = [0, 1, 2, 1137, 1138, 1139, 4000, 300_000, 2_000_000];
    for (frame, &len) in sizes.iter().enumerate() {
        let data = payload(len, frame as u64);
        let dgrams = packetize(&mut p, frame as u32, &data);
        for d in &dgrams {
            assert!(
                d.len() + AUTH_TAG_LEN <= DEFAULT_MAX_DATAGRAM,
                "{}",
                d.len()
            );
        }
        let out = feed(&mut r, now, &dgrams);
        assert_eq!(out.len(), 1, "len {len}");
        let got = complete(&out[0]);
        assert_eq!(got.data, data, "len {len}");
        assert_eq!(got.frame, frame as u32);
        assert_eq!(got.keyframe, frame == 0);
        assert_eq!(got.recovery, frame != 0);
        assert_eq!(got.capture_time_us, 1000 + frame as u64);
        assert!(!got.recovered);
    }
    let s = r.stats();
    assert_eq!(s.frames_complete, sizes.len() as u64);
    assert_eq!((s.frames_lost, s.shards_restored, s.malformed), (0, 0, 0));
}

#[test]
fn large_payload_uses_several_blocks() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let dgrams = packetize(&mut p, 0, &payload(2_000_000, 1));
    let b = blocks(&dgrams);
    // 2 MB / 1138 B = 1758 shards -> 7 blocks of 251-252 data shards, 15 % parity.
    assert_eq!(b.len(), 7);
    for blk in &b {
        let (_, data, parity) = blk[0];
        assert!(data == 251 || data == 252, "{data}");
        assert_eq!(parity, (f64::from(data) * 0.15).ceil() as u16);
        assert_eq!(blk.len(), usize::from(data + parity));
    }
}

#[test]
fn restores_up_to_parity_losses_per_block() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let mut rng = Rng::new(3);
    let now = Instant::now();
    for frame in 0..20u32 {
        let data = payload(1 + rng.below_inclusive(400_000) as usize, u64::from(frame));
        let dgrams = packetize(&mut p, frame, &data);
        let mut drop = vec![false; dgrams.len()];
        for blk in blocks(&dgrams) {
            let parity = usize::from(blk[0].2);
            // Drop exactly `parity` random shards of the block (data or parity).
            let mut idx: Vec<usize> = blk.iter().map(|b| b.0).collect();
            for _ in 0..parity {
                let k = rng.below_inclusive(idx.len() as u64 - 1) as usize;
                drop[idx.swap_remove(k)] = true;
            }
        }
        let kept: Vec<Vec<u8>> = dgrams
            .into_iter()
            .zip(&drop)
            .filter(|(_, d)| !**d)
            .map(|(g, _)| g)
            .collect();
        let out = feed(&mut r, now, &kept);
        assert_eq!(out.len(), 1, "frame {frame}");
        assert_eq!(complete(&out[0]).data, data, "frame {frame}");
    }
    let s = r.stats();
    assert_eq!(s.frames_lost, 0);
    assert!(s.shards_restored > 0 && s.frames_recovered > 0);
}

#[test]
fn too_many_losses_reported_when_next_frame_completes() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    let f0 = packetize(&mut p, 0, &payload(10_000, 0));
    let f1 = packetize(&mut p, 1, &payload(10_000, 1));
    let f2 = packetize(&mut p, 2, &payload(10_000, 2));
    let f3 = packetize(&mut p, 3, &payload(10_000, 3));
    assert_eq!(feed(&mut r, now, &f0).len(), 1);
    // Frame 1: 9 data + 2 parity; lose 3.
    assert_eq!(f1.len(), 11);
    assert!(feed(&mut r, now, &f1[3..]).is_empty());
    // Frame 2 lost entirely; frame 3 completes.
    let _ = f2;
    let out = feed(&mut r, now, &f3);
    assert_eq!(
        out[0],
        Delivery::Lost {
            kind: PacketKind::Video,
            stream: 0,
            first: 1,
            last: 2
        }
    );
    assert_eq!(complete(&out[1]).frame, 3);
    // Late shards of frame 1 are stale now.
    assert!(feed(&mut r, now, &f1[..3]).is_empty());
    // Stale: 3 late shards of frame 1 + 2 unneeded parity shards each of frames 0 and 3.
    let s = r.stats();
    assert_eq!((s.frames_lost, s.frames_complete, s.stale), (2, 2, 7));
}

#[test]
fn a_hopeless_frame_is_reported_soon_not_at_the_next_frame() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let t0 = Instant::now();
    feed(&mut r, t0, &packetize(&mut p, 0, &payload(10_000, 0)));
    // Frame 1: 9 data + 2 parity. Shards 0..3 lost, 3 > 2 parity: hopeless once 3 arrives.
    let f1 = packetize(&mut p, 1, &payload(10_000, 1));
    assert!(feed(&mut r, t0, &f1[3..5]).is_empty());
    let mut out = Vec::new();
    r.expire(t0 + REORDER_GRACE - Duration::from_micros(1), &mut out);
    assert!(
        out.is_empty(),
        "within the grace a reordered shard may still come"
    );
    r.expire(t0 + REORDER_GRACE, &mut out);
    assert_eq!(
        out,
        vec![Delivery::Lost {
            kind: PacketKind::Video,
            stream: 0,
            first: 1,
            last: 1
        }]
    );
}

#[test]
fn a_reordered_shard_within_the_grace_saves_the_frame() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let t0 = Instant::now();
    let f0 = packetize(&mut p, 0, &payload(10_000, 0));
    // The tail of the block first, then the head 1 ms later.
    assert!(feed(&mut r, t0, &f0[4..]).is_empty());
    let out = feed(&mut r, t0 + Duration::from_millis(1), &f0[..4]);
    assert_eq!(complete(&out[0]).frame, 0);
    let mut out = Vec::new();
    r.expire(t0 + REORDER_GRACE, &mut out);
    assert!(out.is_empty());
}

#[test]
fn a_newer_frame_arriving_dooms_the_older_incomplete_one() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let t0 = Instant::now();
    feed(&mut r, t0, &packetize(&mut p, 0, &payload(10_000, 0)));
    // Frame 1's tail is lost (nothing below its highest shard is missing yet).
    let f1 = packetize(&mut p, 1, &payload(10_000, 1));
    let f2 = packetize(&mut p, 2, &payload(10_000, 2));
    assert!(feed(&mut r, t0, &f1[..5]).is_empty());
    let mut out = Vec::new();
    r.expire(t0 + REORDER_GRACE, &mut out);
    assert!(out.is_empty(), "the tail may still be on its way");
    // The first shard of frame 2 at t1: frame 1 is reported at t1 + grace.
    let t1 = t0 + Duration::from_millis(5);
    assert!(feed(&mut r, t1, &f2[..1]).is_empty());
    r.expire(t1 + REORDER_GRACE, &mut out);
    assert_eq!(
        out,
        vec![Delivery::Lost {
            kind: PacketKind::Video,
            stream: 0,
            first: 1,
            last: 1
        }]
    );
}

#[test]
fn incomplete_frame_expires() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let t0 = Instant::now();
    let f0 = packetize(&mut p, 0, &payload(10_000, 0));
    let f1 = packetize(&mut p, 1, &payload(10_000, 1));
    feed(&mut r, t0, &f0);
    feed(&mut r, t0, &f1[..2]);
    let mut out = Vec::new();
    r.expire(t0 + TIMEOUT, &mut out);
    assert!(out.is_empty());
    r.expire(t0 + TIMEOUT + Duration::from_millis(1), &mut out);
    assert_eq!(
        out,
        vec![Delivery::Lost {
            kind: PacketKind::Video,
            stream: 0,
            first: 1,
            last: 1
        }]
    );
}

#[test]
fn first_frame_loss_is_reported() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    let f0 = packetize(&mut p, 0, &payload(10_000, 0));
    let f1 = packetize(&mut p, 1, &payload(10_000, 1));
    feed(&mut r, now, &f0[..2]);
    let out = feed(&mut r, now, &f1);
    assert!(matches!(
        out[0],
        Delivery::Lost {
            first: 0,
            last: 0,
            ..
        }
    ));
    assert_eq!(complete(&out[1]).frame, 1);
}

#[test]
fn shuffled_datagrams_and_cursor_stream() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    let video = payload(50_000, 1);
    let cursor = payload(16_400, 2);
    let mut dgrams = packetize(&mut p, 0, &video);
    p.packetize(
        PacketKind::CursorShape,
        0,
        0,
        Flags::empty(),
        0,
        &cursor,
        &mut dgrams,
    )
    .unwrap();
    let mut rng = Rng::new(9);
    for i in (1..dgrams.len()).rev() {
        dgrams.swap(i, rng.below_inclusive(i as u64) as usize);
    }
    let out = feed(&mut r, now, &dgrams);
    assert_eq!(out.len(), 2);
    for d in &out {
        let got = complete(d);
        match got.kind {
            PacketKind::Video => assert_eq!(got.data, video),
            PacketKind::CursorShape => assert_eq!(got.data, cursor),
            PacketKind::Control | PacketKind::Handshake | PacketKind::Audio | PacketKind::File => {
                unreachable!()
            }
        }
    }
}

#[test]
fn foreign_session_and_inconsistent_fragments() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut other = Packetizer::new(FecConfig::default(), SESSION + 1).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    assert!(feed(&mut r, now, &packetize(&mut other, 0, &payload(5000, 0))).is_empty());
    assert_eq!(r.stats().stale, 6); // 5 data + 1 parity

    let a = packetize(&mut p, 0, &payload(5000, 0));
    let b = packetize(&mut p, 0, &payload(7000, 0)); // same frame, different total_len
    let mut out = Vec::new();
    let (h, body) = PacketHeader::decode(&a[0]).unwrap();
    r.push(now, &h, body, &mut out).unwrap();
    let (h, body) = PacketHeader::decode(&b[1]).unwrap();
    assert!(r.push(now, &h, body, &mut out).is_err());
    assert_eq!(r.stats().malformed, 1);
}

#[test]
fn no_parity_config() {
    let cfg = FecConfig {
        parity_ratio: 0.0,
        ..FecConfig::default()
    };
    let mut p = Packetizer::new(cfg, SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let data = payload(100_000, 5);
    let dgrams = packetize(&mut p, 0, &data);
    assert_eq!(dgrams.len(), 100_000usize.div_ceil(1138));
    assert_eq!(
        complete(&feed(&mut r, Instant::now(), &dgrams)[0]).data,
        data
    );
}

#[test]
fn bad_configs_rejected() {
    let bad = |f: fn(&mut FecConfig)| {
        let mut c = FecConfig::default();
        f(&mut c);
        Packetizer::new(c, 0).is_err()
    };
    assert!(bad(|c| c.max_shard_len = 1137));
    assert!(bad(|c| c.max_block_data = 0));
    assert!(bad(|c| c.parity_ratio = 1.5));
    assert!(bad(|c| c.parity_ratio = f64::NAN));
}

#[test]
fn fuzz_reassembler_never_panics() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut seeds = Vec::new();
    for f in 0..4 {
        seeds.extend(packetize(
            &mut p,
            f,
            &payload(3000 * (f as usize + 1), f.into()),
        ));
    }
    let mut rng = Rng::new(11);
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    let mut out = Vec::new();
    for _ in 0..30_000 {
        let mut d = seeds[rng.below_inclusive(seeds.len() as u64 - 1) as usize].clone();
        for _ in 0..1 + rng.below_inclusive(2) {
            let i = rng.below_inclusive(d.len() as u64 - 1) as usize;
            d[i] = rng.next_u64() as u8;
        }
        if let Ok((h, body)) = PacketHeader::decode(&d)
            && h.kind != PacketKind::Control
        {
            let _ = r.push(now, &h, body, &mut out);
        }
        if rng.below_inclusive(500) == 0 {
            r = Reassembler::new(SESSION, TIMEOUT);
        }
    }
}

/// 2000 frames of 4K-desktop-like sizes through the emulated link.
fn run_over_link(loss: f64, burst: f64, parity: f64, min_parity: u16) -> (u64, u64, u64) {
    let cfg = FecConfig {
        parity_ratio: parity,
        min_parity,
        ..FecConfig::default()
    };
    let mut p = Packetizer::new(cfg, SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let mut link = LinkEmulator::new(LinkConfig {
        loss,
        mean_burst_len: burst,
        seed: 42,
        ..LinkConfig::default()
    });
    let mut rng = Rng::new(1);
    let now = Instant::now();
    let mut sent = Vec::new();
    let mut delivered = 0;
    let mut dgrams = Vec::new();
    for frame in 0..2000u32 {
        // Mostly small P-frames, an occasional big update.
        let len = if frame % 100 == 0 {
            300_000
        } else {
            2_000 + rng.below_inclusive(60_000) as usize
        };
        let data = payload(len, u64::from(frame));
        dgrams.clear();
        p.packetize(
            PacketKind::Video,
            0,
            frame,
            Flags::empty(),
            0,
            &data,
            &mut dgrams,
        )
        .unwrap();
        sent.push(data);
        for d in dgrams.drain(..) {
            link.send(now, d);
        }
        let mut out = Vec::new();
        while let Some(d) = link.poll(now) {
            let (h, body) = PacketHeader::decode(&d).unwrap();
            r.push(now, &h, body, &mut out).unwrap();
        }
        for d in out {
            if let Delivery::Complete(pl) = d {
                assert_eq!(pl.data, sent[pl.frame as usize], "frame {}", pl.frame);
                delivered += 1;
            }
        }
    }
    let s = r.stats();
    eprintln!(
        "loss {:.0} % burst {burst} parity {:.0} % (min {min_parity}): delivered {delivered}/2000, recovered {}, lost {}, shards restored {}",
        loss * 100.0,
        parity * 100.0,
        s.frames_recovered,
        s.frames_lost,
        s.shards_restored
    );
    (delivered, s.frames_recovered, s.frames_lost)
}

#[test]
fn emulated_link_loss_rates() {
    assert_eq!(run_over_link(0.0, 1.0, 0.15, 1), (2000, 0, 0));
    let (d, rec, lost) = run_over_link(0.01, 1.0, 0.15, 1);
    assert!(rec > 0);
    assert!(lost <= 4, "1 % random loss: {lost} frames lost");
    assert_eq!(d + lost, 2000);
    // 5 % random and bursty: reported, asserted only for accounting.
    for (loss, burst, min_parity) in [
        (0.05, 1.0, 1),
        (0.05, 1.0, 3),
        (0.05, 4.0, 1),
        (0.05, 4.0, 3),
    ] {
        let (d, _, lost) = run_over_link(loss, burst, 0.15, min_parity);
        assert_eq!(d + lost, 2000);
    }
}

fn packetize_on(p: &mut Packetizer, stream: u8, frame: u32, data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    p.packetize(
        PacketKind::Video,
        stream,
        frame,
        Flags::empty(),
        0,
        data,
        &mut out,
    )
    .unwrap();
    out
}

#[test]
fn video_streams_are_reassembled_apart() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut r = Reassembler::new(SESSION, TIMEOUT);
    let now = Instant::now();
    // The same frame numbers on three streams, their datagrams interleaved.
    let frames: Vec<Vec<Vec<Vec<u8>>>> = (0..3u8)
        .map(|s| {
            (0..4u32)
                .map(|f| {
                    packetize_on(
                        &mut p,
                        s,
                        f,
                        &payload(20_000, u64::from(s) * 10 + u64::from(f)),
                    )
                })
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for f in 0..4 {
        // Round robin over the streams, one datagram at a time; stream 1 loses
        // frame 1 entirely: the loss shows on stream 1 only.
        let mut queues: Vec<std::collections::VecDeque<&Vec<u8>>> = frames
            .iter()
            .enumerate()
            .map(|(s, stream)| {
                if s == 1 && f == 1 {
                    Default::default()
                } else {
                    stream[f].iter().collect()
                }
            })
            .collect();
        while queues.iter().any(|q| !q.is_empty()) {
            for q in &mut queues {
                if let Some(d) = q.pop_front() {
                    let (h, body) = PacketHeader::decode(d).unwrap();
                    r.push(now, &h, body, &mut out).unwrap();
                }
            }
        }
    }
    let mut got: Vec<Vec<u32>> = vec![Vec::new(); 3];
    for d in &out {
        match d {
            Delivery::Complete(p) => {
                assert_eq!(
                    p.data,
                    payload(20_000, u64::from(p.stream) * 10 + u64::from(p.frame))
                );
                got[usize::from(p.stream)].push(p.frame);
            }
            Delivery::Lost {
                kind,
                stream,
                first,
                last,
            } => assert_eq!(
                (*kind, *stream, *first, *last),
                (PacketKind::Video, 1, 1, 1)
            ),
        }
    }
    assert_eq!(got, vec![vec![0, 1, 2, 3], vec![0, 2, 3], vec![0, 1, 2, 3]]);
    assert_eq!(r.stats().frames_lost, 1);

    // A stream started over: its frame numbers may begin anew.
    r.reset_stream(2);
    let again = packetize_on(&mut p, 2, 0, &payload(5_000, 99));
    let out = feed(&mut r, now, &again);
    assert_eq!((complete(&out[0]).stream, complete(&out[0]).frame), (2, 0));
    // Without the reset frame 0 of stream 0 is stale.
    assert!(feed(&mut r, now, &packetize_on(&mut p, 0, 0, &payload(5_000, 1))).is_empty());
}

#[test]
fn stream_out_of_range_is_refused() {
    let mut p = Packetizer::new(FecConfig::default(), SESSION).unwrap();
    let mut out = Vec::new();
    assert!(
        p.packetize(
            PacketKind::Video,
            protocol::MAX_STREAMS,
            0,
            Flags::empty(),
            0,
            b"x",
            &mut out
        )
        .is_err()
    );
}
