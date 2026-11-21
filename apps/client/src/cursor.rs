//! Draws the host's mouse pointer over the video (client side, no video latency).

use anyhow::{Context, Result};
use gpu_convert::{compile_src, pixel_shader, set_fullscreen_triangle, vertex_shader};
use protocol::{CursorKind, CursorShape};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SHADER_SRC: &str = include_str!("../shaders/cursor.hlsl");

/// A shape converted for drawing: straight-alpha BGRA plus a mask of pixels
/// that invert what is under them.
#[derive(Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
    pub invert: Vec<u8>,
}

/// Converts the three Desktop Duplication shape kinds. Monochrome: AND/XOR
/// masks (0,0) black, (0,1) white, (1,0) transparent, (1,1) invert. Masked
/// colour: alpha 0 replaces the screen; alpha 0xFF XORs — black XOR is a no-op,
/// any other XOR colour is drawn as inversion.
pub fn to_bitmap(s: &CursorShape) -> Bitmap {
    let w = usize::from(s.width);
    let pitch = s.pitch as usize;
    let h = match s.kind {
        CursorKind::Monochrome => usize::from(s.height) / 2,
        _ => usize::from(s.height),
    };
    let mut bgra = vec![0u8; w * h * 4];
    let mut invert = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let o = y * w + x;
            let (px, inv): ([u8; 4], bool) = match s.kind {
                CursorKind::Color => {
                    let i = y * pitch + x * 4;
                    (s.data[i..i + 4].try_into().unwrap(), false)
                }
                CursorKind::MaskedColor => {
                    let i = y * pitch + x * 4;
                    let (b, g, r, a) = (s.data[i], s.data[i + 1], s.data[i + 2], s.data[i + 3]);
                    match (a, (b, g, r)) {
                        (0, _) => ([b, g, r, 255], false),
                        (_, (0, 0, 0)) => ([0; 4], false),
                        _ => ([0; 4], true),
                    }
                }
                CursorKind::Monochrome => {
                    let bit = |row: usize| (s.data[row * pitch + x / 8] >> (7 - x % 8)) & 1;
                    match (bit(y), bit(y + h)) {
                        (0, 0) => ([0, 0, 0, 255], false),
                        (0, _) => ([255, 255, 255, 255], false),
                        (_, 0) => ([0; 4], false),
                        _ => ([0; 4], true),
                    }
                }
            };
            bgra[o * 4..o * 4 + 4].copy_from_slice(&px);
            invert[o] = if inv { 255 } else { 0 };
        }
    }
    Bitmap {
        width: w,
        height: h,
        bgra,
        invert,
    }
}

struct Loaded {
    id: u32,
    width: u32,
    height: u32,
    hot_x: i32,
    hot_y: i32,
    image: ID3D11ShaderResourceView,
    invert: Option<ID3D11ShaderResourceView>,
}

pub struct CursorOverlay {
    device: ID3D11Device,
    vs: ID3D11VertexShader,
    ps_image: ID3D11PixelShader,
    ps_invert: ID3D11PixelShader,
    blend_alpha: ID3D11BlendState,
    blend_invert: ID3D11BlendState,
    params: ID3D11Buffer,
    shape: Option<Loaded>,
}

fn blend(device: &ID3D11Device, src: D3D11_BLEND, dst: D3D11_BLEND) -> Result<ID3D11BlendState> {
    let mut d = D3D11_BLEND_DESC::default();
    d.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: src,
        DestBlend: dst,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_ZERO,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut state = None;
    unsafe { device.CreateBlendState(&d, Some(&mut state))? };
    state.context("blend state")
}

fn texture(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    bytes: &[u8],
    pitch: u32,
) -> Result<ID3D11ShaderResourceView> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr() as *const _,
        SysMemPitch: pitch,
        SysMemSlicePitch: 0,
    };
    unsafe {
        let mut tex = None;
        device.CreateTexture2D(&desc, Some(&init), Some(&mut tex))?;
        let tex: ID3D11Texture2D = tex.context("cursor texture")?;
        let mut srv = None;
        device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        srv.context("cursor SRV")
    }
}

