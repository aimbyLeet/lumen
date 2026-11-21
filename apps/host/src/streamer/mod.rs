//! The host's monitors in a session: one stream each ). A [`Screen`] is a monitor's capture (source, frame texture, P010
//! converter); it outlives sessions. In a session each shown screen gets a
//! thread of its own: capture → P010 → NVENC → FEC → the session's paced
//! sender, where the streams' datagrams take turns.

mod cursor;
mod layout;
mod rate;
mod screen;
mod worker;

use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Result, bail};
use capture::qpmap::QpMapParams;
use nvenc::RateControl;
use transport::{PacedHandle, SeqCounter};

pub(crate) use self::cursor::{renumber, send_cursor_shape};
pub(crate) use self::layout::{Spot, desktop_layout};
pub(crate) use self::rate::SessionRate;
pub(crate) use self::screen::{Screen, open_screens};
use self::worker::Worker;
use crate::control::SharedSealer;
use crate::cursor::CursorFeed;

/// What one second of a stream did, for the log and the overlay.
#[derive(Default, Clone)]
pub(crate) struct Window {
    pub(crate) frames: u32,
    /// Frames with a new image from the capture (the rest repeat the last one).
    pub(crate) captured: u32,
    pub(crate) bytes: u64,
    pub(crate) encode_ms: Vec<f64>,
    pub(crate) keyframes: u32,
    pub(crate) recoveries: u32,
    pub(crate) max_frame: usize,
    pub(crate) qp_sum: u64,
    pub(crate) ignored_requests: u32,
    /// The QP map's delta of moving blocks at the last frame (no map: none),
    /// and the share of moving blocks summed over the frames.
    pub(crate) map_delta: Option<i8>,
    pub(crate) moving_share_sum: f64,
}

impl Window {
    /// A frame of `bytes` went out.
    fn count(&mut self, bytes: usize, keyframe: bool, recovery: bool, qp: u32) {
        self.frames += 1;
        self.bytes += bytes as u64;
        self.keyframes += u32::from(keyframe);
        self.recoveries += u32::from(recovery);
        self.max_frame = self.max_frame.max(bytes);
        self.qp_sum += u64::from(qp);
    }
}

/// What the session tells a stream.
pub(crate) enum Cmd {
    /// An IDR next (the client restarted).
    Idr,
    /// The client lost `frame` of this stream and asks for an IDR.
    Keyframe { frame: u32 },
    /// The client lost `lost`; `last_good` is its newest whole frame.
    Recovery { last_good: u32, lost: u32 },
    /// FEC parity from now on.
    Fec(f64),
    /// Captured and encoded or not (the client does not show this monitor);
    /// shown again: from an IDR.
    Show(bool),
}

/// What every stream of a session shares.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) session: u32,
    pub(crate) seq: SeqCounter,
    pub(crate) sealer: SharedSealer,
    pub(crate) out: PacedHandle,
    pub(crate) cursor: Arc<Mutex<CursorFeed>>,
    pub(crate) rate: Arc<SessionRate>,
    pub(crate) stop: Arc<AtomicBool>,
    /// A stream's monitor changed its mode under it (its capture says so):
    /// the session opens the monitors anew.
    pub(crate) stale: Arc<AtomicBool>,
}

/// How a stream encodes.
pub(crate) struct Setup {
    pub(crate) fps: u32,
    pub(crate) preset: u32,
    pub(crate) rate_control: RateControl,
    /// The QP map for still regions (`--static-qp`).
    pub(crate) static_map: Option<QpMapParams>,
    pub(crate) parity: f64,
    /// The stream's AUs, as encoded (`--dump-stream`).
    pub(crate) dump: Option<BufWriter<File>>,
    /// The first frame's number: a stream started over goes on from its
    /// last one (its old frames may still be on the way: a lower number would
    /// look stale to the client).
    pub(crate) first_frame: u32,
    /// The pacer lane of its datagrams: its stream number, or 0 for all
    /// (`--no-interleave`).
    pub(crate) lane: u8,
}

/// What a stream's thread gives back: its screen, how it ended, the number
/// its next frame would have had.
pub(crate) type Ended = (Screen, Result<()>, u32);

/// A stream running on its thread.
pub(crate) struct Stream {
    pub(crate) cmds: Sender<Cmd>,
    pub(crate) win: Arc<Mutex<Window>>,
    thread: Option<JoinHandle<Ended>>,
}

impl Stream {
    /// Starts stream `stream` of `screen`; fails if its encoder does not open.
    pub(crate) fn start(stream: u8, screen: Screen, setup: Setup, shared: Shared) -> Result<Self> {
        let (cmds, rx) = mpsc::channel();
        let win = Arc::new(Mutex::new(Window::default()));
        let (ready_tx, ready) = mpsc::channel();
        let w = win.clone();
        let thread = thread::Builder::new()
            .name(format!("stream-{stream}"))
            .spawn(move || {
                let mut screen = screen;
                let worker = match Worker::new(stream, &mut screen, setup, shared, rx, w) {
                    Ok(worker) => {
                        let _ = ready_tx.send(Ok(()));
                        worker
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return (screen, Ok(()), 0);
                    }
                };
                let (r, next) = worker.run(&mut screen);
                (screen, r, next)
            })?;
        let mut s = Stream {
            cmds,
            win,
            thread: Some(thread),
        };
        match ready.recv() {
            Ok(Ok(())) => Ok(s),
            Ok(Err(e)) => {
                let _ = s.join();
                Err(e)
            }
            Err(_) => {
                let _ = s.join();
                bail!("stream {stream} thread died")
            }
        }
    }

    pub(crate) fn send(&self, cmd: Cmd) {
        let _ = self.cmds.send(cmd);
    }

    /// The thread ended by itself (a failure).
    pub(crate) fn ended(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the thread (the session's `stop` is set); gives the screen back.
    pub(crate) fn join(&mut self) -> Option<Ended> {
        let t = self.thread.take()?;
        Some(
            t.join()
                .unwrap_or_else(|_| panic!("stream thread panicked")),
        )
    }

    /// The last second's numbers, the window started anew.
    pub(crate) fn take_window(&self) -> Window {
        std::mem::take(&mut *self.win.lock().unwrap())
    }
}
