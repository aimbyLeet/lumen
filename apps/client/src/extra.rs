//! Mode 3: a window per monitor of the host after the first, each presented
//! by a thread of its own on a D3D device of its own.
//!
//! Swapchains on one device share its queue of frames: with three windows on
//! the render thread the present of the window shown last in a pass waited for
//! the screen's refresh (~11 ms at 90 Hz), and decoding and the other windows
//! with it — 34 fps a window. Now the render
//! thread only draws a monitor's picture (pointer and notice included) into a
//! texture shared with the window's device (keyed mutex, no CPU copy) and never
//! waits for it; the window's thread copies it into its swapchain and presents.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HANDLE, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::Interface;

use crate::present::Presenter;
use crate::render::NetworkVideo;
use crate::window::{self, Window};

/// The keyed mutex's one key: whoever holds it uses the texture.
const KEY: u64 = 0;
/// How long the window's thread waits for news before it looks at its
/// window's size and at `stop` again.
const IDLE_MS: u32 = 100;

/// An auto-reset event the render thread sets when there is a new picture.
struct Event(desktop::Handle);

impl Event {
    fn new() -> Result<Self> {
        let h = unsafe { CreateEventW(None, false, false, None)? };
        // SAFETY: just made, ours alone.
        Ok(Event(unsafe { desktop::Handle::from_raw(h) }))
    }

    fn handle(&self) -> HANDLE {
        self.0.get()
    }

    fn set(&self) {
        unsafe {
            let _ = SetEvent(self.handle());
        }
    }
}

/// Between the render thread and the window's thread.
struct Shared {
    /// Bumped after each picture drawn into the texture.
    generation: AtomicU64,
    /// The newest monitor frame in the texture: its capture, µs on our
    /// clock (0: none yet).
    capture_us: AtomicU64,
    /// The generation the window's thread copied last.
    copied: AtomicU64,
    stop: AtomicBool,
    wake: Event,
    /// The render loop's wake event (not ours): set once a picture is copied,
    /// the stream's next picture may be decoded.
    main_wake: usize,
}

/// Acquires the keyed mutex; false if it is not free within `ms`.
fn acquire(mutex: &IDXGIKeyedMutex, ms: u32) -> Result<bool> {
    // `AcquireSync` of the bindings turns WAIT_TIMEOUT (a success code) into Ok.
    let hr = unsafe { (Interface::vtable(mutex).AcquireSync)(mutex.as_raw(), KEY, ms) };
    if hr.0 == WAIT_TIMEOUT.0 as i32 {
        return Ok(false);
    }
    hr.ok().context("AcquireSync")?;
    Ok(true)
}

/// A window showing one more monitor of the host.
pub struct ExtraWindow {
    pub win: Window,
    pub stream: u8,
    /// News not drawn into the texture yet (the window's thread held it).
    pub dirty: bool,
    rtv: ID3D11RenderTargetView,
    mutex: IDXGIKeyedMutex,
    /// Capture time of the frame drawn last (only new frames are counted).
    last_capture: u64,
    /// The generation of a new frame handed to the thread, until it took it.
    handed: u64,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Result<Stats>>>,
}

impl ExtraWindow {
    /// Opens the window (on this thread: its messages come with the main
    /// window's) and its presenting thread; the picture is `width`×`height`.
    pub fn open(
        device: &ID3D11Device,
        title: &str,
        stream: u8,
        (width, height): (u32, u32),
        (x, y): (i32, i32),
        scene: crate::scene::Scene,
        main_wake: HANDLE,
    ) -> Result<ExtraWindow> {
        let win = Window::new_extra(title, width, height, x, y)?;
        crate::scene::set_for(win.hwnd().0 as isize, Some(scene));
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R10G10B10A2_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
        };
        let (texture, rtv, handle) = unsafe {
            let mut tex = None;
            device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex: ID3D11Texture2D = tex.context("shared texture")?;
            let mut rtv = None;
            device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
            let handle = tex.cast::<IDXGIResource>()?.GetSharedHandle()?;
            (tex, rtv.context("shared texture RTV")?, handle.0 as usize)
        };
        let mutex: IDXGIKeyedMutex = texture.cast()?;
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            capture_us: AtomicU64::new(0),
            copied: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            wake: Event::new()?,
            main_wake: main_wake.0 as usize,
        });
        let hwnd = win.hwnd().0 as isize;
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name(format!("monitor {}", usize::from(stream) + 1))
                .spawn(move || present_loop(hwnd, handle, &shared))?
        };
        Ok(ExtraWindow {
            win,
            stream,
            dirty: true,
            rtv,
            mutex,
            last_capture: 0,
            handed: 0,
            shared,
            worker: Some(worker),
        })
    }

    /// Draws the monitor's picture for the window's thread, unless that one
    /// holds the texture right now (then `dirty` stays: soon again). Never waits.
    pub fn update(&mut self, ctx: &ID3D11DeviceContext, video: &mut NetworkVideo) -> Result<()> {
        if !acquire(&self.mutex, 0)? {
            return Ok(());
        }
        let drawn = video.render_stream(ctx, &self.rtv, self.stream);
        unsafe { self.mutex.ReleaseSync(KEY)? };
        drawn?;
        let new_frame = video
            .stream_capture_us(self.stream)
            .filter(|&t| t != self.last_capture);
        if let Some(t) = new_frame {
            self.last_capture = t;
            self.shared.capture_us.store(t, Ordering::Relaxed);
        }
        let generation = self.shared.generation.fetch_add(1, Ordering::Release) + 1;
        if new_frame.is_some() {
            self.handed = generation;
        }
        self.shared.wake.set();
        self.dirty = false;
        Ok(())
    }
}

