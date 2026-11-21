//! `lumen-netsim --file-mb N`: a file through the bulk transport
//! (`transport::BulkSender`, protocol v8) over the emulated link, written on
//! the other side and compared byte for byte; with `--video-mbps`, synthetic
//! 60 fps video shares the link (what a transfer does to the picture).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use protocol::{FilePacket, Flags, PacketHeader, PacketKind};
use transport::{
    BulkConfig, BulkReceiver, BulkSender, BulkStats, Delivery, EmulatedSender, FecConfig,
    LinkConfig, Pacer, PacerConfig, Packetizer, Reassembler, SOCKET_BUFFER, TransferState, UdpLink,
    percentile,
};

use crate::{SESSION, free_port};

pub struct Options {
    pub file_mb: u64,
    pub cap_mbps: f64,
    pub video_mbps: f64,
    pub link: LinkConfig,
    pub dir: PathBuf,
}

const TRANSFER: u32 = 1;

fn datagram(kind: PacketKind, seq: u64, body: &FilePacket) -> Vec<u8> {
    let mut d = Vec::with_capacity(protocol::DEFAULT_MAX_DATAGRAM);
    PacketHeader {
        kind,
        flags: Flags::empty(),
        stream: 0,
        session: SESSION,
        seq,
    }
    .encode(&mut d);
    body.encode(&mut d);
    d
}

/// A file of `mb` MiB of random-looking bytes (made once, kept).
fn source(dir: &Path, mb: u64) -> Result<PathBuf> {
    let path = dir.join(format!("netsim-file-{mb}.bin"));
    if path.metadata().is_ok_and(|m| m.len() == mb << 20) {
        return Ok(path);
    }
    let mut f = std::io::BufWriter::new(File::create(&path)?);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut block = vec![0u8; 1 << 20];
    for _ in 0..mb {
        for b in block.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b.copy_from_slice(&x.to_le_bytes()[..b.len()]);
        }
        f.write_all(&block)?;
    }
    f.flush()?;
    Ok(path)
}

fn same_files(a: &Path, b: &Path) -> Result<bool> {
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

struct VideoResult {
    /// When each frame was whole, and its latency, ms.
    frames: Vec<(Instant, f64)>,
    /// When frames were found lost, and how many.
    lost: Vec<(Instant, u32)>,
}

/// The receiving side: file chunks written at their offsets, acks back
/// (through `acks`), video frames reassembled.
fn receive(
    link: UdpLink,
    acks: Arc<EmulatedSender>,
    out: PathBuf,
    len: u64,
    t0: Instant,
    stop: Arc<AtomicBool>,
) -> Result<(u64, u64, VideoResult)> {
    let mut f = File::create(&out)?;
    f.set_len(len)?;
    let mut rx = BulkReceiver::new();
    let mut video = Reassembler::new(SESSION, Duration::from_millis(100));
    let mut delivered = Vec::new();
    let mut vr = VideoResult {
        frames: Vec::new(),
        lost: Vec::new(),
    };
    let mut buf = vec![0u8; 2048];
    let mut seq = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let wait = rx
            .next_ack()
            .map_or(Duration::from_millis(5), |t| {
                t.saturating_duration_since(Instant::now())
            })
            .min(Duration::from_millis(5));
        if let Some(n) = link.recv_timeout(&mut buf, wait)? {
            let now = Instant::now();
            let (h, body) = PacketHeader::decode(&buf[..n])?;
            match h.kind {
                PacketKind::File => {
                    if let FilePacket::Data(d) = FilePacket::decode(body)?
                        && rx.on_data(now, &d)
                    {
                        f.seek(SeekFrom::Start(d.offset))?;
                        f.write_all(&d.data)?;
                    }
                }
                PacketKind::Video => {
                    let _ = video.push(now, &h, body, &mut delivered);
                }
                _ => {}
            }
        }
        let now = Instant::now();
        if rx.ack_due(now) {
            seq += 1;
            acks.send(datagram(
                PacketKind::File,
                seq,
                &FilePacket::Ack(rx.ack(now)),
            ));
        }
        video.expire(now, &mut delivered);
        for d in delivered.drain(..) {
            match d {
                Delivery::Complete(p) => vr.frames.push((
                    now,
                    (t0.elapsed().as_micros() as u64).saturating_sub(p.capture_time_us) as f64
                        / 1e3,
                )),
                Delivery::Lost { first, last, .. } => vr.lost.push((now, last - first + 1)),
            }
        }
    }
    f.flush()?;
    Ok((rx.bytes(), rx.duplicates(), vr))
}

