//! UDP over loopback/LAN with precise timing helpers, plus a sender that routes
//! datagrams through a [`LinkEmulator`] so loopback tests see loss and delay.

use std::cell::Cell;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::emulator::{LinkConfig, LinkEmulator, LinkStats};

/// The size asked for SO_RCVBUF/SO_SNDBUF of a session's socket.
pub const SOCKET_BUFFER: usize = 8 << 20;

/// Remaining time below which `sleep_until` spins instead of asking the OS.
const SPIN_MARGIN: Duration = Duration::from_micros(1500);

/// Sleeps until `deadline` with sub-100 µs accuracy: OS sleep for the bulk,
/// spin for the tail. Returns immediately if the deadline has passed.
pub fn sleep_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let rem = deadline - now;
        if rem > SPIN_MARGIN {
            thread::sleep(rem - SPIN_MARGIN);
        } else {
            std::hint::spin_loop();
        }
    }
}

/// A connected UDP socket.
pub struct UdpLink {
    sock: UdpSocket,
    recv_buf: usize,
    send_buf: usize,
    /// Last SO_RCVTIMEO set, to skip the syscall when it doesn't change.
    read_timeout: Cell<Option<Duration>>,
}

impl UdpLink {
    /// Binds `local`, connects to `peer`, requests `buffer_bytes` for both
    /// SO_RCVBUF and SO_SNDBUF. The OS may grant a different size; see
    /// [`recv_buffer_granted`](Self::recv_buffer_granted).
    pub fn bind_connect(
        local: SocketAddr,
        peer: SocketAddr,
        buffer_bytes: usize,
    ) -> io::Result<Self> {
        let link = Self::bind(local, buffer_bytes)?;
        link.sock.connect(peer)?;
        Ok(link)
    }

    /// Binds `local` without a peer (a host waiting for its client). Use
    /// [`wait_for_peer`](Self::wait_for_peer) to accept the first sender.
    pub fn bind(local: SocketAddr, buffer_bytes: usize) -> io::Result<Self> {
        let s = Socket::new(Domain::for_address(local), Type::DGRAM, Some(Protocol::UDP))?;
        s.set_recv_buffer_size(buffer_bytes)?;
        s.set_send_buffer_size(buffer_bytes)?;
        s.bind(&local.into())?;
        let recv_buf = s.recv_buffer_size()?;
        let send_buf = s.send_buffer_size()?;
        Ok(Self {
            sock: s.into(),
            recv_buf,
            send_buf,
            read_timeout: Cell::new(None),
        })
    }

