use std::cell::{Cell, RefCell};

use anyhow::{Context, Result, anyhow};
use gpu_convert::texture_desc;
use windows::Win32::Foundation::{HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;

use crate::layout::Layout;

const COLOR_SPACE: DXGI_COLOR_SPACE_TYPE = DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R10G10B10A2_UNORM;
const FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// Flip-model swapchain, R10G10B10A2, sized and filled by a [`Layout`]: the
/// buffer has the window's aspect and is stretched over it (exactly 1:1 when
/// it is the client area's size). Renderers draw the picture 1:1 into
/// [`render_target`](Self::render_target): the buffer itself when it is just
/// the picture, else a picture-sized texture copied into the black buffer at
/// the layout's origin on [`present`](Self::present).
pub struct Presenter {
    device: ID3D11Device,
    swapchain: IDXGISwapChain3,
    waitable: HANDLE,
    layout: Cell<Layout>,
    /// The picture when it is not the whole buffer.
    picture: RefCell<Option<(ID3D11Texture2D, ID3D11RenderTargetView)>>,
}

impl Presenter {
    pub fn new(device: &ID3D11Device, hwnd: HWND, layout: Layout) -> Result<Presenter> {
        unsafe {
            let dxgi_dev: IDXGIDevice = device.cast()?;
            let adapter = dxgi_dev.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: layout.buffer.0,
                Height: layout.buffer.1,
                Format: FORMAT,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                // A buffer of the client area's size is shown as it is.
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: FLAGS.0 as u32,
            };
            let sc1 = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            let swapchain: IDXGISwapChain3 = sc1.cast()?;

            let support = swapchain.CheckColorSpaceSupport(COLOR_SPACE)?;
            if support & DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT.0 as u32 == 0 {
                return Err(anyhow!(
                    "swapchain colour space RGB_FULL_G22_NONE_P709 not supported for present"
                ));
            }
            swapchain.SetColorSpace1(COLOR_SPACE)?;

            let sc2: IDXGISwapChain2 = swapchain.cast()?;
            sc2.SetMaximumFrameLatency(1)?;
            let waitable = sc2.GetFrameLatencyWaitableObject();
            if waitable.is_invalid() {
                return Err(anyhow!("no frame latency waitable object"));
            }
            let p = Presenter {
                device: device.clone(),
                swapchain,
                waitable,
                layout: Cell::new(layout),
                picture: RefCell::new(None),
            };
            p.make_picture_target()?;
            Ok(p)
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout.get()
    }

    /// Takes a new layout (the window was resized, maximised, made borderless).
    /// No render target may be alive.
    pub fn set_layout(&self, layout: Layout) -> Result<()> {
        let old = self.layout.replace(layout);
        if old.buffer != layout.buffer {
            self.picture.borrow_mut().take();
            unsafe {
                self.swapchain
                    .ResizeBuffers(0, layout.buffer.0, layout.buffer.1, FORMAT, FLAGS)
                    .context("ResizeBuffers")?;
                self.swapchain.SetColorSpace1(COLOR_SPACE)?;
            }
        }
        if old.picture != layout.picture || old.buffer != layout.buffer {
            self.make_picture_target()?;
        }
        Ok(())
    }

    fn make_picture_target(&self) -> Result<()> {
        let l = self.layout.get();
        let mut slot = self.picture.borrow_mut();
        *slot = None;
        if l.is_picture_only() {
            return Ok(());
        }
        let desc = texture_desc(l.picture.0, l.picture.1, FORMAT, D3D11_BIND_RENDER_TARGET);
        unsafe {
            let mut tex = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex = tex.context("picture texture")?;
            let mut rtv = None;
            self.device
                .CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
            *slot = Some((tex, rtv.context("picture RTV")?));
        }
        Ok(())
    }

    /// Human-readable swapchain description (size, format, how it is shown).
    pub fn describe(&self) -> Result<String> {
        let d = unsafe { self.swapchain.GetDesc1()? };
        let l = self.layout.get();
        let shown = if l.is_one_to_one() {
            "1:1".to_string()
        } else {
            format!(
                "scaled to {}x{} (not 1:1: judge colour in a window of the picture's size)",
                l.client.0, l.client.1
            )
        };
        let format = if d.Format == FORMAT {
            "R10G10B10A2_UNORM".to_string()
        } else {
            format!("{:?}", d.Format)
        };
        Ok(format!(
            "{}x{} format={format} buffers={} colour_space=RGB_FULL_G22_NONE_P709, picture {}x{} at ({}, {}), {shown}",
            d.Width, d.Height, d.BufferCount, l.picture.0, l.picture.1, l.origin.0, l.origin.1
        ))
    }

    /// The frame-latency waitable: signalled when the swapchain can take a frame.
    pub fn waitable(&self) -> HANDLE {
        self.waitable
    }

    /// Waits for the swapchain to accept a new frame (1 s timeout).
    pub fn wait_for_frame(&self) {
        unsafe {
            let r = WaitForSingleObject(self.waitable, 1000);
            if r != WAIT_OBJECT_0 {
                eprintln!("warning: frame latency wait returned {r:?}");
            }
        }
    }

    fn back_buffer_rtv(&self) -> Result<ID3D11RenderTargetView> {
        unsafe {
            let buf: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            let mut rtv = None;
            self.device
                .CreateRenderTargetView(&buf, None, Some(&mut rtv))?;
            rtv.ok_or_else(|| anyhow!("no RTV"))
        }
    }

    /// Where to draw the picture, 1:1 at its top-left corner.
    pub fn render_target(&self) -> Result<ID3D11RenderTargetView> {
        match &*self.picture.borrow() {
            Some((_, rtv)) => Ok(rtv.clone()),
            None => self.back_buffer_rtv(),
        }
    }

    /// Shows what was drawn: puts the picture into the black buffer first
    /// when it is not the buffer itself.
    pub fn present(&self, ctx: &ID3D11DeviceContext, sync_interval: u32) -> Result<()> {
        if let Some((tex, _)) = &*self.picture.borrow() {
            let l = self.layout.get();
            unsafe {
                let buf: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
                ctx.ClearRenderTargetView(&self.back_buffer_rtv()?, &[0.0, 0.0, 0.0, 1.0]);
                ctx.CopySubresourceRegion(&buf, 0, l.origin.0, l.origin.1, 0, tex, 0, None);
            }
        }
        unsafe {
            self.swapchain
                .Present(sync_interval, DXGI_PRESENT(0))
                .ok()?
        };
        Ok(())
    }

    /// Copies the picture as drawn (call before `present`) and returns its
    /// R, G, B 10-bit codes per pixel, row-major. Verification only.
    pub fn read_back(&self, ctx: &ID3D11DeviceContext) -> Result<Vec<[u16; 3]>> {
        let (width, height) = self.layout.get().picture;
        unsafe {
            let buf: ID3D11Texture2D = match &*self.picture.borrow() {
                Some((tex, _)) => tex.clone(),
                None => self.swapchain.GetBuffer(0)?,
            };
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            buf.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| anyhow!("no staging texture"))?;
            ctx.CopyResource(&staging, &buf);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let mut out = Vec::with_capacity((width * height) as usize);
            for y in 0..height as usize {
                let row = std::slice::from_raw_parts(
                    (map.pData as *const u8).add(y * map.RowPitch as usize) as *const u32,
                    width as usize,
                );
                out.extend(row.iter().map(|&p| {
                    [
                        (p & 0x3FF) as u16,
                        ((p >> 10) & 0x3FF) as u16,
                        ((p >> 20) & 0x3FF) as u16,
                    ]
                }));
            }
            ctx.Unmap(&staging, 0);
            Ok(out)
        }
    }
}