/// Synthetic video: a frame of `mbps / 60` bytes every 1/60 s, FEC 15 %,
/// paced like the host (150 Mbit/s, 8 KB bursts), until `stop`.
fn video(link: Arc<EmulatedSender>, mbps: f64, t0: Instant, stop: Arc<AtomicBool>) -> Result<()> {
    let mut p = Packetizer::new(FecConfig::default(), SESSION)?;
    let mut pacer = Pacer::new(PacerConfig {
        rate_bps: 150_000_000,
        burst_bytes: 14 << 10,
    });
    let frame_len = (mbps * 1e6 / 8.0 / 60.0) as usize;
    let payload: Vec<u8> = (0..frame_len).map(|i| (i * 7) as u8).collect();
    let interval = Duration::from_secs_f64(1.0 / 60.0);
    let start = Instant::now();
    let mut frame = 0u32;
    let mut out = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let due = start + interval * frame;
        if now >= due {
            out.clear();
            p.packetize(
                PacketKind::Video,
                0,
                frame,
                Flags::empty(),
                t0.elapsed().as_micros() as u64,
                &payload,
                &mut out,
            )?;
            out.drain(..).for_each(|d| pacer.push(d));
            frame += 1;
        }
        while let Some(d) = pacer.poll(now) {
            link.send(d);
        }
        thread::sleep(Duration::from_micros(500));
    }
    Ok(())
}