    /// On an unconnected link: waits up to `timeout` for a datagram for which
    /// `accept` returns true, then connects to its sender. Returns the sender
    /// and the datagram length (in `buf`). Other datagrams are dropped.
    pub fn wait_for_peer(
        &self,
        buf: &mut [u8],
        timeout: Duration,
        mut accept: impl FnMut(&[u8]) -> bool,
    ) -> io::Result<Option<(SocketAddr, usize)>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.sock.set_read_timeout(Some(left))?;
            self.read_timeout.set(Some(left));
            match self.sock.recv_from(buf) {
                Ok((n, from)) if accept(&buf[..n]) => {
                    self.sock.connect(from)?;
                    return Ok(Some((from, n)));
                }
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// On an unconnected link: sends to `peer`.
    pub fn send_to(&self, datagram: &[u8], peer: SocketAddr) -> io::Result<()> {
        self.sock.send_to(datagram, peer).map(|_| ())
    }

    /// On an unconnected link: the next datagram and its sender, `Ok(None)`
    /// after `timeout` (ICMP errors from closed ports are skipped).
    pub fn recv_from_timeout(
        &self,
        buf: &mut [u8],
        timeout: Duration,
    ) -> io::Result<Option<(usize, SocketAddr)>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let left = left.max(Duration::from_millis(1));
            self.sock.set_read_timeout(Some(left))?;
            self.read_timeout.set(Some(left));
            match self.sock.recv_from(buf) {
                Ok(r) => return Ok(Some(r)),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                // An expired SO_RCVTIMEO reported as ERROR_IO_PENDING (997).
                Err(e) if cfg!(windows) && e.raw_os_error() == Some(997) => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// From now on only `peer`: `send` goes there, other senders are dropped.
    pub fn connect(&self, peer: SocketAddr) -> io::Result<()> {
        self.sock.connect(peer)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// SO_RCVBUF as reported by the OS after the request (Linux reports double).
    pub fn recv_buffer_granted(&self) -> usize {
        self.recv_buf
    }

    /// SO_SNDBUF as reported by the OS after the request.
    pub fn send_buffer_granted(&self) -> usize {
        self.send_buf
    }

    pub fn send(&self, datagram: &[u8]) -> io::Result<()> {
        self.sock.send(datagram).map(|_| ())
    }

    /// `Ok(None)` on timeout. The timeout is rounded up to whole milliseconds
    /// (at least 1 ms). On Windows, ICMP-induced `ConnectionReset`
    /// errors (peer port not yet open) are ignored and the wait continues.
    pub fn recv_timeout(&self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
        let deadline = Instant::now() + timeout;
        loop {
            // Whole milliseconds (the Windows granularity) so a constant
            // timeout hits the cache above instead of a syscall per call.
            let left_us = deadline
                .saturating_duration_since(Instant::now())
                .as_micros();
            let left = Duration::from_millis(left_us.div_ceil(1000).max(1) as u64);
            if self.read_timeout.get() != Some(left) {
                self.sock.set_read_timeout(Some(left))?;
                self.read_timeout.set(Some(left));
            }
            match self.sock.recv(buf) {
                Ok(n) => return Ok(Some(n)),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                // Windows occasionally reports an expired SO_RCVTIMEO as
                // ERROR_IO_PENDING (997) instead of WSAETIMEDOUT.
                Err(e) if cfg!(windows) && e.raw_os_error() == Some(997) => {
                    return Ok(None);
                }
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Another handle to the same socket (e.g. one thread sends, one receives).
    pub fn try_clone(&self) -> io::Result<UdpLink> {
        Ok(UdpLink {
            sock: self.sock.try_clone()?,
            recv_buf: self.recv_buf,
            send_buf: self.send_buf,
            read_timeout: Cell::new(None),
        })
    }
}

/// Sends datagrams through a [`LinkEmulator`] on a background thread, then out
/// of a [`UdpLink`]. Dropping it stops and joins the thread; packets still in
/// flight are discarded.
pub struct EmulatedSender {
    tx: Option<Sender<(Instant, Vec<u8>)>>,
    stats: Arc<Mutex<LinkStats>>,
    thread: Option<JoinHandle<()>>,
}

impl EmulatedSender {
    /// # Panics
    /// Panics if `cfg` is invalid (see `LinkConfig::validate`).
    pub fn new(link: UdpLink, cfg: LinkConfig) -> Self {
        let emu = LinkEmulator::new(cfg);
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(Mutex::new(LinkStats::default()));
        let st = stats.clone();
        let thread = thread::Builder::new()
            .name("emulated-sender".into())
            .spawn(move || run_emulated(emu, link, rx, st))
            .expect("spawn emulated-sender");
        Self {
            tx: Some(tx),
            stats,
            thread: Some(thread),
        }
    }

    /// Timestamps the datagram with `Instant::now()` and hands it to the thread.
    pub fn send(&self, datagram: Vec<u8>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((Instant::now(), datagram));
        }
    }

    /// Link counters as of the thread's last wake-up.
    pub fn stats(&self) -> LinkStats {
        *self.stats.lock().unwrap()
    }
}

impl Drop for EmulatedSender {
    fn drop(&mut self) {
        self.tx = None; // disconnects the channel, waking the thread
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_emulated(
    mut emu: LinkEmulator,
    link: UdpLink,
    rx: mpsc::Receiver<(Instant, Vec<u8>)>,
    stats: Arc<Mutex<LinkStats>>,
) {
    // Senders on several threads may enqueue slightly out of timestamp order;
    // the emulator needs monotonic time.
    let mut last_ts: Option<Instant> = None;
    let mut feed = |emu: &mut LinkEmulator, ts: Instant, p: Vec<u8>| {
        let ts = last_ts.map_or(ts, |l| l.max(ts));
        last_ts = Some(ts);
        emu.send(ts, p);
    };
    loop {
        loop {
            match rx.try_recv() {
                Ok((ts, p)) => feed(&mut emu, ts, p),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let now = Instant::now();
        while let Some(p) = emu.poll(now) {
            let _ = link.send(&p);
        }
        *stats.lock().unwrap() = emu.stats();

        match emu.next_delivery() {
            None => match rx.recv() {
                Ok((ts, p)) => feed(&mut emu, ts, p),
                Err(_) => return,
            },
            Some(t) => {
                let rem = t.saturating_duration_since(Instant::now());
                if rem > SPIN_MARGIN {
                    // Channel timeouts follow the coarse system tick on Windows
                    // (~15.6 ms); `thread::sleep` is high-resolution. Nap in short
                    // steps and pick up new packets at the top of the loop.
                    thread::sleep((rem - SPIN_MARGIN).min(Duration::from_millis(1)));
                } else {
                    sleep_until(t);
                }
            }
        }
    }
}
