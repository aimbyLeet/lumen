//! lumen-netsim: end-to-end transport check on one machine.
//!
//! `stream.hevc` → access units at the stream frame rate → `Packetizer` (FEC)
//! → `Pacer` → `EmulatedSender` (loss / delay / bandwidth) → UDP loopback →
//! `Reassembler` → every delivered AU compared byte-for-byte with the original.
//! A lost frame is what would trigger a keyframe request in the client.
//! `--input` given several times: one video stream per file (monitors of the
//! host), their frames sent together, each checked on its own.

use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use protocol::{Flags, PacketHeader, PacketKind};
use transport::{
    Delivery, EmulatedSender, FecConfig, LinkConfig, LinkStats, Pacer, PacerConfig, Packetizer,
    Reassembler, ReassemblyStats, SOCKET_BUFFER, UdpLink, percentile, sleep_until,
};

#[derive(Parser)]
#[command(about = "Send a recorded HEVC stream through the transport over an emulated link")]
struct Args {
    /// Recorded stream; several times: one video stream each (protocol v10).
    #[arg(long, default_value = "bench-out/stream.hevc")]
    input: Vec<std::path::PathBuf>,
    /// Frame rate the access units are sent at.
    #[arg(long, default_value_t = 60.0)]
    fps: f64,
    /// Packet loss, percent.
    #[arg(long, default_value_t = 0.0)]
    loss: f64,
    /// Mean loss burst length in packets (1 = independent losses).
    #[arg(long, default_value_t = 1.0)]
    burst: f64,
    /// One-way delay, ms.
    #[arg(long, default_value_t = 0.0)]
    delay_ms: f64,
    /// Bottleneck bandwidth, Mbit/s (0 = unlimited).
    #[arg(long, default_value_t = 250.0)]
    link_mbps: f64,
    /// Pacing rate, Mbit/s; defaults to 95 % of the link.
    #[arg(long)]
    pace_mbps: Option<f64>,
    /// FEC parity, percent of data shards per block.
    #[arg(long, default_value_t = 15.0)]
    parity: f64,
    #[arg(long, default_value_t = 1)]
    min_parity: u16,
    /// Frame timeout on the receiver, ms.
    #[arg(long, default_value_t = 100)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Write delivered AUs here (equals the input when nothing was lost); with
    /// several streams, stream `n` goes to `<out>` with the extension `n`.
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Instead of the stream: a file of this many MiB through the bulk
    /// transport (file transfers), compared byte for byte.
    #[arg(long)]
    file_mb: Option<u64>,
    /// With --file-mb: the sender's cap, Mbit/s (the server's file speed).
    #[arg(long, default_value_t = 100.0)]
    cap_mbps: f64,
    /// With --file-mb: synthetic 60 fps video of this many Mbit/s on the same
    /// link (2 s alone, during the transfer, 2 s after).
    #[arg(long, default_value_t = 0.0)]
    video_mbps: f64,
    /// Bottleneck queue, KiB.
    #[arg(long, default_value_t = 4096)]
    queue_kb: usize,
}

mod files;

pub(crate) const SESSION: u32 = 1;

struct Received {
    stream: u8,
    frame: u32,
    data: Vec<u8>,
    recovered: bool,
    latency_us: u64,
}

/// Stream, first and last frame of each gap.
type ReceiverResult = (Vec<Received>, Vec<(u8, u32, u32)>, ReassemblyStats);

pub(crate) fn free_port() -> Result<SocketAddr> {
    let s = std::net::UdpSocket::bind("127.0.0.1:0")?;
    Ok(s.local_addr()?)
}

fn receive(
    link: UdpLink,
    t0: Instant,
    timeout: Duration,
    done: mpsc::Receiver<()>,
) -> Result<ReceiverResult> {
    let mut r = Reassembler::new(SESSION, timeout);
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::new();
    let mut got = Vec::new();
    let mut lost = Vec::new();
    let mut quiet_since: Option<Instant> = None;
    let mut sender_done = false;
    loop {
        match link.recv_timeout(&mut buf, Duration::from_millis(5))? {
            Some(n) => {
                quiet_since = None;
                let (h, body) = PacketHeader::decode(&buf[..n])?;
                let _ = r.push(Instant::now(), &h, body, &mut out);
            }
            None => {
                sender_done |= done.try_recv().is_ok();
                // Once the sender is done, stop after the link stays quiet past the timeout.
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if sender_done && since.elapsed() > timeout * 2 {
                    break;
                }
            }
        }
        r.expire(Instant::now(), &mut out);
        for d in out.drain(..) {
            match d {
                Delivery::Complete(p) => got.push(Received {
                    stream: p.stream,
                    frame: p.frame,
                    data: p.data,
                    recovered: p.recovered,
                    latency_us: (t0.elapsed().as_micros() as u64).saturating_sub(p.capture_time_us),
                }),
                Delivery::Lost {
                    stream,
                    first,
                    last,
                    ..
                } => lost.push((stream, first, last)),
            }
        }
    }
    Ok((got, lost, r.stats()))
}

