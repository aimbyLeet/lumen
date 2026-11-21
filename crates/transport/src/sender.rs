//! Background sender: frame datagrams go through a [`Pacer`], control
//! datagrams bypass it and leave immediately.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::net::UdpLink;
use crate::pacing::{Pacer, PacerConfig};

/// Longest sleep between looks at the channel; channel waits are avoided
/// because their timeouts follow the coarse system tick on Windows.
const NAP: Duration = Duration::from_millis(1);
/// How late a short sleep wakes: Windows wakes a sleeping thread on its timer's
/// 0.5 ms tick (asking for 100-450 µs here woke 230-680 µs late, p50).
const OS_TICK: Duration = Duration::from_micros(600);

/// The bucket the pacer gets: the burst asked for, but at least what the rate
/// earns in one [`OS_TICK`], since the sender sleeps between releases instead
/// of spinning (spinning to each release of 8 KiB bursts at 150 Mbit/s took
/// ~28 % of a core on the host).
fn bucket(cfg: PacerConfig) -> PacerConfig {
    let per_tick = (cfg.rate_bps as u128 * OS_TICK.as_nanos() / 8 / 1_000_000_000) as usize;
    PacerConfig {
        rate_bps: cfg.rate_bps,
        burst_bytes: cfg.burst_bytes.max(per_tick + 1500),
    }
}

/// Numbers (and seals) a paced datagram as it leaves.
pub type SealPaced = Box<dyn FnMut(&mut Vec<u8>) + Send>;

enum Msg {
    /// The datagrams of one frame of video stream (pacer lane) `.0`.
    Paced(u8, Vec<Vec<u8>>),
    Now(Vec<u8>),
    SetRate(u64),
    SetPacing {
        rate_bps: u64,
        burst_bytes: usize,
    },
    Seal(SealPaced),
    /// The sender is dropped: stop even if a [`NowSender`] keeps the channel.
    Quit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SenderStats {
    pub datagrams: u64,
    pub bytes: u64,
    pub send_errors: u64,
    /// Datagrams still waiting in the pacer at the last update.
    pub queued: usize,
}

pub struct PacedSender {
    tx: Option<Sender<Msg>>,
    stats: Arc<Mutex<SenderStats>>,
    thread: Option<JoinHandle<()>>,
}

impl PacedSender {
    pub fn new(link: UdpLink, cfg: PacerConfig) -> Self {
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(Mutex::new(SenderStats::default()));
        let st = stats.clone();
        let thread = thread::Builder::new()
            .name("paced-sender".into())
            .spawn(move || run(link, cfg, rx, st))
            .expect("spawn paced-sender");
        PacedSender {
            tx: Some(tx),
            stats,
            thread: Some(thread),
        }
    }

    /// Queues the datagrams of one frame behind the pacer.
    pub fn send_paced(&self, datagrams: Vec<Vec<u8>>) {
        self.post(Msg::Paced(0, datagrams));
    }

    /// Queues the datagrams of a frame of video stream `stream`: the pacer
    /// takes the streams in turn, a datagram each.
    pub fn send_paced_stream(&self, stream: u8, datagrams: Vec<Vec<u8>>) {
        self.post(Msg::Paced(stream, datagrams));
    }

    /// A handle for another thread to queue frames (as `send_paced_stream`).
    pub fn paced_sender(&self) -> PacedHandle {
        PacedHandle(self.tx.clone())
    }

    /// Sends a control datagram ahead of any queued frame data.
    pub fn send_now(&self, datagram: Vec<u8>) {
        self.post(Msg::Now(datagram));
    }

    pub fn set_rate(&self, rate_bps: u64) {
        self.post(Msg::SetRate(rate_bps));
    }

    /// Rate and the largest back-to-back burst for frame data from now on.
    pub fn set_pacing(&self, rate_bps: u64, burst_bytes: usize) {
        self.post(Msg::SetPacing {
            rate_bps,
            burst_bytes,
        });
    }

