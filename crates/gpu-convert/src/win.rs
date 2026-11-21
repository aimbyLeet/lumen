use anyhow::{Context, Result, anyhow};
use color::subsample::Yuv420p10;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, PCSTR, s};

use crate::d3d::{
    desc_of, fullscreen_raster, pixel_shader, read_rows, set_fullscreen_triangle, texture_desc,
    vertex_shader, viewport,
};

const SHADER_SRC: &str = include_str!("../shaders/bgra_to_p010.hlsl");

fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    compile_src(SHADER_SRC, s!("bgra_to_p010.hlsl"), entry, target)
}

/// Compiles HLSL `src` (shared by the host, client and test shaders).
pub fn compile_src(src: &str, name: PCSTR, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let res = unsafe {
        D3DCompile(
            src.as_ptr() as *const _,
            src.len(),
            name,
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(e) = res {
        let msg = errors
            .map(|b| unsafe {
                let p = std::slice::from_raw_parts(
                    b.GetBufferPointer() as *const u8,
                    b.GetBufferSize(),
                );
                String::from_utf8_lossy(p).into_owned()
            })
            .unwrap_or_default();
        return Err(anyhow!("shader compile failed: {e}\n{msg}"));
    }
    let blob = blob.context("no blob")?;
    Ok(unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
    .to_vec())
}

/// BGRA8 → P010 (BT.709 full range, 4:2:0, left siting) on the GPU.
pub struct BgraToP010 {
    device: ID3D11Device,
    vs: ID3D11VertexShader,
    ps_luma: ID3D11PixelShader,
    ps_chroma: ID3D11PixelShader,
    raster: ID3D11RasterizerState,
    /// Output texture, registered with NVENC.
    pub output: ID3D11Texture2D,
    rtv_luma: ID3D11RenderTargetView,
    rtv_chroma: ID3D11RenderTargetView,
    pub width: u32,
    pub height: u32,
}

impl BgraToP010 {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        anyhow::ensure!(
            width.is_multiple_of(2) && height.is_multiple_of(2),
            "P010 needs even dimensions, got {width}x{height}"
        );
        let vs_bc = compile(s!("VS"), s!("vs_5_0"))?;
        let luma_bc = compile(s!("PS_Luma"), s!("ps_5_0"))?;
        let chroma_bc = compile(s!("PS_Chroma"), s!("ps_5_0"))?;
        let vs = vertex_shader(device, &vs_bc)?;
        let ps_luma = pixel_shader(device, &luma_bc, "ps luma")?;
        let ps_chroma = pixel_shader(device, &chroma_bc, "ps chroma")?;
        let raster = fullscreen_raster(device)?;
        let td = texture_desc(width, height, DXGI_FORMAT_P010, D3D11_BIND_RENDER_TARGET);
        unsafe {
            let mut output = None;
            device
                .CreateTexture2D(&td, None, Some(&mut output))
                .context("create P010 texture")?;
            let output: ID3D11Texture2D = output.context("P010 texture")?;

            // For planar video formats the view format selects the plane.
            let rtv = |fmt: DXGI_FORMAT| -> Result<ID3D11RenderTargetView> {
                let desc = D3D11_RENDER_TARGET_VIEW_DESC {
                    Format: fmt,
                    ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
                    },
                };
                let mut v = None;
                device.CreateRenderTargetView(&output, Some(&desc), Some(&mut v))?;
                v.context("RTV")
            };
            let rtv_luma = rtv(DXGI_FORMAT_R16_UNORM)?;
            let rtv_chroma = rtv(DXGI_FORMAT_R16G16_UNORM)?;

            Ok(Self {
                device: device.clone(),
                vs,
                ps_luma,
                ps_chroma,
                raster,
                output,
                rtv_luma,
                rtv_chroma,
                width,
                height,
            })
        }
    }

    /// Creates a shader resource view for a BGRA source texture (create once, reuse).
    pub fn source_view(&self, src: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(src, None, Some(&mut srv))?
        };
        srv.context("SRV")
    }

    /// Records the conversion on `ctx`. Does not wait for the GPU.
    pub fn convert(&self, ctx: &ID3D11DeviceContext, src: &ID3D11ShaderResourceView) {
        set_fullscreen_triangle(ctx);
        unsafe {
            ctx.VSSetShader(&self.vs, None);
            ctx.RSSetState(&self.raster);
            ctx.PSSetShaderResources(0, Some(&[Some(src.clone())]));

            ctx.OMSetRenderTargets(Some(&[Some(self.rtv_luma.clone())]), None);
            ctx.RSSetViewports(Some(&[viewport(self.width, self.height)]));
            ctx.PSSetShader(&self.ps_luma, None);
            ctx.Draw(3, 0);

            ctx.OMSetRenderTargets(Some(&[Some(self.rtv_chroma.clone())]), None);
            ctx.RSSetViewports(Some(&[viewport(self.width / 2, self.height / 2)]));
            ctx.PSSetShader(&self.ps_chroma, None);
            ctx.Draw(3, 0);

            // Unbind so the texture can be used by NVENC / copied.
            ctx.OMSetRenderTargets(None, None);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }
}

/// Blocks until all GPU work submitted on `ctx` has finished (for timing).
pub fn wait_gpu(device: &ID3D11Device, ctx: &ID3D11DeviceContext) -> Result<()> {
    unsafe {
        let desc = D3D11_QUERY_DESC {
            Query: D3D11_QUERY_EVENT,
            MiscFlags: 0,
        };
        let mut q = None;
        device.CreateQuery(&desc, Some(&mut q))?;
        let q = q.context("query")?;
        ctx.End(&q);
        let mut done: i32 = 0;
        loop {
            let hr = ctx.GetData(
                &q,
                Some(&mut done as *mut i32 as *mut _),
                std::mem::size_of::<i32>() as u32,
                0,
            );
            if hr.is_ok() && done != 0 {
                return Ok(());
            }
            std::hint::spin_loop();
        }
    }
}

/// Reads a P010 texture back to the CPU as 10-bit codes (for verification).
pub fn read_p010(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
) -> Result<Yuv420p10> {
    let desc = desc_of(tex);
    anyhow::ensure!(desc.Format == DXGI_FORMAT_P010, "not a P010 texture");
    let (w, h) = (desc.Width as usize, desc.Height as usize);
    let (cw, ch) = (w / 2, h / 2);
    let word = |b: [u8; 2]| -> u16 { u16::from_le_bytes(b) >> 6 };
    let mut y = Vec::with_capacity(w * h);
    let mut u = Vec::with_capacity(cw * ch);
    let mut v = Vec::with_capacity(cw * ch);
    // Chroma plane follows the luma plane: interleaved U,V words.
    read_rows(device, ctx, tex, &desc, h + ch, w * 2, |row, bytes| {
        if row < h {
            y.extend(bytes.as_chunks::<2>().0.iter().map(|&b| word(b)));
        } else {
            for &[u0, u1, v0, v1] in bytes.as_chunks::<4>().0 {
                u.push(word([u0, u1]));
                v.push(word([v0, v1]));
            }
        }
    })?;
    Ok(Yuv420p10 {
        width: w,
        height: h,
        y,
        u,
        v,
    })
}

/// Raw COM pointer, e.g. for handing the device / texture to NVENC.
pub fn raw<T: Interface>(i: &T) -> *mut core::ffi::c_void {
    i.as_raw()
}
