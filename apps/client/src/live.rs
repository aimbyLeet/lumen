//! Present loop for a live stream: decode on arrival, present as soon as the
//! swapchain can take a frame.
//!
//! One wait covers everything that can make the picture change: the network
//! thread's wake event (frame or cursor update), window messages (local
//! mouse), and — only while there is something new to show — the swapchain's
//! frame-latency waitable. Frames are presented with sync interval 0; the flip
//! model shows the newest one at the next vblank, so nothing waits for a
//! render tick before being decoded.
//!
//! A decoded picture is presented before the next one is decoded: frames that
//! arrive in a burst would otherwise be decoded in one go (a 4K decode takes
//! ~13 ms on Pascal) and only the last one shown. The wait is bounded by
//! [`HOLD_MAX`], in case the swapchain takes no frames (hidden window).
//! Further behind (start, after a stall) nothing waits: each picture is
//! presented if the swapchain takes it right then, else decoded over.
//!
//! While Windows runs a modal loop of the window (it is being dragged), the
//! window's timer drives the same decode and present instead.
//!
//! Mode 3 (a window per monitor of the host): the other monitors' windows
//! ([`ExtraWindow`]) take their messages here; their pictures are drawn here
//! and presented by threads of their own (`crate::extra`).

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};

use crate::extra::ExtraWindow;
use crate::memory::{Memory, View};
use crate::present::Presenter;
use crate::render::{Decode, NetworkVideo};
use crate::scene::Scene;
use crate::window::{self, Window};

/// The windows the mode wants: mode 3 — one per monitor after the first, all
/// on this machine's main monitor (cascaded from the main window); the user
/// spreads them. Otherwise none.
fn follow_extras(
    extras: &mut Vec<ExtraWindow>,
    main: &Window,
    ctx: &ID3D11DeviceContext,
    video: &NetworkVideo,
) -> Result<()> {
    let wanted = video.extra_windows();
    if extras
        .iter()
        .map(|e| e.stream)
        .eq(wanted.iter().map(|m| m.stream))
    {
        return Ok(());
    }
    extras.clear();
    let device = unsafe { ctx.GetDevice()? };
    let (x, y) = main.position();
    for (k, m) in wanted.iter().enumerate() {
        let step = 48 * (k as i32 + 1);
        extras.push(ExtraWindow::open(
            &device,
            &format!("Lumen client: monitor {}", usize::from(m.stream) + 1),
            m.stream,
            (m.width, m.height),
            (x + step, y + step),
            Scene::of(std::slice::from_ref(m)),
            video.wake_event(),
        )?);
    }
    Ok(())
}

/// Every window on a monitor of its own (Ctrl+Alt+Shift+F in mode 3 spreads
/// them borderless; two on one: refused, the owner's decision 2026-10-30).
fn all_apart(monitors: &[isize]) -> bool {
    monitors
        .iter()
        .enumerate()
        .all(|(i, m)| !monitors[..i].contains(m))
}

/// Ctrl+Alt+Shift+F in mode 3: all borderless, each on the monitor of this
/// machine under its centre; or all framed again.
fn group_borderless(
    main: &mut Window,
    extras: &mut [ExtraWindow],
    video: &mut NetworkVideo,
) -> Result<()> {
    if main.is_borderless() || extras.iter().any(|e| e.win.is_borderless()) {
        main.set_borderless(false);
        extras.iter_mut().for_each(|e| e.win.set_borderless(false));
        return Ok(());
    }
    let monitors: Vec<isize> = std::iter::once(main.monitor())
        .chain(extras.iter().map(|e| e.win.monitor()))
        .collect();
    if !all_apart(&monitors) {
        println!("borderless: two windows on one monitor, refused");
        video.say("two_windows_on_one_monitor");
        return Ok(());
    }
    main.set_borderless(true);
    extras.iter_mut().for_each(|e| e.win.set_borderless(true));
    Ok(())
}

/// The same monitors at the host and the mode the memory gave: the windows
/// go where they were (mode 3's first, the main one last: it stays in front).
fn restore(v: &View, main: &mut Window, extras: &mut [ExtraWindow], video: &NetworkVideo) {
    if v.monitors != video.monitors() || v.mode != video.mode() {
        return;
    }
    if v.windows.len() != 1 + extras.len() {
        return;
    }
    for (e, p) in extras.iter_mut().zip(&v.windows[1..]) {
        e.win.restore(p);
    }
    main.restore(&v.windows[0]);
    println!("view: as the last session ({:?})", v.mode);
}

/// The view now, for the memory.
fn view(main: &Window, extras: &[ExtraWindow], video: &NetworkVideo) -> View {
    View {
        monitors: video.monitors().to_vec(),
        mode: video.mode(),
        windows: std::iter::once(main)
            .chain(extras.iter().map(|e| &e.win))
            .filter_map(Window::place)
            .collect(),
    }
}

