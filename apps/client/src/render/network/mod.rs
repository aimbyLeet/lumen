//! The live video from a host: connection, reconnection, decoding, presentation.
//! Its views are in `views`, its pointer in `pointer`, frame hashes in `hashes`.

mod hashes;
mod pointer;
mod views;

use anyhow::{Context, Result};
use decoder::Decoder;
use transport::{percentile as pct, wall_clock_us};

use protocol::{ControlMessage, InputEvent};

use super::{InputScript, ViewCache};
use crate::composite::Composite;
use crate::cursor::CursorOverlay;
use crate::scene::{self, Scene};
use hashes::HashSink;
use pointer::Pointer;
use views::MonitorView;
use windows::Win32::Foundation::HANDLE;

use crate::net::{Connection, CursorEvent, Decoded, Pick};
use gpu_convert::Yuv420ToRgb10;
use signal::ClientSession;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::*;

/// Several monitors: a stream's picture waits at most this long to be shown
/// before the next one is decoded over it (as `live::HOLD_MAX` for one).
const STREAM_HOLD: Duration = Duration::from_millis(25);
const MAX_STREAMS: usize = protocol::MAX_STREAMS as usize;

/// No new picture for this long while connected: the session is dead.
const STALL: Duration = Duration::from_secs(3);
/// Wait this long between failed reconnection attempts.
const RETRY: Duration = Duration::from_secs(1);
/// How often the overlay gets the counters (its rate graph's step).
const OVERLAY_SNAPSHOT: Duration = Duration::from_millis(200);

/// Opens a new session through the server (signaling, handshake).
pub type OpenSession = Box<dyn FnMut() -> Result<(Connection, ClientSession)> + Send>;
/// Errors a retry cannot fix (no approval any more, unknown token).
pub type IsPermanent = fn(&anyhow::Error) -> bool;

/// Automatic reconnection (server mode): when the session dies, a new one is
/// opened on a background thread while the last picture stays on screen.
pub struct Reconnect {
    open: Arc<Mutex<OpenSession>>,
    permanent: IsPermanent,
    session: Option<ClientSession>,
    pending: Option<Receiver<Result<(Connection, ClientSession)>>>,
    lost_at: Option<Instant>,
    next_try: Instant,
}

impl Reconnect {
    pub fn new(open: OpenSession, session: ClientSession, permanent: IsPermanent) -> Self {
        Reconnect {
            open: Arc::new(Mutex::new(open)),
            permanent,
            session: Some(session),
            pending: None,
            lost_at: None,
            next_try: Instant::now(),
        }
    }
}

/// How the window shows several monitors of the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Mode 1: every monitor, laid out as at the host.
    All,
    /// Mode 2: one monitor (its index in `Monitors`) over the whole window,
    /// the circles above it switch.
    One(usize),
    /// Mode 3: a window per monitor (the main one shows monitor 1).
    Windows,
}

/// How [`NetworkVideo::update`] decodes what arrived.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    /// Up to the first new picture: the live loop presents it before decoding
    /// the next one over it (a 4K decode takes ~13 ms on Pascal, and frames
    /// arriving in a burst were decoded in one go, the first never shown).
    /// Behind by more than a frame (start, after a stall) it does not wait for
    /// the swapchain, but shows each one the swapchain takes on the way.
    UntilPicture,
    /// Nothing: a decoded picture waits for the swapchain.
    Hold,
}

/// The device, and the decoders made on it.
struct Decoding {
    device: ID3D11Device,
    /// One per stream (monitor of the host); made for the monitors the host has.
    decoders: Vec<Option<Decoder>>,
    /// The CUDA context of all the decoders: making one takes ~60 ms, a
    /// decoder in it a few (monitors told, a monitor more, a reconnection).
    gpu: std::rc::Rc<decoder::Gpu>,
    /// Decoders made anew that have not had their IDR yet (a bit per stream).
    fresh_decoders: u8,
}

/// The picture of the main window: decoded, waiting for its first presentation.
struct Presenting {
    current: Option<Decoded>,
    /// The current frame was drawn for the first time and not yet presented.
    fresh: bool,
    /// When the current frame was decoded.
    fresh_at: Instant,
    /// More than one access unit waited after the current frame was decoded:
    /// catching up, it must not wait for the swapchain.
    behind: bool,
}

