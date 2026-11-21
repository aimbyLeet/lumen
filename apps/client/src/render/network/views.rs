//! The live video: the host's monitors and how the window shows them
//! (modes 1, 2, 3).

use super::*;

/// The host's monitors, how the window shows them, their pictures not shown 1:1.
pub(super) struct MonitorView {
    /// The host's monitors as the window shows them, and the `Monitors`
    /// generation it was made of (none yet: monitor 1 alone).
    pub(super) scene: Scene,
    pub(super) scene_of: Option<u32>,
    /// Several monitors: their pictures, composed.
    pub(super) composite: Option<Composite>,
    /// The host's monitors as it said last; how the window shows them.
    pub(super) monitors: Vec<protocol::Monitor>,
    pub(super) mode: Mode,
    /// The mode to start in once the host tells its monitors: for these
    /// monitors only (the card's memory), or any (`--view`, checks).
    pub(super) start_mode: Option<(Option<Vec<protocol::Monitor>>, Mode)>,
    /// The newest picture of each monitor not shown 1:1 now (the one shown
    /// is `current`): on a switch it stays until the monitor's first frame.
    pub(super) pictures: Vec<Option<Decoded>>,
    /// A monitor was asked for: when, which (its first frame is reported).
    pub(super) switched: Option<(Instant, u8)>,
    /// Several monitors: when each stream's newest picture was decoded, while
    /// it is not shown yet (its next one waits up to [`STREAM_HOLD`]).
    pub(super) unshown: [Option<Instant>; MAX_STREAMS],
    /// Mode 3: streams with something new to show (a bit each) since the
    /// live loop last looked; Ctrl+Alt+Shift+F was pressed.
    pub(super) dirty_streams: u8,
    pub(super) group_borderless: bool,
}

