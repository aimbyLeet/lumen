//! The picture the host streams: a monitor (Desktop Duplication) or a
//! synthetic test source.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use capture::{Acquired, DesktopDuplicator, GpuDevice, Pointer, SyntheticSource};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

/// Sleeps until about `t`: up to ~0.7 ms late (Windows wakes a sleeping thread
/// on its timer's 0.5 ms tick). `transport::sleep_until` is exact but spins
/// the last 1.5 ms: 9 % of a core at 60 fps, for nothing here — a change of
/// the desktop meanwhile waits for the capture, it is not lost.
fn nap_until(t: Instant) {
    std::thread::sleep(t.saturating_duration_since(Instant::now()));
}

pub(crate) enum Source {
    Desktop(DesktopDuplicator),
    Synthetic(SyntheticSource),
}

impl Source {
    /// What changed in the image since the last call.
    pub(crate) fn take_changes(&mut self) -> capture::regions::Changes {
        match self {
            Source::Desktop(d) => d.take_changes(),
            Source::Synthetic(s) => s.take_changes(),
        }
    }

    pub(crate) fn gpu(&self) -> &GpuDevice {
        match self {
            Source::Desktop(d) => d.gpu(),
            Source::Synthetic(s) => s.gpu(),
        }
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        match self {
            Source::Desktop(d) => (d.width, d.height),
            Source::Synthetic(s) => (s.width, s.height),
        }
    }

    pub(crate) fn create_frame_texture(&self) -> Result<ID3D11Texture2D> {
        match self {
            Source::Desktop(d) => d.create_frame_texture(),
            Source::Synthetic(s) => s.create_frame_texture(),
        }
    }

    /// Before a session: nothing of the last one's image stays in `dst`.
    pub(crate) fn restart(&mut self, dst: &ID3D11Texture2D) -> Result<()> {
        match self {
            Source::Desktop(d) => d.restart(dst),
            // Every frame is rendered whole.
            Source::Synthetic(_) => Ok(()),
        }
    }

    /// Puts frame `frame` into `dst`, one frame per tick: `tick` ends the
    /// frame's slot, which opens one `interval` earlier. Synthetic: rendered
    /// at the tick. Desktop: taken at the first change of the image within the
    /// slot, else at its end (an unchanged desktop is encoded again: a cheap
    /// P-frame). Pointer-only updates do not end the wait; they go to
    /// `on_pointer` at once. True if the image is new (not the last one again).
    pub(crate) fn grab(
        &mut self,
        frame: u32,
        dst: &ID3D11Texture2D,
        tick: Instant,
        interval: Duration,
        mut on_pointer: impl FnMut(&Pointer) -> Result<()>,
    ) -> Result<bool> {
        match self {
            Source::Desktop(d) => {
                if let Some(open) = tick.checked_sub(interval) {
                    nap_until(open);
                }
                loop {
                    let left = tick.saturating_duration_since(Instant::now());
                    // Whole milliseconds, rounded up: never give up before the tick.
                    let ms = left.as_micros().div_ceil(1000) as u32;
                    match d.acquire_into(ms, dst)? {
                        Acquired::NewFrame { .. } => return Ok(true),
                        Acquired::PointerOnly => on_pointer(&d.pointer)?,
                        Acquired::Timeout if left.is_zero() => return Ok(false),
                        Acquired::Timeout => {}
                    }
                }
            }
            Source::Synthetic(s) => {
                nap_until(tick);
                s.render_into(frame, dst).map(|()| true)
            }
        }
    }
}

pub(crate) fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .context("--size must look like 3840x2160")?;
    Ok((w.parse()?, h.parse()?))
}