/// The session's statistics, printed by [`NetworkVideo::report`].
struct Stats {
    shown: u64,
    skipped: u64,
    /// Other monitors' pictures replaced before they were shown.
    others_skipped: u64,
    /// Capture → reassembled / decoded / presented, ms, per first presentation.
    lat: [Vec<f64>; 3],
    started: std::time::Instant,
}

/// Checks only: barcodes, scripted input, frame hashes, the frame log.
struct Checks {
    barcode_every: u64,
    /// Scripted input instead of the user's (end-to-end input check).
    script: Option<InputScript>,
    /// The run is scripted: the user's input stays out of it to the end, also
    /// after the script (it would spoil the comparison of the logs).
    scripted: bool,
    barcode_ok: u64,
    barcode_bad: u64,
    /// "<frame> <hash>" of every decoded picture (end-to-end bit-exactness check).
    hashes: Option<HashSink>,
    /// Every decoded frame's times and every first presentation (--frame-log).
    frame_log: Option<std::io::BufWriter<std::fs::File>>,
}

/// Live stream from a host, driven by [`crate::live`].
pub struct NetworkVideo {
    conn: Connection,
    reconnect: Option<Reconnect>,
    /// When the last picture was decoded (stall detection).
    last_frame: Instant,
    dec: Decoding,
    mon: MonitorView,
    pointer: Pointer,
    /// Keys and buttons held down on the host, released when the window loses the focus.
    held: input::Held,
    shader: Yuv420ToRgb10,
    views: ViewCache,
    present: Presenting,
    stats: Stats,
    checks: Checks,
    /// What the overlay (Ctrl+Alt+Shift+O) shows, and when it last got the counters.
    overlay: Option<(crate::overlay::Feed, Instant)>,
    /// Host clock minus ours, ms, as the counters said last (frame times are the host's).
    offset_ms: f64,
}

impl NetworkVideo {
    pub fn new(
        device: &ID3D11Device,
        conn: Connection,
        barcode_every: u64,
        script: Option<InputScript>,
        hashes: Option<&std::path::Path>,
        frame_log: Option<&std::path::Path>,
        reconnect: Option<Reconnect>,
    ) -> Result<Self> {
        let hashes = hashes.map(|p| HashSink::new(device, p)).transpose()?;
        let gpu = decoder::Gpu::new(device)?;
        // Monitor 1 alone, until the host tells its monitors.
        let first = Scene::single(u32::from(conn.format.width), u32::from(conn.format.height));
        scene::set(first.clone());
        let frame_log = frame_log
            .map(|p| {
                std::fs::File::create(p)
                    .map(std::io::BufWriter::new)
                    .with_context(|| format!("create {p:?}"))
            })
            .transpose()?;
        Ok(NetworkVideo {
            conn,
            reconnect,
            last_frame: Instant::now(),
            dec: Decoding {
                device: device.clone(),
                decoders: vec![Some(Decoder::on(&gpu, device)?)],
                gpu,
                fresh_decoders: 0,
            },
            mon: MonitorView {
                scene: first,
                scene_of: None,
                composite: None,
                monitors: Vec::new(),
                mode: Mode::All,
                start_mode: None,
                pictures: Vec::new(),
                switched: None,
                unshown: [None; MAX_STREAMS],
                dirty_streams: 0,
                group_borderless: false,
            },
            pointer: Pointer {
                cursor: CursorOverlay::new(device)?,
                host_pointer: None,
                local_pointer: None,
                shape_requested: None,
                drawn_at: None,
                local_moved: false,
            },
            held: input::Held::default(),
            shader: Yuv420ToRgb10::new(device)?,
            views: ViewCache::new(device),
            present: Presenting {
                current: None,
                fresh: false,
                fresh_at: Instant::now(),
                behind: false,
            },
            stats: Stats {
                shown: 0,
                skipped: 0,
                others_skipped: 0,
                lat: Default::default(),
                started: std::time::Instant::now(),
            },
            checks: Checks {
                barcode_every,
                scripted: script.is_some(),
                script,
                barcode_ok: 0,
                barcode_bad: 0,
                hashes,
                frame_log,
            },
            overlay: None,
            offset_ms: 0.0,
        })
    }