impl NetworkVideo {
    /// The host told its monitors (anew): the scene, a decoder per monitor;
    /// pictures of monitors that changed are gone.
    pub(super) fn follow_monitors(&mut self) -> Result<bool> {
        let told = self.conn.monitors.lock().unwrap().clone();
        let Some((generation, monitors)) = told else {
            return Ok(false);
        };
        if self.mon.scene_of == Some(generation) || monitors.is_empty() {
            return Ok(false);
        }
        self.mon.scene_of = Some(generation);
        let scene = Scene::of(&monitors);
        println!(
            "monitors of the host: {}",
            monitors
                .iter()
                .map(|m| format!("{}x{} at {},{}", m.width, m.height, m.x, m.y))
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.dec.decoders.clear();
        for m in &monitors {
            let i = usize::from(m.stream);
            if self.dec.decoders.len() <= i {
                self.dec.decoders.resize_with(i + 1, || None);
            }
            self.dec.decoders[i] = Some(Decoder::on(&self.dec.gpu, &self.dec.device)?);
            self.dec.fresh_decoders |= 1 << m.stream;
        }
        self.present.current = None;
        self.mon.pictures.clear();
        if let Some(c) = self.mon.composite.as_mut() {
            c.clear();
        }
        // New monitors: mode 1 anew (the owner's decision, 2026-10-30).
        scene::host_changed();
        self.mon.monitors = monitors;
        self.mon.mode = match self.mon.start_mode.take() {
            Some((for_monitors, m))
                if self.mon.monitors.len() > 1
                    && for_monitors
                        .as_ref()
                        .is_none_or(|f| *f == self.mon.monitors) =>
            {
                m
            }
            _ => Mode::All,
        };
        drop(scene);
        self.apply_mode()?;
        Ok(true)
    }

    /// The scene for the mode: every monitor (mode 1) or one (mode 2); the
    /// host encodes what is shown.
    pub(super) fn apply_mode(&mut self) -> Result<()> {
        let shown_before = self.shown_stream();
        let scene = match self.mon.mode {
            Mode::All => Scene::of(&self.mon.monitors),
            Mode::One(i) => Scene::of(&self.mon.monitors[i..=i]),
            // The main window: monitor 1; the others have windows of their own.
            Mode::Windows => Scene::of(&self.mon.monitors[..1]),
        };
        crate::window::set_group(self.mon.mode == Mode::Windows);
        if scene.is_multi() && self.mon.composite.is_none() {
            self.mon.composite = Some(Composite::new(&self.dec.device)?);
        }
        let mask = match self.mon.mode {
            Mode::All | Mode::Windows => 0,
            Mode::One(i) => 1 << self.mon.monitors[i].stream,
        };
        self.conn
            .shown
            .store(mask, std::sync::atomic::Ordering::Relaxed);
        scene::set(scene.clone());
        self.mon.scene = scene;
        // The picture shown 1:1 is another monitor's now: keep the old one's,
        // show the new one's last picture until its first frame.
        let shown_now = self.shown_stream();
        if shown_now != shown_before {
            let slot = |v: &mut Vec<Option<Decoded>>, s: u8| {
                let i = usize::from(s);
                if v.len() <= i {
                    v.resize_with(i + 1, || None);
                }
                i
            };
            let old = slot(&mut self.mon.pictures, shown_before);
            self.mon.pictures[old] = self.present.current.take();
            let new = slot(&mut self.mon.pictures, shown_now);
            self.present.current = self.mon.pictures[new].take();
        }
        // Redrawn at once (not counted as a new frame).
        self.pointer.local_moved = true;
        Ok(())
    }

    /// The monitor whose picture is `current`: mode 2's, else monitor 1.
    pub(super) fn shown_stream(&self) -> u8 {
        match self.mon.mode {
            Mode::One(i) => self.mon.monitors.get(i).map_or(0, |m| m.stream),
            Mode::All | Mode::Windows => self.mon.monitors.first().map_or(0, |m| m.stream),
        }
    }

    /// Ctrl+Alt+Shift+L: mode 1 → mode 2 (monitor 1) → mode 3 → mode 1. One
    /// monitor at the host: nothing changes.
    pub(super) fn next_mode(&mut self) -> Result<()> {
        if self.mon.monitors.len() < 2 {
            return Ok(());
        }
        self.mon.mode = match self.mon.mode {
            Mode::All => Mode::One(0),
            Mode::One(_) => Mode::Windows,
            Mode::Windows => Mode::All,
        };
        println!("view: {:?}", self.mon.mode);
        self.mon.switched = Some((Instant::now(), self.shown_stream()));
        self.apply_mode()
    }

    /// Mode 2: monitor number `i` + 1 (a circle was clicked).
    pub(super) fn show_one(&mut self, i: usize) -> Result<()> {
        if self.mon.mode == Mode::One(i) || i >= self.mon.monitors.len() {
            return Ok(());
        }
        self.mon.mode = Mode::One(i);
        println!("view: {:?}", self.mon.mode);
        self.mon.switched = Some((Instant::now(), self.shown_stream()));
        self.apply_mode()
    }

    /// Mode 3: the monitors that get windows of their own (all but monitor 1):
    /// stream, size; empty in the other modes.
    pub fn extra_windows(&self) -> Vec<protocol::Monitor> {
        match self.mon.mode {
            Mode::Windows => self.mon.monitors.iter().skip(1).copied().collect(),
            _ => Vec::new(),
        }
    }

    /// One monitor's frames only (mode 2, or one monitor at the host): a
    /// decoded picture may wait for the swapchain before the next is decoded.
    /// With several, frames are decoded as they come (others' windows wait).
    pub fn holds_pictures(&self) -> bool {
        self.mon.monitors.len() < 2 || matches!(self.mon.mode, Mode::One(_))
    }

    /// Streams whose newest picture waits to be shown, still within [`STREAM_HOLD`].
    pub(super) fn held_streams(&self) -> u8 {
        let now = Instant::now();
        self.mon
            .unshown
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_some_and(|t| now < t + STREAM_HOLD))
            .fold(0, |m, (s, _)| m | 1 << s)
    }

    /// Several monitors: when a held picture stops waiting (the loop comes
    /// back then to decode the next one).
    pub fn hold_deadline(&self) -> Option<Instant> {
        if self.holds_pictures() {
            return None;
        }
        let now = Instant::now();
        self.mon
            .unshown
            .iter()
            .flatten()
            .map(|t| *t + STREAM_HOLD)
            .filter(|&until| now < until)
            .min()
    }

    /// Mode 3: monitor `stream`'s window took its picture (the next may come).
    pub fn taken(&mut self, stream: u8) {
        self.mon.unshown[usize::from(stream) % MAX_STREAMS] = None;
    }

    /// Streams with something new to show since the last call (a bit each).
    pub fn take_dirty_streams(&mut self) -> u8 {
        std::mem::take(&mut self.mon.dirty_streams)
    }

    /// Ctrl+Alt+Shift+F in mode 3: for the live loop (it has the windows).
    pub fn take_group_borderless(&mut self) -> bool {
        std::mem::take(&mut self.mon.group_borderless)
    }
}
