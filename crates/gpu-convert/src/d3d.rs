//! D3D11 plumbing shared by the GPU passes: fullscreen-triangle draws, plain
//! 2D textures and CPU readback (verification only).

use anyhow::{Context, Result, ensure};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Vertex shader from compiled bytecode.
pub fn vertex_shader(device: &ID3D11Device, bytecode: &[u8]) -> Result<ID3D11VertexShader> {
    let mut vs = None;
    unsafe { device.CreateVertexShader(bytecode, None, Some(&mut vs))? };
    vs.context("vs")
}

/// Pixel shader from compiled bytecode; `what` names it in the error.
pub fn pixel_shader(
    device: &ID3D11Device,
    bytecode: &[u8],
    what: &'static str,
) -> Result<ID3D11PixelShader> {
    let mut ps = None;
    unsafe { device.CreatePixelShader(bytecode, None, Some(&mut ps))? };
    ps.context(what)
}

/// Rasterizer state for a fullscreen triangle: solid, no culling.
pub fn fullscreen_raster(device: &ID3D11Device) -> Result<ID3D11RasterizerState> {
    let rd = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        DepthClipEnable: true.into(),
        ..Default::default()
    };
    let mut raster = None;
    unsafe { device.CreateRasterizerState(&rd, Some(&mut raster))? };
    raster.context("raster")
}

/// No vertex buffers: the vertex shader makes the triangle from `SV_VertexID`.
#[inline]
pub fn set_fullscreen_triangle(ctx: &ID3D11DeviceContext) {
    unsafe {
        ctx.IASetInputLayout(None);
        ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    }
}

/// A `width` × `height` viewport at the top-left corner, depth 0…1.
#[inline]
pub fn viewport(width: u32, height: u32) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    }
}

/// A plain GPU-only 2D texture: one mip, one slice, no multisampling.
pub fn texture_desc(
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    bind: D3D11_BIND_FLAG,
) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

/// The description `tex` was created with.
pub fn desc_of(tex: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { tex.GetDesc(&mut desc) };
    desc
}

/// Rows of a mapped texture of `desc`: planar 4:2:0 formats keep the chroma
/// plane (half the rows) below the luma plane in the same mapping.
fn mapped_rows(desc: &D3D11_TEXTURE2D_DESC) -> usize {
    let h = desc.Height as usize;
    match desc.Format {
        DXGI_FORMAT_NV12 | DXGI_FORMAT_P010 | DXGI_FORMAT_P016 => h + h / 2,
        _ => h,
    }
}

/// Copies `tex` (described by `desc`, see [`desc_of`]) to a CPU-readable
/// staging texture and calls `row(index, bytes)` with the first
/// `bytes_per_row` bytes of each of its first `rows` rows, top to bottom.
/// Verification only: it waits for the GPU.
pub fn read_rows(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    desc: &D3D11_TEXTURE2D_DESC,
    rows: usize,
    bytes_per_row: usize,
    mut row: impl FnMut(usize, &[u8]),
) -> Result<()> {
    let mut desc = *desc;
    ensure!(
        rows <= mapped_rows(&desc),
        "{rows} rows asked of a texture with {}",
        mapped_rows(&desc)
    );
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    unsafe {
        let mut staging = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.context("staging")?;
        ctx.CopyResource(&staging, tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        if bytes_per_row > pitch {
            ctx.Unmap(&staging, 0);
            anyhow::bail!("{bytes_per_row} bytes asked of rows of {pitch}");
        }
        for r in 0..rows {
            // In bounds: `rows` rows of `pitch` bytes are mapped (checked above).
            let p = (mapped.pData as *const u8).add(r * pitch);
            row(r, std::slice::from_raw_parts(p, bytes_per_row));
        }
        ctx.Unmap(&staging, 0);
    }
    Ok(())
}