    /// The mode to start in once the host tells its monitors: if they are
    /// `monitors` (none: whatever they are).
    pub fn set_start_mode(&mut self, monitors: Option<Vec<protocol::Monitor>>, mode: Mode) {
        self.mon.start_mode = Some((monitors, mode));
    }

    /// The host's monitors as it said last (none yet: empty).
    pub fn monitors(&self) -> &[protocol::Monitor] {
        &self.mon.monitors
    }

    pub fn mode(&self) -> Mode {
        self.mon.mode
    }

    /// Feeds the overlay from now on.
    pub fn set_overlay(&mut self, feed: crate::overlay::Feed) {
        self.overlay = Some((feed, Instant::now() - OVERLAY_SNAPSHOT));
    }

    /// The counters to the overlay, a few times a second.
    fn feed_overlay(&mut self) {
        let Some((feed, last)) = self.overlay.as_mut() else {
            return;
        };
        let now = Instant::now();
        if now - *last < OVERLAY_SNAPSHOT {
            return;
        }
        *last = now;
        let counters = *self.conn.counters.lock().unwrap();
        self.offset_ms = counters.clock.offset_us().unwrap_or(0) as f64 / 1e3;
        let f = self.conn.format;
        let (w, h) = self.mon.scene.canvas;
        let share = |m: &crate::scene::Placed| {
            [
                m.canvas.x as f32 / w.max(1) as f32,
                m.canvas.y as f32 / h.max(1) as f32,
                m.canvas.w as f32 / w.max(1) as f32,
                m.canvas.h as f32 / h.max(1) as f32,
            ]
        };
        feed.set_view(crate::overlay::ViewInfo {
            mode: match self.mon.mode {
                Mode::All => "all",
                Mode::One(_) => "one",
                Mode::Windows => "windows",
            },
            active: match self.mon.mode {
                Mode::One(i) => i,
                _ => 0,
            },
            count: self.mon.monitors.len(),
            first: self
                .mon
                .scene
                .monitors
                .first()
                .map_or([0.0, 0.0, 1.0, 1.0], share),
        });
        feed.snapshot(
            now,
            crate::overlay::Snapshot {
                format: Some(f),
                fps: f64::from(f.fps_num) / f64::from(f.fps_den.max(1)),
                counters,
                shown: self.stats.shown,
                audio: crate::audio::status(),
            },
        );
    }

    /// The stream's picture size.
    pub fn size(&self) -> (u32, u32) {
        let f = self.conn.format;
        (u32::from(f.width), u32::from(f.height))
    }

    /// With automatic reconnection the stream never closes by itself.
    pub fn is_closed(&self) -> bool {
        self.reconnect.is_none() && self.conn.is_closed()
    }

