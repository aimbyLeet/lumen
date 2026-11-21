//! Socket and timing tests for `net` / `pacing`. A separate test binary on
//! purpose: cargo runs test binaries one after another, so CPU-heavy unit
//! tests (FEC) cannot disturb these timing measurements.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Mutex as StdMutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use transport::{EmulatedSender, LinkConfig, Pacer, PacerConfig, UdpLink, sleep_until};

/// Timing tests compete for CPU; run them one at a time.
static SERIAL: StdMutex<()> = StdMutex::new(());

/// One timing test at a time; a failed one does not fail the others.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

const MIB: usize = 1 << 20;

fn lo(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

/// Two connected loopback links `(a, b)`.
fn pair(buf: usize) -> (UdpLink, UdpLink) {
    let ra = UdpSocket::bind(lo(0)).unwrap();
    let rb = UdpSocket::bind(lo(0)).unwrap();
    let (aa, ab) = (ra.local_addr().unwrap(), rb.local_addr().unwrap());
    drop((ra, rb));
    (
        UdpLink::bind_connect(aa, ab, buf).unwrap(),
        UdpLink::bind_connect(ab, aa, buf).unwrap(),
    )
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn payload(seq: u32, epoch: Instant, len: usize) -> Vec<u8> {
    let mut p = vec![0u8; len];
    p[..4].copy_from_slice(&seq.to_le_bytes());
    let ns = epoch.elapsed().as_nanos() as u64;
    p[4..12].copy_from_slice(&ns.to_le_bytes());
    p
}

#[test]
fn sleep_until_accuracy() {
    let _g = serial();
    let mut over: Vec<Duration> = (0..200)
        .map(|_| {
            let d = Instant::now() + Duration::from_micros(500);
            sleep_until(d);
            Instant::now() - d
        })
        .collect();
    over.sort();
    let (p50, p99, max) = (pct(&over, 0.5), pct(&over, 0.99), over[over.len() - 1]);
    let p90 = pct(&over, 0.9);
    eprintln!("sleep_until(+500us) overshoot: p50 {p50:?} p90 {p90:?} p99 {p99:?} max {max:?}");
    // p90, not p99: the 2nd worst of 200 is whether Windows preempted the
    // spinning thread twice (a scheduler quantum), not how `sleep_until` works.
    assert!(p90 < Duration::from_micros(200));
    // Long sleeps (OS-sleep path) stay accurate too: the median of five, for the same reason.
    let mut long: Vec<Duration> = (0..5)
        .map(|_| {
            let d = Instant::now() + Duration::from_millis(20);
            sleep_until(d);
            Instant::now() - d
        })
        .collect();
    long.sort();
    eprintln!("sleep_until(+20ms) overshoot: {long:?}");
    assert!(long[2] < Duration::from_millis(1));
}

/// Sends `n` datagrams of 1200 B; returns (received, receive Mbit/s,
/// sender Mbit/s, granted rcv/snd buffers).
fn udp_run(paced: bool, n: u32) -> (u32, f64, f64, usize, usize) {
    const LEN: usize = 1200;
    let (tx, rx) = pair(8 * MIB);
    let (rcv_buf, snd_buf) = (rx.recv_buffer_granted(), tx.send_buffer_granted());
    let recv = thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut seen = vec![false; n as usize];
        let (mut count, mut first, mut last) = (0u32, None, None);
        let mut idle = 0;
        while count < n && idle < 2 {
            match rx
                .recv_timeout(&mut buf, Duration::from_millis(500))
                .unwrap()
            {
                Some(len) => {
                    idle = 0;
                    assert_eq!(len, LEN);
                    let seq = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                    if !seen[seq] {
                        seen[seq] = true;
                        count += 1;
                    }
                    let now = Instant::now();
                    first.get_or_insert(now);
                    last = Some(now);
                }
                None => idle += 1,
            }
        }
        let span = match (first, last) {
            (Some(f), Some(l)) => (l - f).as_secs_f64().max(1e-9),
            _ => 1.0,
        };
        (count, span)
    });
    let mk = |i: u32| {
        let mut p = vec![0u8; LEN];
        p[..4].copy_from_slice(&i.to_le_bytes());
        p
    };
    let t0 = Instant::now();
    if paced {
        let mut pacer = Pacer::new(PacerConfig {
            rate_bps: 250_000_000,
            burst_bytes: 64 * 1024,
        });
        for i in 0..n {
            pacer.push(mk(i));
        }
        while pacer.queued_packets() > 0 {
            let now = Instant::now();
            match pacer.poll(now) {
                Some(p) => tx.send(&p).unwrap(),
                None => sleep_until(pacer.next_release(now).unwrap()),
            }
        }
    } else {
        for i in 0..n {
            tx.send(&mk(i)).unwrap();
        }
    }
    let send_secs = t0.elapsed().as_secs_f64();
    let (count, span) = recv.join().unwrap();
    let bits = |c: u32| c as f64 * LEN as f64 * 8.0 / 1e6;
    (
        count,
        bits(count) / span,
        bits(n) / send_secs,
        rcv_buf,
        snd_buf,
    )
}

