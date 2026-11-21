//! CPU readback of decoded frames, for verification only (never on the hot path).

use anyhow::{Result, ensure};
use color::subsample::Yuv420p10;
use gpu_convert::{desc_of, read_rows};
use windows::Win32::Graphics::Direct3D11::*;

use crate::Frame;

/// Copies one texture to a staging texture and returns its rows as 16-bit words.
fn read_words(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    words_per_row: usize,
    rows: usize,
) -> Result<Vec<u16>> {
    let mut out = Vec::with_capacity(words_per_row * rows);
    read_rows(
        device,
        ctx,
        tex,
        &desc_of(tex),
        rows,
        words_per_row * 2,
        |_, bytes| {
            out.extend(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_le_bytes(*b)),
            )
        },
    )?;
    Ok(out)
}

/// FNV-1a over all 10-bit codes (Y, then U, then V): compares decoded pictures
/// across runs and machines without keeping them.
pub fn frame_hash(p: &Yuv420p10) -> u64 {
    p.y.iter()
        .chain(&p.u)
        .chain(&p.v)
        .fold(0xcbf2_9ce4_8422_2325u64, |acc, &v| {
            (acc ^ u64::from(v)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Reads a decoded frame back as 10-bit codes. Fails if any word has non-zero
/// low 6 bits (the P016 output must be MSB-aligned 10-bit).
pub fn read_frame(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    frame: &Frame,
) -> Result<Yuv420p10> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let luma = read_words(device, ctx, &frame.luma, w, h)?;
    let chroma = read_words(device, ctx, &frame.chroma, w, h / 2)?;
    ensure!(
        luma.iter().chain(&chroma).all(|v| v & 0x3F == 0),
        "decoded words are not MSB-aligned 10-bit"
    );
    let (u, v): (Vec<u16>, Vec<u16>) = chroma
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| (p[0] >> 6, p[1] >> 6))
        .unzip();
    Ok(Yuv420p10 {
        width: w,
        height: h,
        y: luma.into_iter().map(|v| v >> 6).collect(),
        u,
        v,
    })
}