fn link_config(args: &Args, link_bps: u64) -> LinkConfig {
    LinkConfig {
        loss: args.loss / 100.0,
        mean_burst_len: args.burst,
        delay: Duration::from_secs_f64(args.delay_ms / 1e3),
        bandwidth_bps: (link_bps > 0).then_some(link_bps),
        queue_limit_bytes: args.queue_kb << 10,
        seed: args.seed,
        ..LinkConfig::default()
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let link_bps = (args.link_mbps * 1e6) as u64;
    if let Some(file_mb) = args.file_mb {
        return files::run(files::Options {
            file_mb,
            cap_mbps: args.cap_mbps,
            video_mbps: args.video_mbps,
            link: link_config(&args, link_bps),
            dir: "bench-out".into(),
        });
    }
    ensure!(
        args.input.len() <= usize::from(protocol::MAX_STREAMS),
        "at most {} streams",
        protocol::MAX_STREAMS
    );
    let files = read_inputs(&args)?;
    let streams: Vec<Vec<&[u8]>> = files
        .iter()
        .map(|d| decoder::annexb::split_access_units(d))
        .collect();
    ensure!(streams.iter().all(|s| !s.is_empty()), "no access units");
    let frames = streams.iter().map(Vec::len).max().unwrap_or(0);

    let (rx_addr, tx_addr) = (free_port()?, free_port()?);
    let rx_link = UdpLink::bind_connect(rx_addr, tx_addr, SOCKET_BUFFER)?;
    let tx_link = UdpLink::bind_connect(tx_addr, rx_addr, SOCKET_BUFFER)?;

    let sender = EmulatedSender::new(tx_link, link_config(&args, link_bps));

    let t0 = Instant::now();
    let timeout = Duration::from_millis(args.timeout_ms);
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || receive(rx_link, t0, timeout, done_rx));

    let sent = send_streams(&args, &sender, &streams, frames, link_bps, t0)?;
    // Let the emulated link drain (delay + queue) before stopping the receiver.
    thread::sleep(Duration::from_secs_f64(args.delay_ms / 1e3) + Duration::from_millis(200));
    let link = sender.stats();
    drop(sender);
    let _ = done_tx.send(());
    let (got, lost, stats) = receiver.join().expect("receiver thread")?;

    print_overview(&args, &streams, frames, &sent, &link, &got, &stats);
    check_streams(&args, &files, &streams, &got, &lost)
}

fn read_inputs(args: &Args) -> Result<Vec<Vec<u8>>> {
    args.input
        .iter()
        .map(|p| std::fs::read(p).with_context(|| format!("read {p:?}")))
        .collect()
}

/// What the sender side did: the pacing rate, the totals and the duration.
struct Sent {
    pace_bps: u64,
    bytes: u64,
    datagrams: u64,
    secs: f64,
}

/// Sender: one AU per frame interval, released by the pacer.
fn send_streams(
    args: &Args,
    sender: &EmulatedSender,
    streams: &[Vec<&[u8]>],
    frames: usize,
    link_bps: u64,
    t0: Instant,
) -> Result<Sent> {
    let mut packetizer = Packetizer::new(
        FecConfig {
            parity_ratio: args.parity / 100.0,
            min_parity: args.min_parity,
            ..FecConfig::default()
        },
        SESSION,
    )?;
    let pace_bps = match args.pace_mbps {
        Some(m) => (m * 1e6) as u64,
        None if link_bps > 0 => link_bps * 95 / 100,
        None => 10_000_000_000,
    };
    let mut pacer = Pacer::new(PacerConfig {
        rate_bps: pace_bps,
        burst_bytes: 64 << 10,
    });
    let interval = Duration::from_secs_f64(1.0 / args.fps);
    let mut dgrams = Vec::new();
    let (mut sent_bytes, mut sent_datagrams) = (0u64, 0u64);
    let start = Instant::now();
    let mut next_frame = 0usize;
    loop {
        let now = Instant::now();
        let due = start + interval.mul_f64(next_frame as f64);
        if next_frame < frames && now >= due {
            // Every stream's frame of this interval at once: the worst case for the queue.
            for (s, aus) in streams.iter().enumerate() {
                let Some(au) = aus.get(next_frame) else {
                    continue;
                };
                dgrams.clear();
                packetizer.packetize(
                    PacketKind::Video,
                    s as u8,
                    next_frame as u32,
                    if next_frame == 0 {
                        Flags::KEYFRAME
                    } else {
                        Flags::empty()
                    },
                    t0.elapsed().as_micros() as u64,
                    au,
                    &mut dgrams,
                )?;
                for d in dgrams.drain(..) {
                    pacer.push_lane(s, d);
                }
            }
            next_frame += 1;
            continue;
        }
        while let Some(d) = pacer.poll(now) {
            sent_bytes += d.len() as u64;
            sent_datagrams += 1;
            sender.send(d);
        }
        let release = pacer.next_release(now);
        let frame_due = (next_frame < frames).then_some(due);
        match (frame_due, release) {
            (None, None) => break,
            (Some(a), Some(b)) => sleep_until(a.min(b)),
            (Some(t), None) | (None, Some(t)) => sleep_until(t),
        }
    }
    let send_secs = start.elapsed().as_secs_f64();
    Ok(Sent {
        pace_bps,
        bytes: sent_bytes,
        datagrams: sent_datagrams,
        secs: send_secs,
    })
}

