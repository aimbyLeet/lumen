//! A monitor's capture, and the screens to stream now.

use anyhow::{Context, Result, bail};
use capture::{DesktopDuplicator, GpuDevice, Scene, SyntheticSource};
use gpu_convert::BgraToP010;
use input::Rect;
use windows::Win32::Graphics::Direct3D11::{ID3D11ShaderResourceView, ID3D11Texture2D};

use super::layout::{Spot, desktop_layout, synthetic_layout};
use crate::app::{Args, SceneArg, SourceArg};
use crate::source::Source;

/// A monitor's capture: it outlives sessions.
pub(crate) struct Screen {
    /// Where the picture is on the host's desktop (its size: even).
    pub(crate) place: Rect,
    pub(crate) source: Source,
    pub(super) frame_tex: ID3D11Texture2D,
    pub(super) converter: BgraToP010,
    pub(super) src_srv: ID3D11ShaderResourceView,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

// SAFETY: the D3D11 objects belong to the screen's own device, made
// multithread-protected (capture::create_device); a screen is used by one
// thread at a time (the station's, or its stream's in a session).
unsafe impl Send for Screen {}

impl Screen {
    fn open(source: Source, left: i32, top: i32) -> Result<Screen> {
        let gpu = source.gpu();
        if !gpu.is_nvidia {
            bail!("source is on '{}', not an NVIDIA GPU", gpu.adapter_name);
        }
        // 4:2:0 needs even sizes: an odd desktop (VMs, unusual modes) loses its
        // last column / row.
        let (full_w, full_h) = source.size();
        let (width, height) = (full_w & !1, full_h & !1);
        if (width, height) != (full_w, full_h) {
            println!("desktop {full_w}x{full_h} is odd-sized: streaming {width}x{height}");
        }
        let frame_tex = source.create_frame_texture()?;
        let converter = BgraToP010::new(&gpu.device, width, height)?;
        let src_srv = converter.source_view(&frame_tex)?;
        Ok(Screen {
            place: Rect {
                left,
                top,
                width: width as i32,
                height: height as i32,
            },
            source,
            frame_tex,
            converter,
            src_srv,
            width,
            height,
        })
    }

    pub(crate) fn gpu(&self) -> &GpuDevice {
        self.source.gpu()
    }

    /// Before a session: nothing of the last one's image stays.
    pub(crate) fn restart(&mut self) -> Result<()> {
        self.source.restart(&self.frame_tex)
    }
}

/// The screens to stream now, and the desktop layout they were made for
/// (`None` for synthetic ones: they do not change).
pub(crate) fn open_screens(args: &Args) -> Result<(Vec<Screen>, Option<Vec<Spot>>)> {
    match args.source {
        SourceArg::Desktop => {
            let layout = desktop_layout(args);
            let mut screens = Vec::new();
            for (i, s) in layout.iter().enumerate() {
                let d = DesktopDuplicator::new(s.adapter, s.output)
                    .with_context(|| format!("monitor {}", i + 1))?;
                let (left, top) = (d.left, d.top);
                let screen = Screen::open(Source::Desktop(d), left, top)?;
                println!(
                    "monitor {}: {}x{} at {},{} (adapter {} output {})",
                    i + 1,
                    screen.width,
                    screen.height,
                    left,
                    top,
                    s.adapter,
                    s.output
                );
                screens.push(screen);
            }
            for o in capture::list_outputs().unwrap_or_default() {
                if !layout
                    .iter()
                    .any(|s| (s.adapter, s.output) == (o.adapter_index, o.output_index))
                {
                    println!(
                        "not streamed: {} {}x{} on {}",
                        o.device_name, o.width, o.height, o.adapter_name
                    );
                }
            }
            Ok((screens, Some(layout)))
        }
        SourceArg::Synthetic => {
            let mut screens = Vec::new();
            for r in synthetic_layout(args)? {
                let mut src = SyntheticSource::new(
                    GpuDevice::open_nvidia()?,
                    r.width as u32,
                    r.height as u32,
                )?;
                src.set_scene(match args.scene {
                    SceneArg::Moving => Scene::Moving,
                    SceneArg::Still => Scene::Still { frame: 100 },
                    SceneArg::Mixed => Scene::Mixed { still_frame: 100 },
                });
                screens.push(Screen::open(Source::Synthetic(src), r.left, r.top)?);
            }
            Ok((screens, None))
        }
    }
}
