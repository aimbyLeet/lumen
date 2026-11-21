use crate::regions::{Changes, Rect};
use anyhow::{Context, Result, bail};
use gpu_convert::{desc_of, read_rows, texture_desc};
use windows::Win32::Foundation::{HANDLE, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetThreadDesktop,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::Interface;

const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// D3D11 device on the GPU that drives the captured output.
#[derive(Clone)]
pub struct GpuDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter: IDXGIAdapter1,
    pub adapter_name: String,
    pub is_nvidia: bool,
}

/// Desktop Duplication requires a per-monitor DPI aware process. Call once, early.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

thread_local! {
    /// The desktop this thread was moved to by [`follow_input_desktop`]: kept open while in use.
    static THREAD_DESKTOP: std::cell::Cell<Option<OwnedDesktop>> = const { std::cell::Cell::new(None) };
}

/// A desktop handle, closed on drop.
struct OwnedDesktop(HDESK);

impl Drop for OwnedDesktop {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseDesktop(self.0);
        }
    }
}

/// Sleeps a little (at most 50 ms) so that a failing capture does not spin.
fn pause(timeout_ms: u32) -> Acquired {
    std::thread::sleep(std::time::Duration::from_millis(u64::from(
        timeout_ms.min(50),
    )));
    Acquired::Timeout
}

/// Moves the calling thread to the desktop that receives input now, if it is
/// another one: `Default`, or `Winlogon` (sign-in, lock screen, UAC prompt,
/// Ctrl+Alt+Del). Desktop Duplication and `SendInput` work on the thread's
/// desktop only. A process of the signed-in user may not open `Winlogon`
/// (nothing changes then); one running as SYSTEM in the console session may.
/// Returns the input desktop's name when the thread moved. Fails if the
/// thread owns windows or hooks (Windows refuses to move it).
pub fn follow_input_desktop() -> Result<Option<String>> {
    // GENERIC_ALL, as Desktop Duplication and SendInput need.
    let access = DESKTOP_ACCESS_FLAGS(0x1000_0000);
    let Ok(desk) = (unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, access) }) else {
        return Ok(None); // not ours to open (a user process at the lock screen)
    };
    let desk = OwnedDesktop(desk);
    let name = desktop_name(desk.0);
    if name == current_desktop_name() {
        return Ok(None);
    }
    if let Err(e) = unsafe { SetThreadDesktop(desk.0) } {
        return Err(e).with_context(|| format!("SetThreadDesktop({name})"));
    }
    // The old one closes as it is replaced.
    THREAD_DESKTOP.set(Some(desk));
    Ok(Some(name))
}

fn desktop_name(desk: HDESK) -> String {
    let mut buf = [0u16; 128];
    let mut needed = 0u32;
    let ok = unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
    };
    if ok.is_err() {
        return String::new();
    }
    wide_to_string(&buf)
}

/// The name of the calling thread's desktop.
fn current_desktop_name() -> String {
    match unsafe { GetThreadDesktop(GetCurrentThreadId()) } {
        Ok(d) => desktop_name(d),
        Err(_) => String::new(),
    }
}

/// Information about one monitor attached to a GPU.
#[derive(Clone, Debug)]
pub struct OutputInfo {
    pub adapter_index: u32,
    pub output_index: u32,
    pub adapter_name: String,
    /// On an NVIDIA GPU: only there can its picture be encoded.
    pub is_nvidia: bool,
    pub device_name: String,
    /// Top-left on the virtual desktop: (0, 0) is the primary monitor.
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Lists all monitors on all adapters.
pub fn list_outputs() -> Result<Vec<OutputInfo>> {
    let mut out = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            let desc = adapter.GetDesc1()?;
            let mut oi = 0;
            while let Ok(output) = adapter.EnumOutputs(oi) {
                let od = output.GetDesc()?;
                let r = od.DesktopCoordinates;
                out.push(OutputInfo {
                    adapter_index: ai,
                    output_index: oi,
                    adapter_name: wide_to_string(&desc.Description),
                    is_nvidia: desc.VendorId == NVIDIA_VENDOR_ID,
                    device_name: wide_to_string(&od.DeviceName),
                    left: r.left,
                    top: r.top,
                    width: (r.right - r.left) as u32,
                    height: (r.bottom - r.top) as u32,
                });
                oi += 1;
            }
            ai += 1;
        }
    }
    Ok(out)
}