impl CursorOverlay {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let name = s!("cursor.hlsl");
        let vs_bc = compile_src(SHADER_SRC, name, s!("VS"), s!("vs_5_0"))?;
        let img_bc = compile_src(SHADER_SRC, name, s!("PS_Image"), s!("ps_5_0"))?;
        let inv_bc = compile_src(SHADER_SRC, name, s!("PS_Invert"), s!("ps_5_0"))?;
        let vs = vertex_shader(device, &vs_bc)?;
        let ps_image = pixel_shader(device, &img_bc, "ps")?;
        let ps_invert = pixel_shader(device, &inv_bc, "ps")?;
        unsafe {
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut params = None;
            device.CreateBuffer(&bd, None, Some(&mut params))?;
            Ok(CursorOverlay {
                device: device.clone(),
                vs,
                ps_image,
                ps_invert,
                blend_alpha: blend(device, D3D11_BLEND_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA)?,
                blend_invert: blend(device, D3D11_BLEND_INV_DEST_COLOR, D3D11_BLEND_ZERO)?,
                params: params.context("constant buffer")?,
                shape: None,
            })
        }
    }

    pub fn shape_id(&self) -> Option<u32> {
        self.shape.as_ref().map(|s| s.id)
    }

    pub fn set_shape(&mut self, s: &CursorShape) -> Result<()> {
        let b = to_bitmap(s);
        let (w, h) = (b.width as u32, b.height as u32);
        let image = texture(
            &self.device,
            w,
            h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &b.bgra,
            w * 4,
        )?;
        let invert = if b.invert.iter().any(|&v| v != 0) {
            Some(texture(
                &self.device,
                w,
                h,
                DXGI_FORMAT_R8_UNORM,
                &b.invert,
                w,
            )?)
        } else {
            None
        };
        self.shape = Some(Loaded {
            id: s.id,
            width: w,
            height: h,
            hot_x: i32::from(s.hot_x),
            hot_y: i32::from(s.hot_y),
            image,
            invert,
        });
        Ok(())
    }

    /// Draws the pointer with its hotspot at (`x`, `y`) in render-target pixels.
    pub fn draw(&self, ctx: &ID3D11DeviceContext, rtv: &ID3D11RenderTargetView, x: i32, y: i32) {
        let Some(s) = &self.shape else { return };
        let (ox, oy) = (x - s.hot_x, y - s.hot_y);
        let params = [ox, oy, 0, 0];
        unsafe {
            ctx.UpdateSubresource(&self.params, 0, None, params.as_ptr() as *const _, 0, 0);
            set_fullscreen_triangle(ctx);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: ox as f32,
                TopLeftY: oy as f32,
                Width: s.width as f32,
                Height: s.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.PSSetShaderResources(0, Some(&[Some(s.image.clone()), s.invert.clone()]));
            ctx.OMSetBlendState(&self.blend_alpha, None, 0xFFFF_FFFF);
            ctx.PSSetShader(&self.ps_image, None);
            ctx.Draw(3, 0);
            if s.invert.is_some() {
                ctx.OMSetBlendState(&self.blend_invert, None, 0xFFFF_FFFF);
                ctx.PSSetShader(&self.ps_invert, None);
                ctx.Draw(3, 0);
            }
            ctx.OMSetBlendState(None, None, 0xFFFF_FFFF);
            ctx.PSSetShaderResources(0, Some(&[None, None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(kind: CursorKind, w: u16, h: u16, pitch: u32, data: Vec<u8>) -> CursorShape {
        CursorShape {
            id: 1,
            kind,
            width: w,
            height: h,
            pitch,
            hot_x: 0,
            hot_y: 0,
            data,
        }
    }

    #[test]
    fn monochrome_masks() {
        // 4×1 visible: AND row then XOR row, MSB first.
        // pixels: (0,0) black, (0,1) white, (1,0) transparent, (1,1) invert
        let s = shape(
            CursorKind::Monochrome,
            4,
            2,
            1,
            vec![0b0011_0000, 0b0101_0000],
        );
        let b = to_bitmap(&s);
        assert_eq!((b.width, b.height), (4, 1));
        assert_eq!(
            b.bgra,
            [[0, 0, 0, 255], [255, 255, 255, 255], [0; 4], [0; 4]].concat()
        );
        assert_eq!(b.invert, vec![0, 0, 0, 255]);
    }

    #[test]
    fn masked_colour() {
        let px = [
            [10, 20, 30, 0],       // replace
            [0, 0, 0, 0xFF],       // XOR black: nothing
            [255, 255, 255, 0xFF], // XOR white: invert
        ];
        let s = shape(CursorKind::MaskedColor, 3, 1, 12, px.concat());
        let b = to_bitmap(&s);
        assert_eq!(b.bgra, [[10, 20, 30, 255], [0; 4], [0; 4]].concat());
        assert_eq!(b.invert, vec![0, 0, 255]);
    }

    #[test]
    fn colour_with_padded_pitch() {
        let mut data = vec![0u8; 2 * 16];
        data[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        data[16..24].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        let b = to_bitmap(&shape(CursorKind::Color, 2, 2, 16, data));
        assert_eq!(b.bgra, (1..=16).collect::<Vec<u8>>());
        assert!(b.invert.iter().all(|&v| v == 0));
    }
}
