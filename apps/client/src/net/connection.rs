//! The render thread's end of the session: the decode queue, input and
//! requests to the network thread, the wake event.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use decoder::Decoder;
use protocol::{ControlMessage, InputEvent, VideoFormat};
use transport::{Payload, SOCKET_BUFFER, UdpLink, wall_clock_us};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

use super::handshake::Handshake;
use super::thread::{NetShared, network};
use super::{Counters, CursorEvent, Decoded, HostMonitors, Options};

/// Upper bound of access units decoded per `decode_pending` call: if decoding
/// falls behind the network, the render loop still gets to present and to pump
/// window messages (otherwise the window hangs).
const MAX_DECODES_PER_CALL: usize = 4;
/// An access unit waiting longer than this to be decoded: its stream's queue
/// is dropped and a keyframe asked for. Without a bound the queue grew while
/// the render loop fell behind (17 minutes of frames, 1.2 GB, 2026-11-13) and
/// only a reconnection brought the picture back to now.
const QUEUE_LIMIT: Duration = Duration::from_millis(500);

/// The streams (bit per stream) with an access unit waiting longer than `limit`.
fn stale_streams(waiting: impl Iterator<Item = (u8, Duration)>, limit: Duration) -> u8 {
    waiting
        .filter(|&(_, waited)| waited > limit)
        .fold(0, |m, (stream, _)| {
            m | 1 << (usize::from(stream) % protocol::MAX_STREAMS as usize)
        })
}

pub(super) struct ToDecode {
    pub(super) payload: Payload,
    pub(super) reassembled_us: u64,
    /// When it was queued (monotonic: the wall clock may step).
    pub(super) queued: Instant,
}

pub struct Connection {
    pub format: VideoFormat,
    /// The host's address that answered.
    pub peer: SocketAddr,
    input: Sender<InputEvent>,
    pub cursor: Receiver<CursorEvent>,
    /// Unreliable one-off requests to the host (e.g. a missing cursor shape).
    requests: Sender<ControlMessage>,
    wake: Wake,
    access_units: Receiver<ToDecode>,
    /// Received, not decoded yet (how far behind the render thread is).
    backlog: Mutex<VecDeque<ToDecode>>,
    pub counters: Arc<Mutex<Counters>>,
    /// A bit per stream set on a decode error; the network thread then
    /// requests an IDR of that stream.
    decode_failed: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    /// The host's monitors as it said last (`Monitors`: generation, list).
    pub monitors: HostMonitors,
    /// The monitors the window shows, a bit each (0: all): the host encodes those.
    pub shown: Arc<AtomicU8>,
    threads: Vec<JoinHandle<()>>,
}

