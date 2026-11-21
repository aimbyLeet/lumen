//! The monitors told to the client (`Monitors`, the `StreamStart`s), the ones
//! it shows, the layout followed, and the question when there is none.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Result, bail};
use protocol::{ControlMessage, Monitor, VideoFormat};

use super::Session;
use super::rate::static_map;
use crate::streamer::{Setup, Stream};

/// What the client is told about the monitors, and what it shows.
pub(super) struct Monitors {
    /// `Monitors` and a `StreamStart` per stream: what a (re)starting client gets.
    pub(super) monitors_msg: ControlMessage,
    pub(super) start_msgs: Vec<ControlMessage>,
    /// The client answered this `Monitors`; when it was sent last.
    pub(super) monitors_acked: bool,
    pub(super) monitors_sent: Instant,
    /// The streams the client shows (a bit each), and each stream's pixels:
    /// the ceiling follows what is shown.
    pub(super) shown: u8,
    pub(super) stream_pixels: Vec<u64>,
    /// Where each stream's frame numbers go on after its streams start over,
    /// for the whole session: also a stream whose monitor was gone for a while
    /// (from 0 again, the client took its frames for old ones and dropped
    /// them until the numbers caught up: the monitor stayed black, 2026-10-30).
    pub(super) next_frames: [u32; protocol::MAX_STREAMS as usize],
    pub(super) layout_checked: Instant,
    /// No monitor to stream: what the client is told.
    pub(super) asking: crate::headless::Asking,
    /// The client's answer at the service (it makes the monitor and waits
    /// for Windows: seconds): whether it took, once it is done. Meanwhile
    /// the question goes on — the client's sign that the host is there.
    pub(super) choosing: Option<std::sync::mpsc::Receiver<bool>>,
}

impl Monitors {
    /// The client started over and was told everything again: `Monitors` is
    /// to be answered anew — if there is one. With no monitor to stream only
    /// the waiting message went, and `monitors_msg` is not for this client
    /// (the first `Bye`, or the monitors that are gone): not sent again.
    pub(super) fn told_again(&mut self) {
        self.monitors_acked = self.start_msgs.is_empty();
    }
}

