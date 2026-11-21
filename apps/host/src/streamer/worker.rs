//! A stream's thread: capture → P010 → NVENC → FEC → the session's paced sender.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use capture::qpmap::StaticQpMap;
use gpu_convert::raw;
use nvenc::{Codec, Encoder, EncoderConfig, InputHandle};
use protocol::{Flags, PacketKind};
use transport::{FecConfig, Packetizer, wall_clock_us};

use super::cursor::send_cursor;
use super::{Cmd, Screen, Setup, Shared, Window};
use crate::source::Source;

/// Encoder reference buffer: how far back a recovery can reach (266 ms at 60 fps).
pub(crate) const REF_FRAMES: u32 = 16;
/// A hidden stream looks at its commands this often.
const HIDDEN_NAP: Duration = Duration::from_millis(20);

/// A stream's state on its thread.
pub(super) struct Worker {
    stream: u8,
    shared: Shared,
    cmds: Receiver<Cmd>,
    win: Arc<Mutex<Window>>,
    encode: Encode,
    wire: Wire,
    clock: Clock,
    frame: u32,
    refresh: Refresh,
    au: Vec<u8>,
    dump: Option<BufWriter<File>>,
}

/// The encoder, its registered input, the static QP map and the rate it is set to.
pub(super) struct Encode {
    pub(super) encoder: Encoder,
    pub(super) input: InputHandle,
    pub(super) static_map: Option<StaticQpMap>,
    /// What this encoder is set to now, bit/s (follows `SessionRate`).
    pub(super) encoder_bps: u32,
}

/// What goes out: the packetizer (FEC) and the pacer lane.
pub(super) struct Wire {
    pub(super) packetizer: Packetizer,
    pub(super) lane: u8,
}

/// When frames are made: shown or not, the frame interval and the next slot.
pub(super) struct Clock {
    pub(super) shown: bool,
    pub(super) interval: Duration,
    pub(super) next_tick: Instant,
}

/// Keyframes and recoveries: what the next frame must be, the last ones sent.
pub(super) struct Refresh {
    pub(super) force_idr: bool,
    pub(super) last_idr: Option<u32>,
    /// The next frame starts a recovery; the last frame that did.
    pub(super) recover_next: bool,
    pub(super) last_recovery: Option<u32>,
}

impl Worker {
    pub(super) fn new(
        stream: u8,
        screen: &mut Screen,
        setup: Setup,
        shared: Shared,
        cmds: Receiver<Cmd>,
        win: Arc<Mutex<Window>>,
    ) -> Result<Self> {
        let mut encoder = unsafe {
            Encoder::new(
                raw(&screen.gpu().device),
                EncoderConfig {
                    codec: Codec::Hevc,
                    width: screen.width,
                    height: screen.height,
                    fps: setup.fps,
                    preset: setup.preset,
                    rate_control: setup.rate_control,
                    ref_frames: REF_FRAMES,
                    qp_map: setup.static_map.is_some(),
                },
            )
        }
        .context("NVENC init")?;
        let static_map = setup
            .static_map
            .map(|p| StaticQpMap::new(screen.width, screen.height, encoder.qp_map_block(), p));
        let input = unsafe { encoder.register_d3d11_p010(raw(&screen.converter.output)) }?;
        let mut packetizer = Packetizer::with_seq(
            FecConfig {
                parity_ratio: setup.parity,
                ..FecConfig::default()
            },
            shared.session,
            shared.seq.clone(),
        )?;
        // Numbered and sealed as they leave the pacer (the session's sender).
        packetizer.defer_seq();
        let encoder_bps = shared.rate.nominal_bps();
        Ok(Worker {
            stream,
            shared,
            cmds,
            win,
            encode: Encode {
                encoder,
                input,
                static_map,
                encoder_bps,
            },
            wire: Wire {
                packetizer,
                lane: setup.lane,
            },
            clock: Clock {
                shown: true,
                interval: Duration::from_secs_f64(1.0 / f64::from(setup.fps)),
                next_tick: Instant::now(),
            },
            frame: setup.first_frame,
            refresh: Refresh {
                force_idr: true,
                last_idr: None,
                recover_next: false,
                last_recovery: None,
            },
            au: Vec::with_capacity(4 << 20),
            dump: setup.dump,
        })
    }

    /// Log lines of monitor 2 and on say which one (monitor 1's as before).
    fn say(&self, what: std::fmt::Arguments) {
        match self.stream {
            0 => println!("{what}"),
            s => println!("monitor {}: {what}", s + 1),
        }
    }