    /// Notices a dead session and swaps in a new one; fails when a new one
    /// cannot be had (e.g. the admin took the access back).
    fn check_connection(&mut self) -> Result<()> {
        let Some(rc) = self.reconnect.as_mut() else {
            return Ok(());
        };
        if let Some(rx) = &rc.pending {
            match rx.try_recv() {
                Ok(Ok((conn, session))) => {
                    let lost = rc.lost_at.take().map_or(0.0, |t| t.elapsed().as_secs_f64());
                    if conn.format != self.conn.format {
                        println!(
                            "note: the host now streams {:?}; restart the client if the picture looks wrong",
                            conn.format
                        );
                    }
                    println!(
                        "reconnected: session {:016x}, {lost:.1} s after the loss",
                        session.id
                    );
                    crate::status::streaming(&conn);
                    self.conn = conn;
                    self.mon.scene_of = None;
                    rc.session = Some(session);
                    rc.pending = None;
                    self.last_frame = Instant::now();
                    self.pointer.host_pointer = None;
                    self.pointer.shape_requested = None;
                }
                Ok(Err(e)) if (rc.permanent)(&e) => return Err(e.context("cannot reconnect")),
                Ok(Err(e)) => {
                    crate::status::retrying(&e);
                    println!("reconnect failed: {e:#}; retrying");
                    rc.pending = None;
                    rc.next_try = Instant::now() + RETRY;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => rc.pending = None,
            }
            return Ok(());
        }
        if self.conn.is_closed() {
            // The host said bye (it was closed, or its --seconds ran out): no
            // reconnection, the window closes.
            println!("the host ended the stream");
            self.reconnect = None;
            return Ok(());
        }
        let why = if rc.session.as_ref().is_none_or(|s| s.ended()) {
            "the session ended on the server"
        } else if self.last_frame.elapsed() > STALL {
            "no picture"
        } else {
            return Ok(());
        };
        if Instant::now() < rc.next_try {
            return Ok(());
        }
        if rc.lost_at.is_none() {
            println!("connection lost ({why}); reconnecting");
            crate::status::reconnecting(why);
            rc.lost_at = Some(Instant::now());
        }
        rc.session = None; // ends the old session on the server
        let open = rc.open.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (open.lock().unwrap())();
            let _ = tx.send(result);
        });
        rc.pending = Some(rx);
        Ok(())
    }

    /// Signalled by the network thread whenever something arrives.
    pub fn wake_event(&self) -> HANDLE {
        self.conn.wake_event()
    }

    /// Since when a decoded picture waits for its first presentation, and may
    /// keep waiting: not when frames queue up behind it (catching up).
    pub fn unshown_since(&self) -> Option<Instant> {
        (self.present.fresh && !self.present.behind).then_some(self.present.fresh_at)
    }

    /// Pictures of the host shown so far (each counted at its first present).
    pub fn shown(&self) -> u64 {
        self.stats.shown
    }

    /// A decoded picture has not been presented yet.
    pub fn is_fresh(&self) -> bool {
        self.present.fresh
    }

    /// Decodes whatever arrived and takes cursor updates. Returns true if the
    /// picture should be redrawn (new frame, pointer moved, shape changed).
    pub fn update(&mut self, ctx: &ID3D11DeviceContext, decode: Decode) -> Result<bool> {
        self.check_connection()?;
        self.feed_overlay();
        let mut dirty = std::mem::take(&mut self.pointer.local_moved);
        dirty |= self.follow_monitors()?;
        if let Some(s) = self.checks.script.as_mut()
            && !s.step(&self.conn)?
        {
            self.checks.script = None;
        }
        let (newest, superseded, waiting) = self.decode_arrived(ctx, decode)?;
        self.stats.skipped += superseded;
        dirty |= self.take_pictures(ctx, newest, waiting)?;
        dirty |= self.take_pointer_news()?;
        Ok(dirty)
    }

    /// Decodes what arrived as `decode` says: the newest picture of each
    /// stream, how many were superseded, how many access units still wait.
    fn decode_arrived(
        &mut self,
        ctx: &ID3D11DeviceContext,
        decode: Decode,
    ) -> Result<(Vec<Decoded>, u64, usize)> {
        // Several monitors shown: each stream's picture waits to be shown
        // before the next of that stream is decoded, the others go on (frames
        // come in bursts through WG: decoding a burst at once dropped a fifth
        // of them, seen 2026-10-30).
        let pick = match decode {
            Decode::Hold => return Ok((Vec::new(), 0, 0)),
            _ if !self.holds_pictures() => Pick::PerStream {
                held: self.held_streams(),
            },
            Decode::UntilPicture => Pick::UntilPicture,
        };
        let mut hash_error = None;
        let hashes = &self.checks.hashes;
        let frame_log = &mut self.checks.frame_log;
        let decoded = self.conn.decode_pending(
            &mut self.dec.decoders,
            &mut self.dec.fresh_decoders,
            pick,
            |d| {
                if d.stream != 0 {
                    return;
                }
                if let Some(sink) = hashes
                    && let Err(e) = sink.push(ctx, &d.frame)
                {
                    hash_error.get_or_insert(e);
                }
                if let Some(log) = frame_log {
                    use std::io::Write;
                    let _ = writeln!(
                        log,
                        "D {} {} {} {} {}",
                        d.number, d.capture_us, d.reassembled_us, d.decoded_us, d.decode_started_us
                    );
                }
            },
        );
        match hash_error {
            Some(e) => Err(e.context("--frame-hashes")),
            None => Ok(decoded),
        }
    }

    /// The new pictures, each to where its monitor is shown. True if the main
    /// window has something new to show.
    fn take_pictures(
        &mut self,
        ctx: &ID3D11DeviceContext,
        newest: Vec<Decoded>,
        waiting: usize,
    ) -> Result<bool> {
        let multi = self.mon.scene.is_multi();
        let holds = self.holds_pictures();
        let shown_stream = self.shown_stream();
        let (mut dirty, mut shown) = (false, None);
        for d in newest {
            if self.mon.switched.is_some_and(|(_, s)| s == d.stream) {
                let (t, s) = self.mon.switched.take().unwrap();
                println!(
                    "monitor {} shown: its first frame {:.0} ms after the switch",
                    s + 1,
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
            if multi {
                self.compose(ctx, &d)?;
                self.last_frame = Instant::now();
                dirty = true;
            }
            self.mon.dirty_streams |= 1 << d.stream;
            if !holds {
                let slot = &mut self.mon.unshown[usize::from(d.stream) % MAX_STREAMS];
                if slot.is_some() && d.stream != shown_stream {
                    self.stats.others_skipped += 1;
                }
                *slot = Some(Instant::now());
            }
            if d.stream == shown_stream {
                shown = Some(d);
            } else {
                let i = usize::from(d.stream);
                if self.mon.pictures.len() <= i {
                    self.mon.pictures.resize_with(i + 1, || None);
                }
                self.mon.pictures[i] = Some(d);
            }
        }
        if let Some(d) = shown {
            if self.checks.barcode_every > 0
                && self.stats.shown.is_multiple_of(self.checks.barcode_every)
            {
                self.check_barcode(ctx, &d)?;
            }
            if self.present.fresh {
                self.stats.skipped += 1; // decoded but replaced before it could be shown
            }
            self.present.current = Some(d);
            self.last_frame = Instant::now();
            self.present.fresh = true;
            self.present.fresh_at = self.last_frame;
            self.present.behind = waiting > 1;
            dirty = true;
        }
        Ok(dirty)
    }

    /// --frame-log: one render + present took `begin..end` (µs, our clock).
    pub fn log_present(&mut self, begin: u64, end: u64) {
        if let Some(log) = self.checks.frame_log.as_mut() {
            use std::io::Write;
            let _ = writeln!(log, "R {begin} {end}");
        }
    }

    /// Draws the newest picture and the pointer into `rtv`.
    pub fn render(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
    ) -> Result<()> {
        self.pointer.drawn_at = None;
        match (&self.mon.composite, &self.present.current) {
            (Some(c), _) if self.mon.scene.is_multi() => {
                c.draw(ctx, rtv, &self.mon.scene, &self.fit())?;
            }
            (_, Some(d)) => self.views.draw(&self.shader, ctx, rtv, &d.frame)?,
            _ => unsafe { ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]) },
        }
        self.draw_pointer(ctx, rtv, None);
        Ok(())
    }

    /// Something for the overlay page to say for a moment.
    pub fn say(&self, code: &'static str) {
        if let Some((feed, _)) = &self.overlay {
            feed.say(code);
        }
    }

    /// Mode 3: monitor `stream`'s picture 1:1 into `rtv` (its window's
    /// render target), the pointer if it is there.
    pub fn render_stream(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        stream: u8,
    ) -> Result<()> {
        let shown = self.shown_stream();
        let picture = if stream == shown {
            self.present.current.as_ref()
        } else {
            self.mon
                .pictures
                .get(usize::from(stream))
                .and_then(Option::as_ref)
        };
        match picture {
            Some(d) => self.views.draw(&self.shader, ctx, rtv, &d.frame)?,
            None => unsafe { ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]) },
        }
        let Some(m) = self
            .mon
            .monitors
            .iter()
            .find(|m| m.stream == stream)
            .copied()
        else {
            return Ok(());
        };
        self.draw_pointer(ctx, rtv, Some(&m));
        Ok(())
    }

    /// When monitor `stream`'s picture shown now was captured, µs on our clock.
    pub fn stream_capture_us(&self, stream: u8) -> Option<u64> {
        let d = if stream == self.shown_stream() {
            self.present.current.as_ref()
        } else {
            self.mon.pictures.get(usize::from(stream))?.as_ref()
        }?;
        let offset = self
            .conn
            .counters
            .lock()
            .unwrap()
            .clock
            .offset_us()
            .unwrap_or(0);
        Some((d.capture_us as i64 - offset).max(1) as u64)
    }

    /// Several monitors: the canvas in the window as it is now.
    fn fit(&self) -> crate::layout::Layout {
        let client = crate::window::current_layout().client;
        crate::layout::layout(self.mon.scene.canvas, client)
    }

    /// A new picture of a monitor into the composite view.
    fn compose(&mut self, ctx: &ID3D11DeviceContext, d: &Decoded) -> Result<()> {
        let Some(c) = self.mon.composite.as_mut() else {
            return Ok(());
        };
        let (luma, chroma) = (
            self.views.get(&d.frame.luma)?,
            self.views.get(&d.frame.chroma)?,
        );
        c.update(
            ctx,
            &self.shader,
            &luma,
            &chroma,
            d.stream,
            (d.frame.width, d.frame.height),
        )
    }

    /// Reads the frame number drawn into the picture by the synthetic host source.
    fn check_barcode(&mut self, ctx: &ID3D11DeviceContext, d: &Decoded) -> Result<()> {
        let yuv = decoder::read_frame(&self.views.device, ctx, &d.frame)?;
        match capture::barcode::read_from_luma(&yuv.y, yuv.width) {
            Some(n) if n == d.number => self.checks.barcode_ok += 1,
            other => {
                self.checks.barcode_bad += 1;
                eprintln!("frame {}: barcode reads {other:?}", d.number);
            }
        }
        Ok(())
    }
}