    /// Paced datagrams are numbered and sealed by `f` as they leave, in the
    /// order they leave (they are queued unsealed, sequence number 0).
    pub fn seal_paced_with(&self, f: SealPaced) {
        self.post(Msg::Seal(f));
    }

    pub fn stats(&self) -> SenderStats {
        *self.stats.lock().unwrap()
    }

    /// A handle for another thread to send datagrams ahead of the queue.
    pub fn now_sender(&self) -> NowSender {
        NowSender(self.tx.clone())
    }

    fn post(&self, m: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(m);
        }
    }
}

/// Queues frames on a [`PacedSender`] from another thread (a monitor's
/// encoder); does nothing once the sender is gone.
#[derive(Clone)]
pub struct PacedHandle(Option<Sender<Msg>>);

impl PacedHandle {
    pub fn send_paced_stream(&self, stream: u8, datagrams: Vec<Vec<u8>>) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(Msg::Paced(stream, datagrams));
        }
    }

    /// Ahead of the queue (control datagrams, cursor).
    pub fn send_now(&self, datagram: Vec<u8>) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(Msg::Now(datagram));
        }
    }
}

/// Sends through a [`PacedSender`] ahead of its queue (as `send_now`), from
/// any thread; does nothing once the sender is gone.
#[derive(Clone)]
pub struct NowSender(Option<Sender<Msg>>);

impl NowSender {
    pub fn send(&self, datagram: Vec<u8>) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(Msg::Now(datagram));
        }
    }
}

impl Drop for PacedSender {
    fn drop(&mut self) {
        self.post(Msg::Quit);
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(link: UdpLink, cfg: PacerConfig, rx: Receiver<Msg>, stats: Arc<Mutex<SenderStats>>) {
    // What was asked for: the bucket follows from it.
    let mut asked = cfg;
    let mut pacer = Pacer::new(bucket(asked));
    let mut local = SenderStats::default();
    let mut seal: Option<SealPaced> = None;
    let send = |d: &[u8], local: &mut SenderStats| match link.send(d) {
        Ok(()) => {
            local.datagrams += 1;
            local.bytes += d.len() as u64;
        }
        Err(_) => local.send_errors += 1,
    };
    let handle = |m: Msg,
                  pacer: &mut Pacer,
                  asked: &mut PacerConfig,
                  local: &mut SenderStats,
                  seal: &mut Option<SealPaced>| {
        let new = match m {
            Msg::Seal(f) => return *seal = Some(f),
            Msg::Paced(lane, ds) => {
                return ds
                    .into_iter()
                    .for_each(|d| pacer.push_lane(usize::from(lane), d));
            }
            Msg::Now(d) => return send(&d, local),
            Msg::Quit => return,
            Msg::SetRate(rate_bps) => PacerConfig { rate_bps, ..*asked },
            Msg::SetPacing {
                rate_bps,
                burst_bytes,
            } => PacerConfig {
                rate_bps,
                burst_bytes,
            },
        };
        *asked = new;
        let b = bucket(new);
        let now = Instant::now();
        pacer.set_rate_at(now, b.rate_bps);
        pacer.set_burst_at(now, b.burst_bytes);
    };
    loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Quit) => return,
                Ok(m) => handle(m, &mut pacer, &mut asked, &mut local, &mut seal),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let now = Instant::now();
        while let Some(mut d) = pacer.poll(now) {
            if let Some(f) = seal.as_mut() {
                f(&mut d);
            }
            send(&d, &mut local);
        }
        local.queued = pacer.queued_packets();
        *stats.lock().unwrap() = local;

        match pacer.next_release(now) {
            // Idle: block until there is something to send.
            None => match rx.recv() {
                Ok(Msg::Quit) | Err(_) => return,
                Ok(m) => handle(m, &mut pacer, &mut asked, &mut local, &mut seal),
            },
            // Wakes up to an OS tick late; the bucket holds what it earns meanwhile.
            Some(t) => thread::sleep(t.saturating_duration_since(Instant::now()).min(NAP)),
        }
    }
}