impl ExtraWindow {
    /// The thread took the new frame handed to it (once per frame).
    pub fn took(&mut self) -> bool {
        if self.handed == 0 || self.shared.copied.load(Ordering::Acquire) < self.handed {
            return false;
        }
        self.handed = 0;
        true
    }
}

impl Drop for ExtraWindow {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.wake.set();
        let stats = self.worker.take().map(|w| w.join());
        match stats {
            Some(Ok(Ok(mut s))) => s.report(self.stream),
            Some(Ok(Err(e))) => eprintln!("monitor {} window: {e:#}", self.stream + 1),
            _ => eprintln!("monitor {} window: its thread panicked", self.stream + 1),
        }
        crate::scene::set_for(self.win.hwnd().0 as isize, None);
        self.win.close_quietly();
    }
}

/// What a window's thread did, printed when the window closes.
struct Stats {
    presents: u64,
    since: Instant,
    /// Capture → presented, ms, per new frame.
    latency: Vec<f64>,
}

impl Stats {
    fn report(&mut self, stream: u8) {
        self.latency.sort_by(f64::total_cmp);
        let secs = self.since.elapsed().as_secs_f64();
        println!(
            "monitor {} window: {} presents in {secs:.1} s ({:.1}/s), {} frames; \
             capture -> presented ms: p50 {:.2}  p95 {:.2}  p99 {:.2}",
            usize::from(stream) + 1,
            self.presents,
            self.presents as f64 / secs.max(1e-3),
            self.latency.len(),
            transport::percentile(&self.latency, 0.5),
            transport::percentile(&self.latency, 0.95),
            transport::percentile(&self.latency, 0.99),
        );
    }
}

/// The window's thread: on news, waits for its swapchain to take a frame,
/// copies the newest picture from the shared texture and presents it.
fn present_loop(hwnd: isize, handle: usize, shared: &Shared) -> Result<Stats> {
    let gpu = capture::GpuDevice::open_nvidia()?;
    let (device, ctx) = (gpu.device, gpu.context);
    let texture: ID3D11Texture2D = unsafe {
        let mut t = None;
        device.OpenSharedResource(HANDLE(handle as *mut _), &mut t)?;
        t.context("open the shared texture")?
    };
    let mutex: IDXGIKeyedMutex = texture.cast()?;
    let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut _);
    let presenter = Presenter::new(&device, hwnd, window::layout_of(hwnd))?;
    let mut stats = Stats {
        presents: 0,
        since: Instant::now(),
        latency: Vec::new(),
    };
    let (mut seen, mut last_capture) = (0, 0);
    loop {
        unsafe { WaitForSingleObject(shared.wake.handle(), IDLE_MS) };
        if shared.stop.load(Ordering::Relaxed) {
            return Ok(stats);
        }
        let layout = window::layout_of(hwnd);
        let resized = layout != presenter.layout();
        if resized {
            presenter.set_layout(layout)?;
        }
        if !resized && shared.generation.load(Ordering::Acquire) == seen {
            continue;
        }
        // The newest picture once the swapchain can take it (one refresh at most).
        presenter.wait_for_frame();
        seen = shared.generation.load(Ordering::Acquire);
        let capture = shared.capture_us.load(Ordering::Relaxed);
        let rtv = presenter.render_target()?;
        let target = unsafe { rtv.GetResource()? };
        // The render thread holds it only to queue a draw.
        if !acquire(&mutex, 1000)? {
            anyhow::bail!("the shared texture stayed taken for 1 s");
        }
        unsafe {
            ctx.CopyResource(&target, &texture);
            mutex.ReleaseSync(KEY)?;
        }
        shared.copied.store(seen, Ordering::Release);
        unsafe {
            let _ = SetEvent(HANDLE(shared.main_wake as *mut _));
        }
        drop((rtv, target));
        presenter.present(&ctx, 0)?;
        stats.presents += 1;
        if capture != 0 && capture != last_capture {
            last_capture = capture;
            let now = transport::wall_clock_us();
            transport::push_recent(
                &mut stats.latency,
                (now as i64 - capture as i64) as f64 / 1e3,
                transport::RECENT_SAMPLES,
            );
        }
    }
}
