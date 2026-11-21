//! Client side: decoded 4:2:0 (luma R16 + chroma R16G16 textures) → 10-bit R'G'B'.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::d3d::{
    desc_of, fullscreen_raster, pixel_shader, read_rows, set_fullscreen_triangle, texture_desc,
    vertex_shader, viewport,
};
use crate::win::compile_src;

const SHADER_SRC: &str = include_str!("../shaders/yuv420_to_rgb10.hlsl");

/// Y'CbCr 4:2:0 → R'G'B' (BT.709 full range, left siting) into a `R10G10B10A2` target, 1:1.
pub struct Yuv420ToRgb10 {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    raster: ID3D11RasterizerState,
}

impl Yuv420ToRgb10 {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let name = s!("yuv420_to_rgb10.hlsl");
        let vs_bc = compile_src(SHADER_SRC, name, s!("VS"), s!("vs_5_0"))?;
        let ps_bc = compile_src(SHADER_SRC, name, s!("PS"), s!("ps_5_0"))?;
        Ok(Self {
            vs: vertex_shader(device, &vs_bc)?,
            ps: pixel_shader(device, &ps_bc, "ps")?,
            raster: fullscreen_raster(device)?,
        })
    }

    /// Records the conversion of a `width` × `height` picture into `target`
    /// at its top-left corner. Does not wait for the GPU.
    pub fn draw(
        &self,
        ctx: &ID3D11DeviceContext,
        luma: &ID3D11ShaderResourceView,
        chroma: &ID3D11ShaderResourceView,
        target: &ID3D11RenderTargetView,
        width: u32,
        height: u32,
    ) {
        set_fullscreen_triangle(ctx);
        unsafe {
            ctx.VSSetShader(&self.vs, None);
            ctx.RSSetState(&self.raster);
            ctx.RSSetViewports(Some(&[viewport(width, height)]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(luma.clone()), Some(chroma.clone())]));
            ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            ctx.Draw(3, 0);
            ctx.OMSetRenderTargets(None, None);
            ctx.PSSetShaderResources(0, Some(&[None, None]));
        }
    }
}

/// Shader resource view with the texture's own format.
pub fn texture_srv(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView> {
    let mut srv = None;
    unsafe { device.CreateShaderResourceView(tex, None, Some(&mut srv))? };
    srv.context("SRV")
}

/// Offscreen `R10G10B10A2_UNORM` render target (for verification without a window).
pub fn create_rgb10_target(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11RenderTargetView)> {
    let td = texture_desc(
        width,
        height,
        DXGI_FORMAT_R10G10B10A2_UNORM,
        D3D11_BIND_RENDER_TARGET,
    );
    unsafe {
        let mut tex = None;
        device.CreateTexture2D(&td, None, Some(&mut tex))?;
        let tex: ID3D11Texture2D = tex.context("RGB10 texture")?;
        let mut rtv = None;
        device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
        Ok((tex, rtv.context("RTV")?))
    }
}

/// Reads a `R10G10B10A2_UNORM` texture back as `[r, g, b]` 10-bit codes (verification only).
pub fn read_rgb10(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
) -> Result<Vec<[u16; 3]>> {
    let desc = desc_of(tex);
    anyhow::ensure!(
        desc.Format == DXGI_FORMAT_R10G10B10A2_UNORM,
        "not a R10G10B10A2 texture"
    );
    let (w, h) = (desc.Width as usize, desc.Height as usize);
    let mut out = Vec::with_capacity(w * h);
    read_rows(device, ctx, tex, &desc, h, w * 4, |_, bytes| {
        out.extend(bytes.as_chunks::<4>().0.iter().map(|b| {
            let v = u32::from_le_bytes(*b);
            [
                (v & 0x3FF) as u16,
                ((v >> 10) & 0x3FF) as u16,
                ((v >> 20) & 0x3FF) as u16,
            ]
        }));
    })?;
    Ok(out)
}