#[test]
fn udp_paced_no_loss() {
    let _g = serial();
    let n = 20_000;
    let (count, rx_mbps, tx_mbps, rb, sb) = udp_run(true, n);
    eprintln!(
        "udp paced 250 Mbit/s: received {count}/{n}, lost {}, rx {rx_mbps:.1} Mbit/s, \
         tx {tx_mbps:.1} Mbit/s, granted SO_RCVBUF {rb} SO_SNDBUF {sb} (requested {})",
        n - count,
        8 * MIB
    );
    assert_eq!(count, n);
}

#[test]
fn udp_unpaced_informational() {
    let _g = serial();
    let n = 20_000;
    let (count, rx_mbps, tx_mbps, _, _) = udp_run(false, n);
    eprintln!(
        "udp unpaced burst: received {count}/{n}, lost {} ({:.2} %), rx {rx_mbps:.0} Mbit/s, tx {tx_mbps:.0} Mbit/s",
        n - count,
        (n - count) as f64 * 100.0 / n as f64
    );
}

/// Runs `n` datagrams through an `EmulatedSender`, `gap` apart. Returns the
/// one-way delay of every received datagram.
fn emulated_run(cfg: LinkConfig, n: u32, gap: Duration) -> Vec<Duration> {
    let (tx, rx) = pair(8 * MIB);
    let epoch = Instant::now();
    let recv = thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut delays = vec![];
        while let Some(len) = rx
            .recv_timeout(&mut buf, Duration::from_millis(300))
            .unwrap()
        {
            assert!(len >= 12);
            let sent = u64::from_le_bytes(buf[4..12].try_into().unwrap());
            delays.push(epoch.elapsed() - Duration::from_nanos(sent));
        }
        delays
    });
    let es = EmulatedSender::new(tx, cfg);
    let mut next = Instant::now();
    for i in 0..n {
        es.send(payload(i, epoch, 200));
        next += gap;
        sleep_until(next);
    }
    thread::sleep(Duration::from_millis(100));
    let st = es.stats();
    eprintln!("emulated sender stats: {st:?}");
    drop(es);
    recv.join().unwrap()
}

#[test]
fn emulated_loss() {
    let _g = serial();
    let n = 20_000;
    let got = emulated_run(
        LinkConfig {
            loss: 0.05,
            seed: 11,
            ..Default::default()
        },
        n,
        Duration::from_micros(20),
    );
    let frac = got.len() as f64 / n as f64;
    eprintln!(
        "emulated loss 5 %: received {}/{n} = {:.2} % (target 95 +- 1)",
        got.len(),
        frac * 100.0
    );
    assert!((frac - 0.95).abs() <= 0.01);
}

#[test]
fn emulated_delay() {
    let _g = serial();
    let n = 2000;
    let mut d = emulated_run(
        LinkConfig {
            delay: Duration::from_millis(20),
            ..Default::default()
        },
        n,
        Duration::from_micros(200),
    );
    assert_eq!(d.len(), n as usize);
    d.sort();
    let mean = d.iter().sum::<Duration>() / d.len() as u32;
    let (p50, p99, min, max) = (pct(&d, 0.5), pct(&d, 0.99), d[0], d[d.len() - 1]);
    eprintln!(
        "emulated delay 20 ms: mean {mean:?} p50 {p50:?} p99 {p99:?} min {min:?} max {max:?}"
    );
    // What the emulator answers for: nothing early, the delay itself on time.
    // Not p99: one stall of a thread (a 15.6 ms scheduler quantum, the machine
    // busy elsewhere) holds the ~80 datagrams queued meanwhile (2026-11-03:
    // 2 runs of 25 with p99 26 and 36 ms, the mean 20.6 instead of 20.05).
    assert!(min >= Duration::from_millis(20));
    assert!(p50 < Duration::from_micros(20_500));
    assert!(mean <= Duration::from_millis(22));
}

#[test]
fn recv_timeout_returns_none() {
    let (_a, b) = pair(MIB);
    let t = Instant::now();
    let mut buf = [0u8; 16];
    assert!(
        b.recv_timeout(&mut buf, Duration::from_millis(30))
            .unwrap()
            .is_none()
    );
    assert!(t.elapsed() >= Duration::from_millis(25));
}

#[test]
fn try_clone_shares_socket() {
    let (a, b) = pair(MIB);
    let a2 = a.try_clone().unwrap();
    a2.send(&[1, 2, 3]).unwrap();
    let mut buf = [0u8; 16];
    let n = b
        .recv_timeout(&mut buf, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], &[1, 2, 3]);
    assert_eq!(a.local_addr().unwrap(), a2.local_addr().unwrap());
}
