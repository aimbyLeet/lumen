//! The composite view of several monitors (mode 1): each monitor's picture is converted to RGB once per new frame,
//! into a texture of its own with mips, and every present draws them scaled
//! where the scene puts them, on black. Into a buffer of the window's size
//! (no DWM stretch); the pointer is drawn on top at its own size.

use anyhow::{Context, Result, anyhow};
use gpu_convert::{
    Yuv420ToRgb10, compile_src, pixel_shader, set_fullscreen_triangle, vertex_shader,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::layout::Layout;
use crate::scene::Scene;

const BLIT_SRC: &str = include_str!("../shaders/blit.hlsl");
const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R10G10B10A2_UNORM;

/// One monitor's picture in RGB.
struct Layer {
    tex: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    size: (u32, u32),
    mips: bool,
}

pub struct Composite {
    device: ID3D11Device,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    /// Mips can be made in R10G10B10A2 on this GPU.
    mipmaps: bool,
    /// Per stream.
    layers: Vec<Option<Layer>>,
}

impl Composite {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let name = s!("blit.hlsl");
        let vs_bc = compile_src(BLIT_SRC, name, s!("VS"), s!("vs_5_0"))?;
        let ps_bc = compile_src(BLIT_SRC, name, s!("PS"), s!("ps_5_0"))?;
        let vs = vertex_shader(device, &vs_bc)?;
        let ps = pixel_shader(device, &ps_bc, "ps")?;
        unsafe {
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&sd, Some(&mut sampler))?;
            let support = device.CheckFormatSupport(FORMAT).unwrap_or(0);
            let mipmaps = support & D3D11_FORMAT_SUPPORT_MIP_AUTOGEN.0 as u32 != 0;
            Ok(Composite {
                device: device.clone(),
                vs,
                ps,
                sampler: sampler.context("sampler")?,
                mipmaps,
                layers: Vec::new(),
            })
        }
    }

    fn layer(&mut self, stream: u8, size: (u32, u32)) -> Result<&Layer> {
        let i = usize::from(stream);
        if self.layers.len() <= i {
            self.layers.resize_with(i + 1, || None);
        }
        if self.layers[i].as_ref().is_none_or(|l| l.size != size) {
            self.layers[i] = Some(self.make_layer(size)?);
        }
        Ok(self.layers[i].as_ref().unwrap())
    }

    fn make_layer(&self, size: (u32, u32)) -> Result<Layer> {
        let mips = self.mipmaps;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: if mips { 0 } else { 1 },
            ArraySize: 1,
            Format: FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: if mips {
                D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32
            } else {
                0
            },
        };
        unsafe {
            let mut tex = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex: ID3D11Texture2D = tex.context("layer texture")?;
            // The view of the top level: the converter writes 1:1 there.
            let mut rtv = None;
            self.device
                .CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
            let mut srv = None;
            self.device
                .CreateShaderResourceView(&tex, None, Some(&mut srv))?;
            Ok(Layer {
                tex,
                rtv: rtv.context("layer RTV")?,
                srv: srv.context("layer SRV")?,
                size,
                mips,
            })
        }
    }

    /// A new picture of `stream`: converted into its layer (and its mips).
    pub fn update(
        &mut self,
        ctx: &ID3D11DeviceContext,
        shader: &Yuv420ToRgb10,
        luma: &ID3D11ShaderResourceView,
        chroma: &ID3D11ShaderResourceView,
        stream: u8,
        size: (u32, u32),
    ) -> Result<()> {
        let layer = self.layer(stream, size)?;
        shader.draw(ctx, luma, chroma, &layer.rtv, size.0, size.1);
        if layer.mips {
            unsafe { ctx.GenerateMips(&layer.srv) };
        }
        let _ = &layer.tex;
        Ok(())
    }

    /// Forgets every picture (the host's monitors changed).
    pub fn clear(&mut self) {
        self.layers.clear();
    }

    /// The monitors of `scene` scaled into `target` (the window's client
    /// area, `fit`: the canvas in it), on black.
    pub fn draw(
        &self,
        ctx: &ID3D11DeviceContext,
        target: &ID3D11RenderTargetView,
        scene: &Scene,
        fit: &Layout,
    ) -> Result<()> {
        unsafe {
            ctx.ClearRenderTargetView(target, &[0.0, 0.0, 0.0, 1.0]);
            set_fullscreen_triangle(ctx);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
        }
        for m in &scene.monitors {
            let Some(layer) = self
                .layers
                .get(usize::from(m.stream))
                .and_then(Option::as_ref)
            else {
                continue;
            };
            let (x0, y0) = fit.picture_to_client_f(m.canvas.x as f32, m.canvas.y as f32);
            let (x1, y1) = fit.picture_to_client_f(
                (m.canvas.x + m.canvas.w) as f32,
                (m.canvas.y + m.canvas.h) as f32,
            );
            unsafe {
                ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                    TopLeftX: x0,
                    TopLeftY: y0,
                    Width: x1 - x0,
                    Height: y1 - y0,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]));
                ctx.PSSetShaderResources(0, Some(&[Some(layer.srv.clone())]));
                ctx.Draw(3, 0);
            }
        }
        unsafe {
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        if scene.monitors.is_empty() {
            return Err(anyhow!("no monitors to draw"));
        }
        Ok(())
    }
}