impl Session<'_> {
    /// A stream for each of the station's screens; tells the client about
    /// them (`Monitors`, `StreamStart`s). All are shown until the client says.
    pub(super) fn start_streams(&mut self, caps: &protocol::Capabilities) -> Result<()> {
        let screens = std::mem::take(&mut self.st.screens);
        let mut monitors = Vec::new();
        self.monitors.start_msgs.clear();
        if screens.is_empty() {
            self.wait_for_monitor();
            return Ok(());
        }
        self.monitors.asking.stop();
        for (i, s) in screens.iter().enumerate() {
            let stream = i as u8;
            let format = VideoFormat::lumen_default(s.width as u16, s.height as u16, self.fps);
            if !caps.accepts(&format) {
                bail!("the client cannot decode {format:?}");
            }
            monitors.push(Monitor {
                stream,
                x: s.place.left,
                y: s.place.top,
                width: s.width,
                height: s.height,
            });
            self.monitors
                .start_msgs
                .push(ControlMessage::StreamStart { stream, format });
        }
        // Told before the first frame leaves: a client in its handshake drops
        // whatever comes before monitor 1's start.
        self.monitors.shown = (1u16 << screens.len()).wrapping_sub(1) as u8;
        self.monitors.stream_pixels = screens
            .iter()
            .map(|s| u64::from(s.width) * u64::from(s.height))
            .collect();
        self.follow_shown();
        self.monitors.monitors_msg = ControlMessage::Monitors {
            generation: self.st.generation,
            monitors,
        };
        self.monitors.monitors_acked = false;
        self.send_monitors();
        self.shared.stop = Arc::new(AtomicBool::new(false));
        for (i, screen) in screens.into_iter().enumerate() {
            let setup = Setup {
                fps: self.fps,
                preset: self.st.args.preset,
                rate_control: self.ceiling.rate_control,
                static_map: static_map(&self.st.args, self.fps),
                parity: self.pace.parity,
                dump: self.dumps.get_mut(i).and_then(Option::take),
                first_frame: self.monitors.next_frames.get(i).copied().unwrap_or(0),
                lane: if self.st.args.no_interleave {
                    0
                } else {
                    i as u8
                },
            };
            match Stream::start(i as u8, screen, setup, self.shared.clone()) {
                Ok(s) => self.streams.push(s),
                Err(e) => {
                    // The screens taken so far come back; the rest are reopened next time.
                    self.stop_streams();
                    self.st.screens.clear();
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Stops the streams; their screens go back to the station.
    pub(super) fn stop_streams(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let started = self.streams.len();
        let mut screens = Vec::new();
        for (i, mut s) in self.streams.drain(..).enumerate() {
            if let Some((screen, r, next)) = s.join() {
                if let Err(e) = r {
                    println!("stream: {e:#}");
                }
                screens.push(screen);
                self.monitors.next_frames[i] = self.monitors.next_frames[i].max(next);
            }
        }
        self.st.screens = if screens.len() == started {
            screens
        } else {
            Vec::new()
        };
    }

    /// Monitor 1's start first (a client in its handshake waits for that one
    /// and drops the rest), then `Monitors` and the other streams' starts.
    /// Every stream begins with an IDR.
    pub(super) fn send_monitors(&mut self) {
        let Some((first, rest)) = self.monitors.start_msgs.split_first() else {
            self.send_waiting();
            return;
        };
        self.send_control(first);
        self.send_control(&self.monitors.monitors_msg);
        for m in rest {
            self.send_control(m);
        }
        self.monitors.monitors_sent = Instant::now();
    }

    /// The monitors changed (Lumen Host «Применить», a mode change): every
    /// stream starts over for the new ones, the client gets a new `Monitors`
    /// (it starts its layout over: the owner's decision, 2026-10-30).
    pub(super) fn follow_layout(&mut self) -> Result<()> {
        // A physical monitor came or went: the service changes the monitors.
        if let Some(m) = self.st.monitors.look(Some(&self.inc.client_key), false) {
            self.monitors.asking.to_ask = crate::headless::to_ask(&m);
        }
        let stale = self.shared.stale.swap(false, Ordering::Relaxed);
        if !stale && !self.st.layout_changed() {
            if self.streams.is_empty() {
                // Still none: asked, or told there is none, as the service says now.
                self.wait_for_monitor();
            }
            return Ok(());
        }
        println!(
            "monitors changed{}: the streams start over",
            if stale { " (a monitor's mode)" } else { "" }
        );
        self.stop_streams();
        self.st.reopen_screens()?;
        for s in &mut self.st.screens {
            s.restart()?;
        }
        let caps = self.caps;
        self.start_streams(&caps)
    }

    /// No monitor to stream: the client is asked for one (the service has
    /// no set of its) or told there is none.
    fn wait_for_monitor(&mut self) {
        if let Some(w) = self.monitors.asking.wait() {
            println!("no monitor to stream: {w:?}");
        }
        self.monitors.shown = 0;
        self.monitors.stream_pixels.clear();
        self.monitors.monitors_acked = true;
        self.send_waiting();
    }

    pub(super) fn send_waiting(&mut self) {
        if let Some(msg) = self.monitors.asking.message() {
            self.send_control(&msg);
        }
    }

    /// The client's monitor for question `ask`: to the service (once), on
    /// a thread of its own — the session goes on asking meanwhile, else the
    /// client takes the host for gone (10 s; the problem host, 2026-11-12).
    pub(super) fn on_choice(
        &mut self,
        ask: u32,
        width: u32,
        height: u32,
        scale: u16,
    ) -> Result<()> {
        if !self.monitors.asking.take_answer(ask) {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let client = self.inc.client_key;
        std::thread::Builder::new()
            .name("client-choice".into())
            .spawn(move || {
                let _ = tx.send(crate::headless::choose(&client, width, height, scale));
            })?;
        self.monitors.choosing = Some(rx);
        Ok(())
    }

    /// The service is done with the client's answer: the streams for its
    /// monitor at once.
    pub(super) fn follow_choice(&mut self) -> Result<()> {
        let Some(rx) = &self.monitors.choosing else {
            return Ok(());
        };
        let took = match rx.try_recv() {
            Ok(took) => took,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        };
        self.monitors.choosing = None;
        if took {
            self.monitors.asking.to_ask = false;
            self.monitors.layout_checked = Instant::now();
            self.follow_layout()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitors(start_msgs: Vec<ControlMessage>) -> Monitors {
        let now = Instant::now();
        Monitors {
            monitors_msg: ControlMessage::Bye,
            start_msgs,
            monitors_acked: true,
            monitors_sent: now,
            shown: 0,
            stream_pixels: Vec::new(),
            next_frames: [0; protocol::MAX_STREAMS as usize],
            layout_checked: now,
            asking: crate::headless::Asking::new(false),
            choosing: None,
        }
    }

    #[test]
    fn monitors_are_told_again_only_when_there_are_some() {
        // No monitor to stream: nothing for the client to answer, nothing
        // (the first `Bye` least of all) is sent again.
        let mut none = monitors(Vec::new());
        none.told_again();
        assert!(none.monitors_acked);
        // Streams: `Monitors` again until the client answers it.
        let mut some = monitors(vec![ControlMessage::Bye]);
        some.told_again();
        assert!(!some.monitors_acked);
    }
}