/// How often the view is compared with the memory (written when it changed).
const KEEP_EVERY: Duration = Duration::from_millis(500);
/// Upper bound for one wait, so a closed connection is noticed.
const WAIT_MS: u32 = 100;
/// A pass of the loop longer than this is written down, with where the time
/// went: the render loop fell behind the network for many minutes and the log
/// could not say why (2026-11-13).
const SLOW_PASS: Duration = Duration::from_millis(250);
/// Slow passes written down at most (then only counted).
const SLOW_PASSES_LOGGED: u32 = 50;
/// Mode 3: how soon news for a window whose thread held its texture is drawn again.
const EXTRA_RETRY_MS: u32 = 2;
/// A decoded picture waits this long at most for the swapchain before newer
/// ones are decoded over it. A shown frame waited at most 5 ms through WG.
const HOLD_MAX: Duration = Duration::from_millis(25);

/// `--check-cursor`: every `every` presented frames, whether the host's pointer
/// is drawn where expected (synthetic host).
pub struct CursorCheck {
    pub every: u64,
    pub checks: u32,
    pub wrong: u32,
}

/// A wait until `until`, ms: at least 1, at most [`WAIT_MS`].
fn ms_until(until: Instant) -> u32 {
    let left = until.saturating_duration_since(Instant::now()).as_micros();
    left.div_ceil(1000).clamp(1, u128::from(WAIT_MS)) as u32
}

/// News for mode 3's other windows: drawn for their threads, which present.
fn update_extras(
    extras: &mut [ExtraWindow],
    ctx: &ID3D11DeviceContext,
    video: &mut NetworkVideo,
) -> Result<()> {
    let news = video.take_dirty_streams();
    for e in extras.iter_mut() {
        if e.took() {
            video.taken(e.stream);
        }
        e.dirty |= news & (1 << e.stream) != 0;
        if e.dirty {
            e.update(ctx, video)?;
        }
    }
    Ok(())
}

struct Live<'a> {
    presenter: &'a Presenter,
    ctx: &'a ID3D11DeviceContext,
    video: &'a mut NetworkVideo,
    cursor: Option<&'a mut CursorCheck>,
    presented: u64,
    /// There is news the swapchain has not shown yet.
    dirty: bool,
    /// The first error of a modal tick (they cannot return one).
    error: Option<anyhow::Error>,
}

impl Live<'_> {
    /// Waits for anything that can change the picture. True if the swapchain
    /// took a slot: a present must follow. The swapchain goes first: when it is
    /// ready and there is news, present before anything else. Waiting on it
    /// consumes a slot, so only then. While a new picture waits for it, nothing
    /// new is decoded.
    fn wait(&self, wake: HANDLE, extras: &[ExtraWindow]) -> bool {
        let swapchain_first = self.dirty;
        let hold = self.holding();
        let handles: Vec<HANDLE> = match (swapchain_first, hold) {
            (true, Some(_)) => vec![self.presenter.waitable()],
            (true, None) => vec![self.presenter.waitable(), wake],
            (false, _) => vec![wake],
        };
        let timeout = hold.map_or(WAIT_MS, ms_until);
        // Several monitors: a held picture stops waiting then.
        let timeout = self
            .video
            .hold_deadline()
            .map_or(timeout, |until| timeout.min(ms_until(until)));
        // Mode 3: another window's news waits for its texture: soon again.
        let timeout = if extras.iter().any(|e| e.dirty) {
            timeout.min(EXTRA_RETRY_MS)
        } else {
            timeout
        };
        let r = unsafe { MsgWaitForMultipleObjects(Some(&handles), false, timeout, QS_ALLINPUT) };
        swapchain_first && r == WAIT_OBJECT_0
    }

    /// The picture decoded last still waits for the swapchain, and may.
    fn holding(&self) -> Option<Instant> {
        if !self.video.holds_pictures() {
            return None;
        }
        self.video
            .unshown_since()
            .map(|t| t + HOLD_MAX)
            .filter(|&until| Instant::now() < until)
    }

    /// `present_next`: the swapchain took a slot; a picture not shown yet goes
    /// first, before anything is decoded over it.
    fn update(&mut self, present_next: bool) -> Result<()> {
        // Resized, maximised, made borderless: redraw at the new layout.
        let layout = window::current_layout();
        if layout != self.presenter.layout() {
            self.presenter.set_layout(layout)?;
            println!("swapchain: {}", self.presenter.describe()?);
            self.dirty = true;
        }
        let hold = self.video.holds_pictures()
            && (self.holding().is_some() || (present_next && self.video.is_fresh()));
        let decode = if hold {
            Decode::Hold
        } else {
            Decode::UntilPicture
        };
        self.dirty |= self.video.update(self.ctx, decode)?;
        Ok(())
    }

    /// Must follow every successful wait on the swapchain's waitable: the
    /// wait took a slot that only a present gives back.
    fn present(&mut self) -> Result<()> {
        let begin = transport::wall_clock_us();
        let rtv = self.presenter.render_target()?;
        self.video.render(self.ctx, &rtv)?;
        drop(rtv);
        self.check_cursor()?;
        self.presenter.present(self.ctx, 0)?;
        self.video.log_present(begin, transport::wall_clock_us());
        self.video.presented();
        self.presented += 1;
        self.dirty = false;
        Ok(())
    }

    /// Reads the picture back (slow): before the present, which may discard it.
    fn check_cursor(&mut self) -> Result<()> {
        let Some(c) = self.cursor.as_deref_mut() else {
            return Ok(());
        };
        if !self.presented.is_multiple_of(c.every.max(1)) {
            return Ok(());
        }
        let Some((x, y, want)) = self.video.probe() else {
            return Ok(());
        };
        c.checks += 1;
        let (w, h) = self.presenter.layout().picture;
        let ok = (0..w as i32).contains(&x)
            && (0..h as i32).contains(&y)
            && self.presenter.read_back(self.ctx)?[(y as u32 * w + x as u32) as usize] == want;
        if !ok {
            c.wrong += 1;
            eprintln!(
                "frame {}: cursor pixel ({x}, {y}) is not {want:?}",
                self.presented
            );
        }
        Ok(())
    }

    /// One step inside a modal loop: decode what arrived, present it if the
    /// swapchain can take a frame right now.
    fn modal_tick(&mut self) {
        if self.error.is_some() {
            return;
        }
        let ready =
            || unsafe { WaitForSingleObject(self.presenter.waitable(), 0) } == WAIT_OBJECT_0;
        let r = if self.dirty && self.video.is_fresh() && ready() {
            self.update(true).and_then(|()| self.present())
        } else {
            self.update(false).and_then(|()| {
                if self.dirty && ready() {
                    self.present()?;
                }
                Ok(())
            })
        };
        if let Err(e) = r {
            self.error = Some(e);
        }
    }
}