/// Latency percentiles of the delivered frames of one stream (or of all).
fn latency_summary(got: &[Received], s: Option<u8>) -> String {
    let mut l: Vec<f64> = got
        .iter()
        .filter(|g| s.is_none_or(|s| g.stream == s))
        .map(|g| g.latency_us as f64 / 1e3)
        .collect();
    l.sort_by(f64::total_cmp);
    format!(
        "p50 {:.2}  p95 {:.2}  p99 {:.2}  max {:.2}",
        percentile(&l, 0.5),
        percentile(&l, 0.95),
        percentile(&l, 0.99),
        l.last().copied().unwrap_or(f64::NAN)
    )
}

/// The totals of the run: input, link, sender and receiver.
fn print_overview(
    args: &Args,
    streams: &[Vec<&[u8]>],
    frames: usize,
    sent: &Sent,
    link: &LinkStats,
    got: &[Received],
    stats: &ReassemblyStats,
) {
    let (pace_bps, sent_bytes, sent_datagrams, send_secs) =
        (sent.pace_bps, sent.bytes, sent.datagrams, sent.secs);
    let payload: usize = streams.iter().flatten().map(|a| a.len()).sum();
    println!(
        "streams:   {} ({} AUs at most), {:.1} MB, {:.1} Mbit/s at {} fps",
        streams.len(),
        frames,
        payload as f64 / 1e6,
        payload as f64 * 8.0 / (frames as f64 / args.fps) / 1e6,
        args.fps
    );
    println!(
        "link:      loss {} % (burst {}), delay {} ms, {} Mbit/s; pacing {:.0} Mbit/s; FEC {} % (min {})",
        args.loss,
        args.burst,
        args.delay_ms,
        args.link_mbps,
        pace_bps as f64 / 1e6,
        args.parity,
        args.min_parity
    );
    println!(
        "sent:      {sent_datagrams} datagrams, {:.1} MB ({:+.1} % over payload), {:.1} Mbit/s over {send_secs:.1} s",
        sent_bytes as f64 / 1e6,
        (sent_bytes as f64 / payload as f64 - 1.0) * 100.0,
        sent_bytes as f64 * 8.0 / send_secs / 1e6
    );
    println!(
        "link:      dropped by loss {}, by queue {}",
        link.dropped_loss, link.dropped_queue
    );
    println!(
        "received:  {} frames, {} shards restored by FEC; latency send -> reassembled, ms: {}",
        got.len(),
        stats.shards_restored,
        latency_summary(got, None)
    );
}

/// Per stream: compare what arrived with the input, print the verdict, write
/// `--out`; fails when any stream is corrupted, out of order or has
/// unaccounted frames.
fn check_streams(
    args: &Args,
    files: &[Vec<u8>],
    streams: &[Vec<&[u8]>],
    got: &[Received],
    lost: &[(u8, u32, u32)],
) -> Result<()> {
    let mut bad = Vec::new();
    for (s, aus) in streams.iter().enumerate() {
        let sid = s as u8;
        let mine: Vec<&Received> = got.iter().filter(|g| g.stream == sid).collect();
        let gaps: Vec<_> = lost.iter().filter(|l| l.0 == sid).collect();
        let lost_frames: u32 = gaps.iter().map(|(_, a, b)| b - a + 1).sum();
        let mismatched = mine
            .iter()
            .filter(|g| aus.get(g.frame as usize) != Some(&&g.data[..]))
            .count();
        let in_order = mine.windows(2).all(|w| w[0].frame < w[1].frame);
        let recovered = mine.iter().filter(|g| g.recovered).count();
        let bytes: Vec<u8> = mine.iter().flat_map(|g| g.data.iter().copied()).collect();
        let verdict = if bytes == files[s] {
            "IDENTICAL to input"
        } else {
            "differs from input (frames lost)"
        };
        println!(
            "stream {s}:  {:?}: {} of {} frames ({recovered} recovered by FEC), {lost_frames} lost in {} gaps -> {} keyframe requests; {mismatched} differ; {}; {verdict}; latency {}",
            args.input[s],
            mine.len(),
            aus.len(),
            gaps.len(),
            gaps.len(),
            if in_order { "in order" } else { "OUT OF ORDER" },
            latency_summary(got, Some(sid))
        );
        if let Some(path) = &args.out {
            let path = if streams.len() == 1 {
                path.clone()
            } else {
                path.with_extension(format!("{s}"))
            };
            std::fs::write(&path, &bytes)?;
        }
        if mismatched > 0 || !in_order || mine.len() + lost_frames as usize != aus.len() {
            bad.push(s);
        }
    }
    ensure!(
        bad.is_empty(),
        "streams {bad:?}: corrupted, out of order or unaccounted frames"
    );
    Ok(())
}