impl NetworkVideo {
    /// A mouse or keyboard message of the window (`msg`, `wparam`, `lparam`).
    pub fn on_input(&mut self, msg: u32, wparam: usize, lparam: isize) {
        if msg == crate::window::WM_GROUP_BORDERLESS {
            self.mon.group_borderless = true;
            return;
        }
        if msg == crate::window::WM_NEXT_LAYOUT {
            if let Err(e) = self.next_mode() {
                eprintln!("view: {e:#}");
            }
            return;
        }
        if msg == crate::window::WM_SHOW_MONITOR {
            if matches!(self.mon.mode, Mode::One(_))
                && wparam < self.mon.monitors.len()
                && let Err(e) = self.show_one(wparam)
            {
                eprintln!("view: {e:#}");
            }
            return;
        }
        if self.checks.scripted {
            return;
        }
        if msg == windows::Win32::UI::WindowsAndMessaging::WM_KILLFOCUS {
            for ev in self.held.release_all() {
                self.conn.send_input(ev);
            }
            return;
        }
        for ev in input::from_window_message(msg, wparam, lparam) {
            if let InputEvent::MouseMove { x, y } = ev {
                let mut touched = self.streams_at(x, y);
                if let Some((lx, ly, _)) = self.pointer.local_pointer {
                    touched |= self.streams_at(lx, ly);
                }
                self.pointer.local_moved |= self.main_window_shows(touched);
                self.mon.dirty_streams |= touched;
                self.pointer.local_pointer = Some((x, y, std::time::Instant::now()));
            }
            self.held.track(&ev);
            self.conn.send_input(ev);
        }
    }