    /// Frames until the session stops the streams; how it ended, and the
    /// number of the next frame.
    pub(super) fn run(mut self, screen: &mut Screen) -> (Result<()>, u32) {
        let r = self.frames(screen);
        (r, self.frame)
    }

    fn frames(&mut self, screen: &mut Screen) -> Result<()> {
        while !self.shared.stop.load(Ordering::Relaxed) {
            if self.clock.shown {
                while let Ok(cmd) = self.cmds.try_recv() {
                    self.on_cmd(cmd);
                }
            } else {
                match self.cmds.recv_timeout(HIDDEN_NAP) {
                    Ok(cmd) => self.on_cmd(cmd),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            if self.clock.shown {
                self.frame(screen)?;
            }
        }
        if let Some(d) = self.dump.as_mut() {
            d.flush()?;
        }
        Ok(())
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        let frame = self.frame;
        match cmd {
            Cmd::Idr => self.refresh.force_idr = true,
            // An IDR sent after the lost frame is already on its way: the
            // client keeps repeating the request until it arrives. If that
            // IDR is lost too, the client reports a newer lost frame.
            Cmd::Keyframe { frame } if self.refresh.last_idr.is_some_and(|k| k > frame) => {
                self.win.lock().unwrap().ignored_requests += 1;
            }
            Cmd::Keyframe { frame } => {
                self.say(format_args!(
                    "keyframe requested (client lost frame {frame})"
                ));
                self.refresh.force_idr = true;
            }
            // Same idea: a recovery (or IDR) newer than the lost frame is in flight.
            Cmd::Recovery { lost, .. }
                if self.refresh.last_idr.is_some_and(|k| k > lost)
                    || self.refresh.last_recovery.is_some_and(|r| r > lost) =>
            {
                self.win.lock().unwrap().ignored_requests += 1;
            }
            Cmd::Recovery { last_good, lost } => self.recover(last_good, lost, frame),
            Cmd::Fec(ratio) => self.wire.packetizer.set_parity_ratio(ratio),
            Cmd::Show(on) if on == self.clock.shown => {}
            Cmd::Show(on) => {
                self.say(format_args!("{}", if on { "shown" } else { "hidden" }));
                self.clock.shown = on;
                // Frames resume from an IDR, on the clock from now.
                self.refresh.force_idr = true;
                self.clock.next_tick = Instant::now();
            }
        }
    }

    /// The client lost frame `lost`; `last_good` is its newest whole one.
    /// Frames `last_good`+1 ..= `frame`-1 are suspect there: invalidated, the
    /// next frame references an older one — or an IDR, if out of reach.
    fn recover(&mut self, last_good: u32, lost: u32, frame: u32) {
        if last_good >= frame || frame - last_good > REF_FRAMES {
            self.say(format_args!(
                "recovery: frame {last_good} is out of the reference buffer (now {frame}), sending an IDR"
            ));
            self.refresh.force_idr = true;
        } else if let Err(e) = (last_good + 1..frame)
            .try_for_each(|f| self.encode.encoder.invalidate_ref_frame(u64::from(f)))
        {
            self.say(format_args!("recovery: {e}, sending an IDR"));
            self.refresh.force_idr = true;
        } else {
            self.say(format_args!(
                "recovery: client lost frame {lost}, referencing frame {last_good} or older"
            ));
            self.refresh.recover_next = true;
        }
    }

    /// Waits for the next frame's slot, then captures, encodes and sends it.
    fn frame(&mut self, screen: &mut Screen) -> Result<()> {
        let captured = self.capture(screen)?;
        let capture_us = wall_clock_us();
        self.send_pointer(screen)?;

        // The map for this frame, from what changed since the last one.
        let mut map = None;
        if let (Some(m), Some(out)) = (
            self.encode.static_map.as_mut(),
            self.encode.encoder.qp_map_mut(),
        ) {
            map = Some(m.update(&screen.source.take_changes(), out));
        }

        let t_enc = Instant::now();
        let gpu = screen.source.gpu();
        screen.converter.convert(&gpu.context, &screen.src_srv);
        self.au.clear();
        let enc = self.encode.encoder.encode(
            self.encode.input,
            u64::from(self.frame),
            self.refresh.force_idr,
            &mut self.au,
        )?;
        let encode_ms = t_enc.elapsed().as_secs_f64() * 1e3;
        self.refresh.force_idr = false;
        if let Some(d) = self.dump.as_mut() {
            d.write_all(&self.au)?;
        }
        let recovery = std::mem::take(&mut self.refresh.recover_next) && !enc.is_keyframe;
        self.send_frame(enc.is_keyframe, recovery, capture_us)?;
        {
            let mut w = self.win.lock().unwrap();
            w.encode_ms.push(encode_ms);
            w.count(self.au.len(), enc.is_keyframe, recovery, enc.avg_qp);
            w.captured += u32::from(captured);
            if let Some(m) = map {
                w.map_delta = Some(m.delta);
                w.moving_share_sum += m.moving_share();
            }
        }
        // With the map the controller sees the encoder's base QP.
        let qp = match self.encode.static_map.as_mut() {
            Some(m) => m.observe(enc.avg_qp).round() as u32,
            None => enc.avg_qp,
        };
        self.steer_rate(qp)?;
        if enc.is_keyframe {
            self.refresh.last_idr = Some(self.frame);
        }
        if recovery {
            self.refresh.last_recovery = Some(self.frame);
        }
        self.frame += 1;
        self.clock.next_tick += self.clock.interval;
        if Instant::now() > self.clock.next_tick + self.clock.interval {
            self.clock.next_tick = Instant::now(); // fell behind: don't burst to catch up
        }
        Ok(())
    }

    /// The next frame's image, at its slot; whether it is a new one. The
    /// pointer's moves until then go out on their own.
    fn capture(&mut self, screen: &mut Screen) -> Result<bool> {
        let (stream, origin) = (self.stream, (screen.place.left, screen.place.top));
        let shared = &self.shared;
        let packetizer = &mut self.wire.packetizer;
        let captured = screen.source.grab(
            self.frame,
            &screen.frame_tex,
            self.clock.next_tick,
            self.clock.interval,
            |pointer| {
                // The pointer moved but the image did not: no frame, only the position.
                let update = shared
                    .cursor
                    .lock()
                    .unwrap()
                    .desktop(pointer, stream, origin);
                if let Some(u) = update {
                    send_cursor(shared, packetizer, u)?;
                }
                Ok(())
            },
        )?;
        if let Source::Desktop(d) = &screen.source
            && d.size_differs()
        {
            shared.stale.store(true, Ordering::Relaxed);
        }
        Ok(captured)
    }

    /// The pointer as this frame's capture saw it.
    fn send_pointer(&mut self, screen: &Screen) -> Result<()> {
        let (stream, origin) = (self.stream, (screen.place.left, screen.place.top));
        let shared = &self.shared;
        let packetizer = &mut self.wire.packetizer;
        let update = match &screen.source {
            Source::Desktop(d) => shared
                .cursor
                .lock()
                .unwrap()
                .desktop(&d.pointer, stream, origin),
            Source::Synthetic(_) if stream == 0 => Some(shared.cursor.lock().unwrap().synthetic(
                self.frame,
                screen.width,
                screen.height,
            )),
            Source::Synthetic(_) => None,
        };
        if let Some(u) = update {
            send_cursor(shared, packetizer, u)?;
        }
        Ok(())
    }

    /// The encoded frame (`au`), fragmented with FEC and paced out in this
    /// stream's lane (the sender numbers and seals it as it leaves).
    fn send_frame(&mut self, keyframe: bool, recovery: bool, capture_us: u64) -> Result<()> {
        let mut flags = Flags::empty();
        flags.set(Flags::KEYFRAME, keyframe);
        flags.set(Flags::RECOVERY, recovery);
        let mut dgrams = Vec::new();
        self.wire.packetizer.packetize(
            PacketKind::Video,
            self.stream,
            self.frame,
            flags,
            capture_us,
            &self.au,
            &mut dgrams,
        )?;
        self.shared.out.send_paced_stream(self.wire.lane, dgrams);
        Ok(())
    }

    /// The session's controller sees this frame (`qp`: its average QP, or
    /// with the map the base QP); the encoder follows its rate.
    fn steer_rate(&mut self, qp: u32) -> Result<()> {
        let rate = &self.shared.rate;
        rate.on_frame(self.au.len(), qp);
        let bps = rate.nominal_bps();
        if bps != 0 && bps != self.encode.encoder_bps {
            self.encode.encoder.set_bitrate(bps)?;
            self.encode.encoder_bps = bps;
        }
        Ok(())
    }
}