pub fn run(o: Options) -> Result<()> {
    let len = o.file_mb << 20;
    let src = source(&o.dir, o.file_mb.max(1))?;
    let out = o.dir.join("netsim-file.out");

    let (rx_addr, tx_addr) = (free_port()?, free_port()?);
    let rx_link = UdpLink::bind_connect(rx_addr, tx_addr, SOCKET_BUFFER)?;
    let tx_link = UdpLink::bind_connect(tx_addr, rx_addr, SOCKET_BUFFER)?;
    let tx_recv = tx_link.try_clone()?;
    let rx_send = rx_link.try_clone()?;
    let ack_link = LinkConfig {
        loss: 0.0,
        bandwidth_bps: None,
        seed: o.link.seed ^ 1,
        ..o.link.clone()
    };
    let path = Arc::new(EmulatedSender::new(tx_link, o.link.clone()));
    let acks = Arc::new(EmulatedSender::new(rx_send, ack_link));

    let t0 = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let receiver = {
        let (acks, stop, out) = (acks.clone(), stop.clone(), out.clone());
        thread::spawn(move || receive(rx_link, acks, out, len, t0, stop))
    };
    let video_thread = (o.video_mbps > 0.0).then(|| {
        let (path, stop) = (path.clone(), stop.clone());
        let mbps = o.video_mbps;
        thread::spawn(move || video(path, mbps, t0, stop))
    });
    // Acks for the sender, from its socket.
    let (ack_tx, ack_rx) = mpsc::channel();
    let ack_reader = {
        let stop = stop.clone();
        thread::spawn(move || -> Result<()> {
            let mut buf = vec![0u8; 2048];
            while !stop.load(Ordering::Relaxed) {
                if let Some(n) = tx_recv.recv_timeout(&mut buf, Duration::from_millis(5))? {
                    let (_, body) = PacketHeader::decode(&buf[..n])?;
                    if let FilePacket::Ack(a) = FilePacket::decode(body)? {
                        let _ = ack_tx.send((Instant::now(), a));
                    }
                }
            }
            Ok(())
        })
    };

    // Video alone first: the baseline it is compared with.
    if o.video_mbps > 0.0 {
        thread::sleep(Duration::from_secs(2));
    }
    let mut file = File::open(&src)?;
    let mut read = |_: u32, offset: u64, buf: &mut [u8]| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)
    };
    let started = Instant::now();
    let mut tx = BulkSender::new(
        BulkConfig {
            cap_bps: (o.cap_mbps * 1e6) as u64,
            ..BulkConfig::default()
        },
        started,
    );
    tx.start(TRANSFER, len);
    let mut seq = 0u64;
    let mut trace: Vec<(f64, BulkStats)> = Vec::new();
    let mut next_trace = started;
    while !tx.idle() {
        for (at, a) in ack_rx.try_iter() {
            tx.on_ack(at, &a);
        }
        let now = Instant::now();
        while let Some(d) = tx.poll(now, &mut read) {
            seq += 1;
            path.send(datagram(PacketKind::File, seq, &FilePacket::Data(d)));
        }
        if now >= next_trace {
            trace.push((now.duration_since(started).as_secs_f64(), tx.stats()));
            next_trace += Duration::from_millis(500);
        }
        ensure!(
            now.duration_since(started) < Duration::from_secs(600),
            "not done in 10 min: {:?}",
            tx.stats()
        );
        let wake = tx
            .next_poll(now)
            .map_or(Duration::from_millis(1), |t| {
                t.saturating_duration_since(now)
            })
            .min(Duration::from_millis(1));
        thread::sleep(wake);
    }
    let secs = started.elapsed().as_secs_f64();
    let stats = tx.stats();
    let state = tx.progress(TRANSFER).map(|p| p.state);
    if o.video_mbps > 0.0 {
        // Video alone again after the transfer.
        thread::sleep(Duration::from_secs(2));
    }
    thread::sleep(o.link.delay * 2 + Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    let link_stats = path.stats();
    let (got, dup, vr) = receiver.join().expect("receiver")?;
    if let Some(v) = video_thread {
        v.join().expect("video")?;
    }
    ack_reader.join().expect("ack reader")?;

    for (t, s) in &trace {
        println!(
            "{t:6.1} s  rate {:6.1}  arrives {:6.1} Mbit/s  rtt {:5.1} ms (base {:5.1})  in flight {:4} KB  resent {}",
            s.rate_bps as f64 / 1e6,
            s.delivery_bps as f64 / 1e6,
            s.srtt.as_secs_f64() * 1e3,
            s.base_rtt.as_secs_f64() * 1e3,
            s.in_flight / 1024,
            s.resent
        );
    }
    println!(
        "link:      loss {:.1} % (burst {}), delay {} ms, {} Mbit/s; cap {} Mbit/s; video {} Mbit/s",
        o.link.loss * 100.0,
        o.link.mean_burst_len,
        o.link.delay.as_millis(),
        o.link
            .bandwidth_bps
            .map_or("unlimited".into(), |b| format!("{}", b / 1_000_000)),
        o.cap_mbps,
        o.video_mbps
    );
    println!(
        "file:      {} MiB in {secs:.2} s = {:.1} Mbit/s of data; state {state:?}",
        o.file_mb,
        len as f64 * 8.0 / secs / 1e6
    );
    println!(
        "sender:    resent {} chunks, {} timeouts, {:.1} MB on the wire; receiver: {:.1} MB, {dup} duplicates",
        stats.resent,
        stats.timeouts,
        stats.sent_bytes as f64 / 1e6,
        got as f64 / 1e6
    );
    println!(
        "link:      dropped by loss {}, by queue {}",
        link_stats.dropped_loss, link_stats.dropped_queue
    );
    if o.video_mbps > 0.0 {
        let ended = started + Duration::from_secs_f64(secs);
        for (name, from, to) in [
            ("alone", t0, started),
            ("+ file", started, ended),
            ("after", ended, ended + Duration::from_secs(10)),
        ] {
            let mut lat: Vec<f64> = vr
                .frames
                .iter()
                .filter(|(t, _)| (from..to).contains(t))
                .map(|&(_, l)| l)
                .collect();
            lat.sort_by(f64::total_cmp);
            let lost: u32 = vr
                .lost
                .iter()
                .filter(|(t, _)| (from..to).contains(t))
                .map(|&(_, n)| n)
                .sum();
            println!(
                "video {name:7} {} frames, {lost} lost; send -> whole, ms: p50 {:.1}  p95 {:.1}  p99 {:.1}  max {:.1}",
                lat.len(),
                percentile(&lat, 0.5),
                percentile(&lat, 0.95),
                percentile(&lat, 0.99),
                lat.last().copied().unwrap_or(f64::NAN)
            );
        }
    }
    ensure!(state == Some(TransferState::Done), "transfer {state:?}");
    let same = same_files(&src, &out).context("compare")?;
    println!(
        "integrity: {}",
        if same {
            "IDENTICAL to the source"
        } else {
            "DIFFERS"
        }
    );
    ensure!(same, "the file differs");
    Ok(())
}