/// Runs until the window closes, the session ends or `max_frames` pictures of
/// the host were shown (not presents: the pointer alone makes those too, so
/// a moving mouse would end a check early). Returns the presents.
pub fn run(
    win: &mut Window,
    presenter: &Presenter,
    ctx: &ID3D11DeviceContext,
    video: &mut NetworkVideo,
    max_frames: Option<u64>,
    cursor: Option<&mut CursorCheck>,
    mut memory: Option<&mut Memory>,
) -> Result<u64> {
    let wake = video.wake_event();
    let mut live = Live {
        presenter,
        ctx,
        video,
        cursor,
        presented: 0,
        dirty: false,
        error: None,
    };
    let mut inputs: Vec<(u32, usize, isize)> = Vec::new();
    let mut scene_seen = crate::scene::host_generation();
    let mut extras: Vec<ExtraWindow> = Vec::new();
    // The windows' remembered places wait for the host's first monitors.
    let mut remembered = memory.as_ref().and_then(|m| m.remembered.clone());
    let mut kept_at = Instant::now();
    let mut slow = 0u32;
    loop {
        if max_frames.is_some_and(|n| live.video.shown() >= n) || live.video.is_closed() {
            break;
        }
        let t0 = Instant::now();
        let ready = live.wait(wake, &extras);
        let t1 = Instant::now();
        let open = win.pump_with(|m, w, l| inputs.push((m, w, l)), &mut || live.modal_tick());
        let t2 = Instant::now();
        if let Some(e) = live.error.take() {
            return Err(e);
        }
        if !open {
            break;
        }
        for (m, w, l) in inputs.drain(..) {
            live.video.on_input(m, w, l);
        }
        live.update(ready)?;
        follow_extras(&mut extras, win, ctx, live.video)?;
        if live.video.take_group_borderless() {
            group_borderless(win, &mut extras, live.video)?;
        }
        update_extras(&mut extras, ctx, live.video)?;
        // The host's monitors changed: the window takes their layout's aspect.
        if crate::scene::host_generation() != scene_seen {
            scene_seen = crate::scene::host_generation();
            win.fit_scene();
            if let Some(v) = remembered.take() {
                restore(&v, win, &mut extras, live.video);
            }
        }
        if let Some(m) = memory.as_deref_mut()
            && kept_at.elapsed() >= KEEP_EVERY
        {
            kept_at = Instant::now();
            m.keep(view(win, &extras, live.video));
        }
        let t3 = Instant::now();
        if ready {
            live.present()?;
        }
        let t4 = Instant::now();
        // The wait is bounded by `WAIT_MS`: past it plus the limit, it is slow too.
        let waited = (t1 - t0).saturating_sub(Duration::from_millis(u64::from(WAIT_MS)));
        if waited + (t4 - t1) > SLOW_PASS {
            slow += 1;
            if slow <= SLOW_PASSES_LOGGED {
                let ms = |d: Duration| d.as_secs_f64() * 1e3;
                println!(
                    "slow render pass: wait {:.0} ms, window messages {:.0} ms, decode and update {:.0} ms, present {:.0} ms",
                    ms(t1 - t0),
                    ms(t2 - t1),
                    ms(t3 - t2),
                    ms(t4 - t3)
                );
            }
        }
    }
    if slow > SLOW_PASSES_LOGGED {
        println!("slow render passes: {slow} in all");
    }
    Ok(live.presented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borderless_needs_a_monitor_per_window() {
        assert!(all_apart(&[1, 2, 3]));
        assert!(all_apart(&[7]));
        assert!(!all_apart(&[1, 2, 1]), "two on monitor 1");
        assert!(!all_apart(&[5, 5]));
    }
}