impl Connection {
    /// Sends `Hello` to every address of the host until one answers (the
    /// handshake proves it is the host), then only to that one until
    /// `StreamStart`, then starts the threads.
    pub fn open(hosts: &[SocketAddr], timeout: Duration, opts: Options) -> Result<Self> {
        let Some(first) = hosts.first() else {
            bail!("the host has no address");
        };
        let local = SocketAddr::new(
            match first.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            },
            0,
        );
        let link = UdpLink::bind(local, SOCKET_BUFFER)?;
        let hosts: Vec<SocketAddr> = hosts
            .iter()
            .copied()
            .filter(|h| h.is_ipv4() == first.is_ipv4())
            .collect();
        let shaken = Handshake::new(&opts)?.run(&link, &hosts, timeout, opts.sim_loss)?;

        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Mutex::new(Counters::default()));
        let (to_decode_tx, to_decode_rx) = mpsc::channel::<ToDecode>();
        let decode_failed = Arc::new(AtomicU8::new(0));
        let monitors = Arc::new(Mutex::new(None));
        let shown = Arc::new(AtomicU8::new(opts.show_monitors.unwrap_or(0)));
        let (input_tx, input_rx) = mpsc::channel::<InputEvent>();
        let (cursor_tx, cursor_rx) = mpsc::channel::<CursorEvent>();
        let (request_tx, request_rx) = mpsc::channel::<ControlMessage>();
        let wake = Wake::new()?;

        let net = {
            let shared = NetShared {
                out: to_decode_tx,
                inputs: input_rx,
                cursor: cursor_tx,
                requests: request_rx,
                wake: wake.clone_handle(),
                counters: counters.clone(),
                decode_failed: decode_failed.clone(),
                stop: stop.clone(),
                monitors: monitors.clone(),
                shown: shown.clone(),
            };
            let (session, sealer, opener) = (shaken.session, shaken.sealer, shaken.opener);
            let seq = shaken.seq;
            thread::Builder::new()
                .name("client-net".into())
                .spawn(move || network(link, session, shared, opts, sealer, opener, seq))?
        };
        Ok(Connection {
            format: shaken.format,
            peer: shaken.peer,
            input: input_tx,
            cursor: cursor_rx,
            requests: request_tx,
            wake,
            access_units: to_decode_rx,
            backlog: Mutex::new(VecDeque::new()),
            counters,
            decode_failed,
            stop,
            monitors,
            shown,
            threads: vec![net],
        })
    }

    /// Decodes the access units received so far (at most `MAX_DECODES_PER_CALL`
    /// a stream), in order, each with its stream's decoder (`decoders[stream]`;
    /// none: dropped), and returns the newest picture of each stream, how many
    /// older ones were decoded but superseded, and how many access units still
    /// wait. [`Pick`] says when to stop. It wakes the render loop for what is
    /// left (not for what waits behind a held stream). `each` sees every decoded
    /// picture. Call on the render thread (see the module docs).
    pub fn decode_pending(
        &self,
        decoders: &mut [Option<Decoder>],
        fresh: &mut u8,
        pick: Pick,
        mut each: impl FnMut(&Decoded),
    ) -> (Vec<Decoded>, u64, usize) {
        let (mut newest, mut superseded) = (Vec::<Decoded>::new(), 0);
        let mut queue = self.backlog.lock().unwrap();
        queue.extend(self.access_units.try_iter());
        self.drop_stale(&mut queue, fresh);
        let mut decoded = 0;
        let streams = decoders.iter().filter(|d| d.is_some()).count().max(1);
        // Access units waiting per stream: three or more, it is behind and
        // decodes on (a held picture would only make it later).
        let mut waiting = [0usize; protocol::MAX_STREAMS as usize];
        for job in queue.iter() {
            waiting[usize::from(job.payload.stream) % waiting.len()] += 1;
        }
        let behind = |w: usize| w > 2;
        let mut stopped: u8 =
            match pick {
                Pick::PerStream { held } => (0..waiting.len()).fold(held, |m, s| {
                    if behind(waiting[s]) { m & !(1 << s) } else { m }
                }),
                _ => 0,
            };
        let mut left = VecDeque::new();
        while let Some(job) = queue.pop_front() {
            let stream = job.payload.stream;
            let s = usize::from(stream) % waiting.len();
            if stopped & (1 << s) != 0 || decoded == MAX_DECODES_PER_CALL * streams {
                left.push_back(job);
                continue;
            }
            decoded += 1;
            waiting[s] -= 1;
            let pictured = self.decode_one(&job, decoders, fresh, |d| {
                each(&d);
                match newest.iter_mut().find(|n| n.stream == d.stream) {
                    Some(n) => {
                        *n = d;
                        superseded += 1;
                    }
                    None => newest.push(d),
                }
            });
            let Some(pictured) = pictured else {
                continue;
            };
            match pick {
                Pick::UntilPicture if !newest.is_empty() => {
                    left.extend(queue.drain(..));
                    break;
                }
                // The new picture waits to be shown before the next of its stream.
                Pick::PerStream { .. } if pictured && !behind(waiting[s] + 1) => stopped |= 1 << s,
                _ => {}
            }
        }
        *queue = left;
        let decodable = queue
            .iter()
            .any(|j| stopped & (1 << (usize::from(j.payload.stream) % waiting.len())) == 0);
        if decodable {
            // Make sure the render loop comes back for the rest.
            self.wake.set();
        }
        (newest, superseded, queue.len())
    }

    /// Drops the queue of every stream that waits too long to be decoded
    /// (`QUEUE_LIMIT`): the decoder takes a keyframe next, and the network
    /// thread asks for one (as after a decode error).
    fn drop_stale(&self, queue: &mut VecDeque<ToDecode>, fresh: &mut u8) {
        let stale = stale_streams(
            queue.iter().map(|j| (j.payload.stream, j.queued.elapsed())),
            QUEUE_LIMIT,
        );
        if stale == 0 {
            return;
        }
        let bit =
            |j: &ToDecode| 1u8 << (usize::from(j.payload.stream) % protocol::MAX_STREAMS as usize);
        let oldest = queue
            .iter()
            .filter(|j| stale & bit(j) != 0)
            .map(|j| j.queued.elapsed())
            .max()
            .unwrap_or_default();
        let before = queue.len();
        queue.retain(|j| stale & bit(j) == 0);
        let dropped = before - queue.len();
        *fresh |= stale;
        self.decode_failed.fetch_or(stale, Ordering::Relaxed);
        let mut c = self.counters.lock().unwrap();
        c.queue_drops += 1;
        c.queue_dropped += dropped as u64;
        if c.queue_drops <= 20 {
            println!(
                "decode queue: {dropped} access units dropped (the oldest waited {:.0} ms; streams {stale:#b}); a keyframe asked for",
                oldest.as_secs_f64() * 1e3
            );
        }
    }

    /// Decodes one access unit with its stream's decoder, `picture` sees what
    /// it gives; whether it gave any, or none at all and an IDR asked for. No decoder (the host's new `Monitors` not
    /// here yet), or a new one whose first frame is not an IDR (the IDR went to
    /// the decoder it replaced): before, the stream stayed black: NVDEC gives
    /// nothing for P-frames without their reference, and says nothing (monitors
    /// added mid-session, 2026-10-30).
    fn decode_one(
        &self,
        job: &ToDecode,
        decoders: &mut [Option<Decoder>],
        fresh: &mut u8,
        mut picture: impl FnMut(Decoded),
    ) -> Option<bool> {
        let p = &job.payload;
        let s = usize::from(p.stream) % protocol::MAX_STREAMS as usize;
        let decoder = decoders
            .get_mut(usize::from(p.stream))
            .and_then(Option::as_mut);
        let starts = *fresh & (1 << s) != 0;
        let Some(decoder) = decoder.filter(|_| !starts || p.keyframe) else {
            self.decode_failed.fetch_or(1 << s, Ordering::Relaxed);
            return None;
        };
        *fresh &= !(1 << s);
        let decode_started_us = wall_clock_us();
        if let Err(e) = decoder.decode(&p.data, i64::from(p.frame)) {
            eprintln!("decode frame {} of stream {}: {e:#}", p.frame, p.stream);
            self.counters.lock().unwrap().decode_errors += 1;
            self.decode_failed.fetch_or(1 << s, Ordering::Relaxed);
            return None;
        }
        let mut pictured = false;
        while let Some(frame) = decoder.next_frame() {
            pictured = true;
            self.counters.lock().unwrap().stats.frames_decoded += 1;
            picture(Decoded {
                number: frame.timestamp as u32,
                stream: p.stream,
                frame,
                capture_us: p.capture_time_us,
                reassembled_us: job.reassembled_us,
                decode_started_us,
                decoded_us: wall_clock_us(),
            });
        }
        Some(pictured)
    }

    /// Auto-reset event set whenever a frame or a cursor update arrives.
    pub fn wake_event(&self) -> HANDLE {
        self.wake.handle()
    }

    pub fn request(&self, msg: ControlMessage) {
        let _ = self.requests.send(msg);
    }

    /// Queues an input event for reliable, ordered delivery to the host.
    pub fn send_input(&self, event: InputEvent) {
        let _ = self.input.send(event);
    }

    pub fn is_closed(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// How far [`Connection::decode_pending`] decodes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Up to the first picture (one monitor: it is shown before the next).
    UntilPicture,
    /// Several monitors: up to a picture per stream; streams in `held` (a bit
    /// each: their last picture is not shown yet) not at all. A stream behind
    /// by three access units or more decodes on.
    PerStream { held: u8 },
}