    /// Right after the picture rendered last was presented.
    pub fn presented(&mut self) {
        match self.mon.mode {
            // The composite shows every monitor's newest picture.
            Mode::All => self.mon.unshown = [None; MAX_STREAMS],
            Mode::Windows => self.taken(self.shown_stream()),
            Mode::One(_) => {}
        }
        if !std::mem::take(&mut self.present.fresh) {
            return;
        }
        let Some(d) = &self.present.current else {
            return;
        };
        let now = wall_clock_us();
        // Signed: `capture_us` is the host's clock, which may run ahead of ours
        // (the report adds the measured offset).
        let ms = |t: u64| (t as i64 - d.capture_us as i64) as f64 / 1e3;
        // The recent ones: a session of days would keep them all.
        for (v, x) in self
            .stats
            .lat
            .iter_mut()
            .zip([d.reassembled_us, d.decoded_us, now])
        {
            transport::push_recent(v, ms(x), transport::RECENT_SAMPLES);
        }
        self.stats.shown += 1;
        if let Some((feed, _)) = &self.overlay {
            // Capture times are the host's clock: ours, with the offset.
            let at = |t: u64| t as f64 / 1e3;
            let capture = at(d.capture_us) - self.offset_ms;
            feed.frame(crate::overlay::FrameSample {
                at: Instant::now(),
                // Not below zero: the offset is an estimate (within half the
                // fastest round trip of the last 30 s).
                net_ms: (at(d.reassembled_us) - capture).max(0.0) as f32,
                // The decoder's own time; the queue before it (a decoded
                // picture is shown before the next is decoded) is waiting.
                decode_ms: (at(d.decoded_us) - at(d.decode_started_us)) as f32,
                show_ms: (at(d.decode_started_us) - at(d.reassembled_us) + at(now)
                    - at(d.decoded_us)) as f32,
            });
        }
        if let Some(log) = self.checks.frame_log.as_mut() {
            use std::io::Write;
            let _ = writeln!(log, "P {} {now}", d.number);
        }
    }