/// The primary monitor (at (0, 0) of the virtual desktop), if DXGI sees it.
pub fn primary_output() -> Option<OutputInfo> {
    list_outputs()
        .ok()?
        .into_iter()
        .find(|o| o.left == 0 && o.top == 0)
}

fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let levels = [D3D_FEATURE_LEVEL_11_1];
    let mut got: D3D_FEATURE_LEVEL = Default::default();
    unsafe {
        D3D11CreateDevice(
            &adapter.cast::<IDXGIAdapter>()?,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut got),
            Some(&mut context),
        )
        .context("D3D11CreateDevice")?;
    }
    let device: ID3D11Device = device.context("no device")?;
    // Encoder and capture share the device from different call sites; make it safe.
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    Ok((device, context.context("no context")?))
}

impl GpuDevice {
    /// Creates a device on adapter `adapter_index` (DXGI enumeration order).
    pub fn open(adapter_index: u32) -> Result<GpuDevice> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory
                .EnumAdapters1(adapter_index)
                .context("adapter not found")?;
            Self::from_adapter(adapter)
        }
    }

    /// Creates a device on the first NVIDIA adapter.
    pub fn open_nvidia() -> Result<GpuDevice> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                if adapter.GetDesc1()?.VendorId == NVIDIA_VENDOR_ID {
                    return Self::from_adapter(adapter);
                }
                i += 1;
            }
        }
        bail!("no NVIDIA adapter found")
    }

    fn from_adapter(adapter: IDXGIAdapter1) -> Result<GpuDevice> {
        let desc = unsafe { adapter.GetDesc1()? };
        let (device, context) = create_device(&adapter)?;
        Ok(GpuDevice {
            device,
            context,
            adapter,
            adapter_name: wide_to_string(&desc.Description),
            is_nvidia: desc.VendorId == NVIDIA_VENDOR_ID,
        })
    }
}

/// Captures one monitor with DXGI Desktop Duplication.
pub struct DesktopDuplicator {
    gpu: GpuDevice,
    output: IDXGIOutput5,
    dup: Option<IDXGIOutputDuplication>,
    /// Top-left of the monitor on the virtual desktop (may be negative).
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Updated by every `acquire_into`.
    pub pointer: Pointer,
    holding_frame: bool,
    /// Reopening failed last time (reported once).
    reopen_failing: bool,
    /// The output's frames are not the size of the caller's texture (reported once).
    size_differs: bool,
    /// When the duplication was made and whether an image came since: one
    /// that gives none is made anew (see [`NO_IMAGE_REOPEN`]).
    opened_at: std::time::Instant,
    imaged: bool,
    /// What changed in the image since [`take_changes`](Self::take_changes).
    changes: Changes,
    /// Reused buffers for the frame metadata.
    move_rects: Vec<DXGI_OUTDUPL_MOVE_RECT>,
    dirty_rects: Vec<RECT>,
}

/// A new duplication's first frame is the whole image; none for this long
/// (a monitor added a moment ago, still changing its mode) and it is made
/// anew. A still desktop gives no frames after the first, so only until then.
const NO_IMAGE_REOPEN: std::time::Duration = std::time::Duration::from_secs(1);

/// Mouse pointer as reported by Desktop Duplication (not part of the image).
#[derive(Clone, Debug, Default)]
pub struct Pointer {
    pub visible: bool,
    /// Position of the shape's top-left corner, relative to the captured monitor.
    pub x: i32,
    pub y: i32,
    /// Increments whenever `shape` changes.
    pub shape_serial: u32,
    pub shape: Option<PointerShape>,
}

#[derive(Clone, Debug)]
pub struct PointerShape {
    /// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE`: 1 monochrome, 2 colour, 4 masked colour.
    pub kind: u32,
    pub width: u32,
    /// For monochrome: twice the visible height (AND mask, then XOR mask).
    pub height: u32,
    pub pitch: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub data: Vec<u8>,
}

/// A Windows rectangle as a [`Rect`].
fn rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

/// Result of [`DesktopDuplicator::acquire_into`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acquired {
    /// The desktop image changed; `dst` now contains the new frame.
    NewFrame { accumulated_frames: u32 },
    /// Only the mouse pointer moved (not part of the image in our design).
    PointerOnly,
    /// Nothing changed within the timeout. `dst` still holds the previous frame.
    Timeout,
}

