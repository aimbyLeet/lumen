//! Render steps plugged into the present loop: test pattern, black fill, video.
//! The renderers of checks are in `checks`; the live video in `network` (its
//! views in `network::views`, its pointer in `network::pointer`).

mod checks;
mod network;

pub use checks::{Clear, InputScript, TestPattern, VideoFile};
pub use network::{Decode, Mode, NetworkVideo, OpenSession, Reconnect};

use std::collections::HashMap;

use anyhow::Result;
use decoder::Frame;

use gpu_convert::{Yuv420ToRgb10, texture_srv};
use windows::Win32::Graphics::Direct3D11::*;
use windows::core::Interface;

/// What the back buffer must contain after `draw`, for readback verification.
pub struct Expected {
    pub pixels: Vec<[u16; 3]>,
    /// Allowed per-channel difference in 10-bit codes.
    pub tolerance: u16,
}

pub trait Renderer {
    /// Draws frame `frame` into `rtv`; returns false when there is nothing more to show.
    fn draw(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        frame: u64,
    ) -> Result<bool>;

    /// Reference image for the frame just drawn (CPU, slow; verification only).
    fn expected(&mut self, ctx: &ID3D11DeviceContext) -> Result<Option<Expected>>;
}

/// Shader resource views per decoder output texture (the decoder reuses a small ring).
struct ViewCache {
    device: ID3D11Device,
    views: HashMap<usize, ID3D11ShaderResourceView>,
}

impl ViewCache {
    fn new(device: &ID3D11Device) -> Self {
        ViewCache {
            device: device.clone(),
            views: HashMap::new(),
        }
    }

    fn get(&mut self, tex: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
        let key = tex.as_raw() as usize;
        if let Some(v) = self.views.get(&key) {
            return Ok(v.clone());
        }
        let v = texture_srv(&self.device, tex)?;
        self.views.insert(key, v.clone());
        Ok(v)
    }

    fn draw(
        &mut self,
        shader: &Yuv420ToRgb10,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        f: &Frame,
    ) -> Result<()> {
        let (luma, chroma) = (self.get(&f.luma)?, self.get(&f.chroma)?);
        shader.draw(ctx, &luma, &chroma, rtv, f.width, f.height);
        Ok(())
    }
}