/// Win32 auto-reset event: lets the render thread sleep until data arrives
/// (together with window messages and the swapchain, in one wait).
pub(super) struct Wake {
    handle: usize,
    /// The event itself, in the copy that made it: closed with it.
    _owner: Option<desktop::Handle>,
}

impl Wake {
    fn new() -> Result<Self> {
        let h = unsafe { CreateEventW(None, false, false, None)? };
        Ok(Wake {
            handle: h.0 as usize,
            // SAFETY: just made, ours alone.
            _owner: Some(unsafe { desktop::Handle::from_raw(h) }),
        })
    }

    /// A non-owning copy for the network thread (the `Connection` outlives it).
    fn clone_handle(&self) -> Wake {
        Wake {
            handle: self.handle,
            _owner: None,
        }
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.handle as *mut _)
    }

    pub(super) fn set(&self) {
        unsafe {
            let _ = SetEvent(self.handle());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_streams_with_an_old_access_unit_are_stale() {
        let ms = Duration::from_millis;
        assert_eq!(stale_streams([].into_iter(), QUEUE_LIMIT), 0);
        let queue = [(0, ms(100)), (1, ms(600)), (1, ms(10)), (2, ms(0))];
        assert_eq!(stale_streams(queue.into_iter(), QUEUE_LIMIT), 0b010);
    }
}
