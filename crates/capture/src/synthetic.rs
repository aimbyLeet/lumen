use crate::barcode;
use crate::regions::{Changes, Rect};
use crate::win::GpuDevice;
use anyhow::{Context, Result, ensure};
use gpu_convert::{
    compile_src, fullscreen_raster, pixel_shader, set_fullscreen_triangle, texture_desc,
    vertex_shader, viewport,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SHADER_SRC: &str = include_str!("../shaders/synthetic.hlsl");

/// Layout of the HLSL `Params` constant buffer (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    frame: u32,
    width: u32,
    height: u32,
    cell_count: u32,
    bits: [u32; 4],
    window: [u32; 4],
    still_frame: u32,
    pad: [u32; 3],
}

/// What the synthetic source shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scene {
    /// Everything moves, every frame (the default).
    #[default]
    Moving,
    /// Frame `frame` every time, barcode included: nothing changes.
    Still { frame: u32 },
    /// Frame `still_frame` with a moving window of a quarter of the picture
    /// (half the width, half the height, off the block grid) and the barcode
    /// counting frames (a small change, like a blinking caret).
    Mixed { still_frame: u32 },
}

/// Synthetic desktop-like frame source rendered entirely on the GPU.
///
/// Produces the same kind of texture as Desktop Duplication
/// (`B8G8R8A8_UNORM`, sRGB-encoded). Content is a pure function of the frame
/// number and embeds [`barcode::cells`] at (0, 0).
pub struct SyntheticSource {
    gpu: GpuDevice,
    pub width: u32,
    pub height: u32,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    raster: ID3D11RasterizerState,
    params: ID3D11Buffer,
    scene: Scene,
    rendered: bool,
    changes: Changes,
}

impl SyntheticSource {
    pub fn new(gpu: GpuDevice, width: u32, height: u32) -> Result<Self> {
        ensure!(
            width.is_multiple_of(2) && height.is_multiple_of(2) && width >= 704 && height >= 16,
            "synthetic size must be even and at least 704x16, got {width}x{height}"
        );
        let device = &gpu.device;
        let vs_bc = compile_src(SHADER_SRC, s!("synthetic.hlsl"), s!("VS"), s!("vs_5_0"))?;
        let ps_bc = compile_src(SHADER_SRC, s!("synthetic.hlsl"), s!("PS"), s!("ps_5_0"))?;
        let vs = vertex_shader(device, &vs_bc)?;
        let ps = pixel_shader(device, &ps_bc, "ps")?;
        let raster = fullscreen_raster(device)?;
        unsafe {
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Params>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let mut params = None;
            device.CreateBuffer(&bd, None, Some(&mut params))?;
            Ok(Self {
                vs,
                ps,
                raster,
                params: params.context("params")?,
                gpu,
                width,
                height,
                scene: Scene::Moving,
                rendered: false,
                changes: Changes::default(),
            })
        }
    }

    pub fn gpu(&self) -> &GpuDevice {
        &self.gpu
    }

    /// Creates a BGRA texture suitable as `dst` for [`render_into`](Self::render_into).
    pub fn create_frame_texture(&self) -> Result<ID3D11Texture2D> {
        let desc = texture_desc(
            self.width,
            self.height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
        );
        let mut tex = None;
        unsafe {
            self.gpu
                .device
                .CreateTexture2D(&desc, None, Some(&mut tex))?
        };
        tex.context("CreateTexture2D")
    }

    pub fn set_scene(&mut self, scene: Scene) {
        self.scene = scene;
        self.changes.set_whole();
    }

    /// The moving window of [`Scene::Mixed`]: x0, y0, x1, y1.
    pub fn mixed_window(&self) -> [u32; 4] {
        let (w, h) = (self.width, self.height);
        let (x0, y0) = ((w * 45 / 100) | 1, (h * 30 / 100) | 1);
        [x0, y0, x0 + w / 2, y0 + h / 2]
    }

    /// What changed in the picture since the last call (see [`Scene`]).
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    /// Renders frame `frame` into `dst` on the GPU (no CPU copies, does not wait).
    pub fn render_into(&mut self, frame: u32, dst: &ID3D11Texture2D) -> Result<()> {
        let all = [0, 0, self.width, self.height];
        let (window, still_frame, code) = match self.scene {
            Scene::Moving => (all, 0, frame),
            Scene::Still { frame: f } => ([0; 4], f, f),
            Scene::Mixed { still_frame } => (self.mixed_window(), still_frame, frame),
        };
        let params = Params {
            frame,
            width: self.width,
            height: self.height,
            cell_count: barcode::CELL_COUNT as u32,
            bits: barcode::packed_cells(code),
            window,
            still_frame,
            pad: [0; 3],
        };
        if !std::mem::replace(&mut self.rendered, true) {
            self.changes.set_whole();
        }
        let r = |[l, t, rr, b]: [u32; 4]| Rect::new(l as i32, t as i32, rr as i32, b as i32);
        match self.scene {
            Scene::Moving => self.changes.set_whole(),
            Scene::Still { .. } => {}
            Scene::Mixed { .. } => {
                self.changes.add(r(window));
                self.changes
                    .add(r([0, 0, barcode::CELL_COUNT as u32 * 16, 16]));
            }
        }
        let ctx = &self.gpu.context;
        unsafe {
            let mut rtv = None;
            self.gpu
                .device
                .CreateRenderTargetView(dst, None, Some(&mut rtv))?;
            let rtv = rtv.context("RTV")?;
            ctx.UpdateSubresource(&self.params, 0, None, &params as *const _ as *const _, 0, 0);
            set_fullscreen_triangle(ctx);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            ctx.RSSetState(&self.raster);
            ctx.RSSetViewports(Some(&[viewport(self.width, self.height)]));
            ctx.OMSetRenderTargets(Some(&[Some(rtv)]), None);
            ctx.Draw(3, 0);
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}
