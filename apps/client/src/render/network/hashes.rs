//! Hashes of decoded pictures for the end-to-end bit-exactness check (--frame-hashes).

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D11::*;

/// Reads decoded pictures back on the render thread (the GPU copy has to happen
/// there) and hashes / writes them on a worker, so the window keeps pumping
/// messages. Verification only: costs a 4K readback per frame.
pub(super) struct HashSink {
    device: ID3D11Device,
    tx: Option<std::sync::mpsc::Sender<(i64, color::subsample::Yuv420p10)>>,
    worker: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

impl HashSink {
    pub(super) fn new(device: &ID3D11Device, path: &std::path::Path) -> Result<Self> {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(
            std::fs::File::create(path).with_context(|| format!("create {path:?}"))?,
        );
        let (tx, rx) = std::sync::mpsc::channel::<(i64, color::subsample::Yuv420p10)>();
        let worker = std::thread::spawn(move || {
            for (frame, picture) in rx {
                writeln!(out, "{frame} {:016x}", decoder::frame_hash(&picture))?;
            }
            out.flush()
        });
        Ok(HashSink {
            device: device.clone(),
            tx: Some(tx),
            worker: Some(worker),
        })
    }

    pub(super) fn push(&self, ctx: &ID3D11DeviceContext, frame: &decoder::Frame) -> Result<()> {
        let picture = decoder::read_frame(&self.device, ctx, frame)?;
        if let Some(tx) = &self.tx {
            tx.send((frame.timestamp, picture))
                .map_err(|_| anyhow::anyhow!("hash writer stopped"))?;
        }
        Ok(())
    }
}

impl Drop for HashSink {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(Ok(Err(e))) = self.worker.take().map(|w| w.join()) {
            eprintln!("--frame-hashes: {e}");
        }
    }
}
