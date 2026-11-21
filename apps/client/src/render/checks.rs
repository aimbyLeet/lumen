//! The renderers of checks: test pattern, black fill, video file, scripted input.

use anyhow::{Context, Result, anyhow};
use color::subsample::yuv420p10_to_rgb10;
use decoder::{Decoder, Frame};

use protocol::{InputEvent, MouseButton};

use super::{Expected, Renderer, ViewCache};
use crate::net::Connection;
use gpu_convert::{Yuv420ToRgb10, compile_src};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::core::s;

const TEST_PATTERN_SRC: &str = include_str!("../../shaders/test_pattern.hlsl");

fn viewport(width: u32, height: u32) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: width as f32,
        Height: height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    }
}

/// Exact-code pattern: R = x mod 1024, G = y mod 1024, B = 1023 − (x mod 1024).
pub struct TestPattern {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    width: u32,
    height: u32,
}

impl TestPattern {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        let name = s!("test_pattern.hlsl");
        let vs_bc = compile_src(TEST_PATTERN_SRC, name, s!("vs_main"), s!("vs_5_0"))?;
        let ps_bc = compile_src(TEST_PATTERN_SRC, name, s!("ps_main"), s!("ps_5_0"))?;
        let mut vs = None;
        let mut ps = None;
        unsafe {
            device.CreateVertexShader(&vs_bc, None, Some(&mut vs))?;
            device.CreatePixelShader(&ps_bc, None, Some(&mut ps))?;
        }
        Ok(TestPattern {
            vs: vs.ok_or_else(|| anyhow!("no VS"))?,
            ps: ps.ok_or_else(|| anyhow!("no PS"))?,
            width,
            height,
        })
    }
}

impl Renderer for TestPattern {
    fn draw(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        _frame: u64,
    ) -> Result<bool> {
        unsafe {
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[viewport(self.width, self.height)]));
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.Draw(3, 0);
        }
        Ok(true)
    }

    fn expected(&mut self, _ctx: &ID3D11DeviceContext) -> Result<Option<Expected>> {
        let (w, h) = (self.width as usize, self.height as usize);
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as u16 & 1023, (i / w) as u16 & 1023);
                [x, y, 1023 - x]
            })
            .collect();
        Ok(Some(Expected {
            pixels,
            tolerance: 0,
        }))
    }
}

/// Plain black fill for when no renderer is selected.
pub struct Clear;

impl Renderer for Clear {
    fn draw(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        _frame: u64,
    ) -> Result<bool> {
        unsafe { ctx.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0]) };
        Ok(true)
    }

    fn expected(&mut self, _ctx: &ID3D11DeviceContext) -> Result<Option<Expected>> {
        Ok(None)
    }
}

/// Plays an HEVC Annex B file: one access unit per presented frame, NVDEC →
/// D3D11 textures → display shader → back buffer, all on the GPU.
pub struct VideoFile {
    decoder: Decoder,
    shader: Yuv420ToRgb10,
    aus: Vec<Vec<u8>>,
    next: usize,
    current: Option<Frame>,
    views: ViewCache,
}

impl VideoFile {
    pub fn open(device: &ID3D11Device, path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read {path:?}"))?;
        let aus: Vec<Vec<u8>> = decoder::annexb::split_access_units(&data)
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect();
        anyhow::ensure!(!aus.is_empty(), "no access units in {path:?}");
        Ok(VideoFile {
            decoder: Decoder::new(device)?,
            shader: Yuv420ToRgb10::new(device)?,
            aus,
            next: 0,
            current: None,
            views: ViewCache::new(device),
        })
    }

    /// Decodes the first picture to learn the video size; it is shown as frame 0.
    pub fn size(&mut self) -> Result<(u32, u32)> {
        if self.current.is_none() {
            self.decode_next()?;
        }
        let f = self
            .current
            .as_ref()
            .context("first access unit gave no picture")?;
        Ok((f.width, f.height))
    }

    pub fn len(&self) -> usize {
        self.aus.len()
    }

    fn decode_next(&mut self) -> Result<bool> {
        while self.next < self.aus.len() {
            let i = self.next;
            self.next += 1;
            self.decoder.decode(&self.aus[i], i as i64)?;
            if let Some(f) = self.decoder.next_frame() {
                self.current = Some(f);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Renderer for VideoFile {
    fn draw(
        &mut self,
        ctx: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        frame: u64,
    ) -> Result<bool> {
        // Frame 0 was decoded by `size()`.
        if (frame > 0 || self.current.is_none()) && !self.decode_next()? {
            return Ok(false);
        }
        let f = self.current.as_ref().context("no picture")?;
        self.views.draw(&self.shader, ctx, rtv, f)?;
        Ok(true)
    }

    fn expected(&mut self, ctx: &ID3D11DeviceContext) -> Result<Option<Expected>> {
        let f = self.current.as_ref().context("no picture")?;
        let yuv = decoder::read_frame(&self.views.device, ctx, f)?;
        Ok(Some(Expected {
            pixels: yuv420p10_to_rgb10(&yuv),
            tolerance: 1,
        }))
    }
}

/// Deterministic input sequence for end-to-end checks: every kind of event,
/// keys and buttons in down/up pairs. The sent sequence is written to a log;
/// the host (`--input-log`) writes what it applied, and the two must match.
pub struct InputScript {
    events: Vec<InputEvent>,
    next: usize,
    per_frame: usize,
    log: std::io::BufWriter<std::fs::File>,
}

impl InputScript {
    pub fn new(path: &std::path::Path, count: usize, width: u16, height: u16) -> Result<Self> {
        let (w, h) = (i32::from(width), i32::from(height));
        let keys = [(0x1E, false), (0x4B, true), (0x1D, true), (0x76, false)]; // A, Left, RCtrl, F24
        let buttons = [MouseButton::Left, MouseButton::Right, MouseButton::X1];
        let mut events = Vec::with_capacity(count);
        let mut i = 0i32;
        while events.len() < count {
            events.push(InputEvent::MouseMove {
                x: (i * 7) % w,
                y: (i * 13) % h,
            });
            let k = keys[(i as usize) % keys.len()];
            match i % 4 {
                0 => {
                    for down in [true, false] {
                        events.push(InputEvent::Key {
                            scancode: k.0,
                            extended: k.1,
                            down,
                        });
                    }
                }
                1 => {
                    let button = buttons[(i as usize / 4) % buttons.len()];
                    for down in [true, false] {
                        events.push(InputEvent::MouseButton { button, down });
                    }
                }
                2 => events.push(InputEvent::MouseWheel {
                    dx: if i % 8 == 2 { 120 } else { 0 },
                    dy: -120,
                }),
                _ => {}
            }
            i += 1;
        }
        events.truncate(count);
        Ok(InputScript {
            events,
            next: 0,
            per_frame: 10,
            log: std::io::BufWriter::new(
                std::fs::File::create(path).with_context(|| format!("create {path:?}"))?,
            ),
        })
    }

    /// Sends the next batch; returns false once everything was sent.
    pub(super) fn step(&mut self, conn: &Connection) -> Result<bool> {
        use std::io::Write;
        let end = (self.next + self.per_frame).min(self.events.len());
        for (id, ev) in self.events[self.next..end].iter().enumerate() {
            conn.send_input(ev.clone());
            writeln!(self.log, "{}\t{ev:?}", self.next + id)?;
        }
        self.next = end;
        if self.next == self.events.len() {
            self.log.flush()?;
        }
        Ok(self.next < self.events.len())
    }
}