    /// Prints the session's statistics at exit.
    pub fn report(&mut self) {
        let secs = self.stats.started.elapsed().as_secs_f64();
        let c = *self.conn.counters.lock().unwrap();
        let s = c.stats;
        let process = desktop::cpu::cpu_time(false).as_secs_f64();
        println!(
            "cpu: process {process:.2} s ({:.1} % of a core), network thread {:.2} s ({:.1} %)",
            100.0 * process / secs,
            c.net_cpu.as_secs_f64(),
            100.0 * c.net_cpu.as_secs_f64() / secs
        );
        println!(
            "network: {} pkts ({:.1} Mbit/s), {} pkts lost, {} frames lost, {} recovered by FEC, {} failed decryption",
            s.packets_received,
            c.bytes as f64 * 8.0 / secs / 1e6,
            s.packets_lost,
            c.frames_lost,
            s.fec_recovered,
            c.rejected
        );
        println!(
            "frames: {} decoded, {} shown, {} skipped (newer frame ready), {} dropped waiting for IDR / recovery, {} keyframe / {} recovery requests, {} decode errors",
            s.frames_decoded,
            self.stats.shown,
            self.stats.skipped,
            s.frames_dropped,
            s.keyframes_requested,
            s.recoveries_requested,
            c.decode_errors
        );
        if c.queue_drops > 0 {
            println!(
                "decode queue: dropped {} times ({} access units waited over 500 ms)",
                c.queue_drops, c.queue_dropped
            );
        }
        if self.mon.monitors.len() > 1 {
            println!(
                "other monitors: {} pictures replaced before they were shown",
                self.stats.others_skipped
            );
        }
        println!(
            "input: {} events sent, {} resent, {} unacknowledged at exit; resend timeout {:.1} ms",
            c.inputs_sent,
            c.inputs_resent,
            c.inputs_in_flight,
            c.input_rto.as_secs_f64() * 1e3
        );
        if self.checks.barcode_every > 0 {
            println!(
                "barcode: {} frames verified, {} mismatched",
                self.checks.barcode_ok, self.checks.barcode_bad
            );
        }
        // Frame timestamps are the host's clock: shift by the measured offset
        // (about 0 with host and client on one machine).
        let offset_ms = match (c.clock.offset_us(), c.clock.rtt_ms()) {
            (Some(off), Some((min, mean, max, n))) => {
                println!(
                    "rtt to host ms: min {min:.2}  mean {mean:.2}  max {max:.2} ({n} pings); \
                     host clock {:+.2} ms from ours (latencies below corrected)",
                    off as f64 / 1e3
                );
                off as f64 / 1e3
            }
            _ => {
                println!("rtt to host: no answer to pings; latencies below assume one clock");
                0.0
            }
        };
        for (name, v) in ["reassembled", "decoded", "presented"]
            .iter()
            .zip(&mut self.stats.lat)
        {
            v.iter_mut().for_each(|x| *x += offset_ms);
            v.sort_by(f64::total_cmp);
            println!(
                "latency capture -> {name:<11} ms: p50 {:6.2}  p95 {:6.2}  p99 {:6.2}  max {:6.2}",
                pct(v, 0.5),
                pct(v, 0.95),
                pct(v, 0.99),
                v.last().copied().unwrap_or(f64::NAN)
            );
        }
    }
}