impl DesktopDuplicator {
    /// Opens duplication of `output_index` on `adapter_index` (see [`list_outputs`]).
    pub fn new(adapter_index: u32, output_index: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory
                .EnumAdapters1(adapter_index)
                .context("adapter not found")?;
            let desc = adapter.GetDesc1()?;
            let output = adapter
                .EnumOutputs(output_index)
                .context("output not found")?;
            let output: IDXGIOutput5 = output
                .cast()
                .context("IDXGIOutput5 needs Windows 10 1703+")?;
            let od = output.GetDesc()?;
            let r = od.DesktopCoordinates;
            let (device, context) = create_device(&adapter)?;
            let gpu = GpuDevice {
                device,
                context,
                adapter,
                adapter_name: wide_to_string(&desc.Description),
                is_nvidia: desc.VendorId == NVIDIA_VENDOR_ID,
            };
            let mut this = Self {
                gpu,
                output,
                dup: None,
                left: r.left,
                top: r.top,
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
                pointer: Pointer::default(),
                holding_frame: false,
                reopen_failing: false,
                size_differs: false,
                opened_at: std::time::Instant::now(),
                imaged: false,
                changes: Changes::whole(),
                move_rects: Vec::new(),
                dirty_rects: Vec::new(),
            };
            // Not fatal: a monitor that was just added often refuses for a
            // moment (DXGI_ERROR_MODE_CHANGE_IN_PROGRESS); acquire_into tries again.
            if let Err(e) = this.reopen() {
                eprintln!("capture paused: {e:#}");
                this.reopen_failing = true;
            }
            Ok(this)
        }
    }

    pub fn gpu(&self) -> &GpuDevice {
        &self.gpu
    }

    fn reopen(&mut self) -> Result<()> {
        self.dup = None;
        self.holding_frame = false;
        // The sign-in / lock screen / UAC prompt is another desktop. Not moved
        // there: duplicating the old one would only fail; try again later.
        if let Some(name) = follow_input_desktop()? {
            println!("capture: now on the {name} desktop");
        }
        // Ask for plain 8-bit BGRA (SDR desktop). FP16 / R10G10B10A2 can be added
        // here later for 10-bit desktops.
        let formats = [DXGI_FORMAT_B8G8R8A8_UNORM];
        let dup = unsafe { self.output.DuplicateOutput1(&self.gpu.device, 0, &formats) }.context(
            "DuplicateOutput1 (is the process per-monitor DPI aware? is another app capturing?)",
        )?;
        let desc = unsafe { dup.GetDesc() };
        self.width = desc.ModeDesc.Width;
        self.height = desc.ModeDesc.Height;
        if desc.ModeDesc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            bail!("unexpected duplication format {:?}", desc.ModeDesc.Format);
        }
        self.dup = Some(dup);
        self.opened_at = std::time::Instant::now();
        self.imaged = false;
        // The image may differ in any way from the last one we saw.
        self.changes.set_whole();
        Ok(())
    }

    /// The monitor's frames are not the size of the stream's texture: the
    /// caller has to open this monitor anew (its mode changed).
    pub fn size_differs(&self) -> bool {
        self.size_differs
    }

    /// Creates a BGRA texture suitable as `dst` for [`acquire_into`].
    pub fn create_frame_texture(&self) -> Result<ID3D11Texture2D> {
        let desc = texture_desc(
            self.width,
            self.height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            // Render target: `restart` clears it.
            D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET,
        );
        let mut tex = None;
        unsafe {
            self.gpu
                .device
                .CreateTexture2D(&desc, None, Some(&mut tex))?
        };
        tex.context("CreateTexture2D")
    }

    /// A new session starts from what is on screen now: `dst` (made by
    /// [`create_frame_texture`], it still holds the last session's image, e.g.
    /// the desktop before the lock) goes black, and the duplication is made
    /// anew on the current input desktop (an idle one may still give a stale
    /// frame of the old desktop); a new one's first frame is the whole image.
    /// This only drops the old duplication: [`Self::acquire_into`] makes the new one.
    pub fn restart(&mut self, dst: &ID3D11Texture2D) -> Result<()> {
        unsafe {
            let mut rtv = None;
            self.gpu
                .device
                .CreateRenderTargetView(dst, None, Some(&mut rtv))?;
            let rtv = rtv.context("CreateRenderTargetView")?;
            self.gpu
                .context
                .ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
        }
        if let Some(dup) = self.dup.take()
            && std::mem::take(&mut self.holding_frame)
        {
            unsafe {
                let _ = dup.ReleaseFrame();
            }
        }
        Ok(())
    }

    /// Waits up to `timeout_ms` for a new desktop frame and copies it into `dst`.
    ///
    /// On `DXGI_ERROR_ACCESS_LOST` (mode change, UAC prompt, fullscreen app)
    /// duplication is recreated transparently and `Timeout` is returned.
    pub fn acquire_into(&mut self, timeout_ms: u32, dst: &ID3D11Texture2D) -> Result<Acquired> {
        if self.dup.is_none() {
            // Not now (a user process while the secure desktop is up): later.
            if let Err(e) = self.reopen() {
                if !std::mem::replace(&mut self.reopen_failing, true) {
                    eprintln!("capture paused: {e:#}");
                }
                return Ok(pause(timeout_ms));
            }
            if std::mem::take(&mut self.reopen_failing) {
                println!("capture resumed");
            }
        }
        if !self.imaged && self.opened_at.elapsed() >= NO_IMAGE_REOPEN {
            eprintln!(
                "capture: no image {} ms after duplicating the monitor; duplicating anew",
                self.opened_at.elapsed().as_millis()
            );
            if std::mem::take(&mut self.holding_frame)
                && let Some(dup) = &self.dup
            {
                unsafe {
                    let _ = dup.ReleaseFrame();
                }
            }
            self.dup = None;
            return Ok(Acquired::Timeout);
        }
        let dup = self.dup.as_ref().unwrap().clone();
        if self.holding_frame {
            unsafe {
                let _ = dup.ReleaseFrame();
            }
            self.holding_frame = false;
        }

        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Acquired::Timeout),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.dup = None;
                return Ok(Acquired::Timeout);
            }
            // Anything else (DXGI_ERROR_INVALID_CALL after signing in, seen on
            // the service's host): this duplication is done for; a new one,
            // after a pause so a lasting error does not spin.
            Err(e) => {
                // Mostly the desktop switched (Winlogon -> Default once
                // signed in): that ends this duplication; not an error then.
                match follow_input_desktop() {
                    Ok(Some(name)) => println!(
                        "capture: now on the {name} desktop (the old one's duplication ended: {:#x})",
                        e.code().0
                    ),
                    _ => eprintln!("capture: AcquireNextFrame: {e}; duplicating anew"),
                }
                self.dup = None;
                return Ok(pause(timeout_ms));
            }
        }
        self.holding_frame = true;
        self.take_pointer(&dup, &info);

        if info.LastPresentTime == 0 {
            return Ok(Acquired::PointerOnly);
        }
        let tex: ID3D11Texture2D = resource.context("no resource")?.cast()?;
        self.copy_image(&dup, &info, &tex, dst)
    }

    /// What changed in the image since the last call (dirty and move rects of
    /// every frame copied since; the whole picture after a new duplication or
    /// when a frame came without them). Empty: the image is the same.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    /// Adds the frame in hand's dirty rects and move destinations to
    /// `changes`; the whole picture if it cannot tell.
    fn gather_changes(&mut self, dup: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO) {
        if self.changes.whole {
            return;
        }
        let size = info.TotalMetadataBufferSize;
        if size == 0 || self.read_changes(dup, size).is_err() {
            self.changes.set_whole();
        }
    }

    /// Move rects (their destinations hold new content; the source's new
    /// content comes as dirty rects), then dirty rects, into `changes`.
    /// `size`: the frame's metadata, bytes — enough for either list.
    fn read_changes(
        &mut self,
        dup: &IDXGIOutputDuplication,
        size: u32,
    ) -> windows::core::Result<()> {
        let fit = |item: usize| (size as usize).div_ceil(item);
        let moves = &mut self.move_rects;
        moves.resize(
            fit(std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>()),
            Default::default(),
        );
        let mut used = 0u32;
        unsafe {
            dup.GetFrameMoveRects(
                std::mem::size_of_val(moves.as_slice()) as u32,
                moves.as_mut_ptr(),
                &mut used,
            )?
        };
        let n = used as usize / std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
        for m in &moves[..n] {
            self.changes.add(rect(m.DestinationRect));
        }
        let dirty = &mut self.dirty_rects;
        dirty.resize(fit(std::mem::size_of::<RECT>()), RECT::default());
        unsafe {
            dup.GetFrameDirtyRects(
                std::mem::size_of_val(dirty.as_slice()) as u32,
                dirty.as_mut_ptr(),
                &mut used,
            )?
        };
        let n = used as usize / std::mem::size_of::<RECT>();
        for &d in &dirty[..n] {
            self.changes.add(rect(d));
        }
        Ok(())
    }

    /// The pointer as the frame in hand tells it: where, and its shape if new.
    fn take_pointer(&mut self, dup: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO) {
        if info.LastMouseUpdateTime != 0 {
            let p = info.PointerPosition;
            self.pointer.visible = p.Visible.as_bool();
            self.pointer.x = p.Position.x;
            self.pointer.y = p.Position.y;
        }
        if info.PointerShapeBufferSize > 0 {
            let mut data = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut needed = 0u32;
            let mut si = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            let shape = unsafe {
                dup.GetFramePointerShape(
                    data.len() as u32,
                    data.as_mut_ptr() as *mut _,
                    &mut needed,
                    &mut si,
                )
            };
            if shape.is_ok() {
                data.truncate(needed as usize);
                self.pointer.shape = Some(PointerShape {
                    kind: si.Type,
                    width: si.Width,
                    height: si.Height,
                    pitch: si.Pitch,
                    hot_x: si.HotSpot.x as u32,
                    hot_y: si.HotSpot.y as u32,
                    data,
                });
                self.pointer.shape_serial = self.pointer.shape_serial.wrapping_add(1);
            }
        }
    }

    /// The frame in hand's image into `dst`, and the frame released.
    fn copy_image(
        &mut self,
        dup: &IDXGIOutputDuplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        tex: &ID3D11Texture2D,
        dst: &ID3D11Texture2D,
    ) -> Result<Acquired> {
        // A copy between textures of different sizes does nothing (D3D says
        // nothing either): the stream would go on with its old, maybe black,
        // image. The output is at another mode now: the caller opens its
        // screen anew (the host does when its monitors change); not a new image.
        let (got, want) = unsafe {
            let (mut a, mut b) = (
                D3D11_TEXTURE2D_DESC::default(),
                D3D11_TEXTURE2D_DESC::default(),
            );
            tex.GetDesc(&mut a);
            dst.GetDesc(&mut b);
            ((a.Width, a.Height), (b.Width, b.Height))
        };
        if got != want {
            unsafe {
                let _ = dup.ReleaseFrame();
            }
            self.holding_frame = false;
            // This frame's changes are lost: the next image is new throughout.
            self.changes.set_whole();
            if !std::mem::replace(&mut self.size_differs, true) {
                eprintln!(
                    "capture: the monitor is {}x{} now, its stream {}x{}",
                    got.0, got.1, want.0, want.1
                );
            }
            return Ok(Acquired::Timeout);
        }
        self.size_differs = false;
        self.imaged = true;
        self.gather_changes(dup, info);
        unsafe {
            self.gpu.context.CopyResource(dst, tex);
        }
        // Release as early as possible: DWM waits for us while we hold the frame.
        unsafe {
            let _ = dup.ReleaseFrame();
        }
        self.holding_frame = false;
        Ok(Acquired::NewFrame {
            accumulated_frames: info.AccumulatedFrames,
        })
    }
}

/// Reads a BGRA8 texture back to the CPU (tightly packed, for dumps/tests).
pub fn read_bgra(gpu: &GpuDevice, tex: &ID3D11Texture2D) -> Result<Vec<u8>> {
    let desc = desc_of(tex);
    let (row, rows) = (desc.Width as usize * 4, desc.Height as usize);
    let mut out = Vec::with_capacity(row * rows);
    read_rows(
        &gpu.device,
        &gpu.context,
        tex,
        &desc,
        rows,
        row,
        |_, bytes| out.extend_from_slice(bytes),
    )?;
    Ok(out)
}
