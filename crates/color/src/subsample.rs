//! 4:2:0 chroma subsampling / upsampling reference.
//!
//! Chroma siting: HEVC/AV1 default for 4:2:0 (`chroma_sample_loc_type = 0`,
//! "left"): horizontally co-sited with even luma columns, vertically centred
//! between two luma rows.
//!
//! Downsampling filter for chroma sample (i, j):
//!   rows    2j, 2j+1            weights 1/2, 1/2   (vertical centre)
//!   columns 2i-1, 2i, 2i+1      weights 1/4, 1/2, 1/4 (co-sited, anti-aliased)
//! Pb/Pr are linear in R'G'B', so we filter R'G'B' first and convert once —
//! the GPU shader does exactly the same thing.

use crate::ycbcr::*;

/// Planar 4:2:0 image with 10-bit codes (0..=1023) in `u16`, *not* P010-shifted.
/// Same layout as FFmpeg's `yuv420p10le`.
#[derive(Clone, Debug)]
pub struct Yuv420p10 {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u16>,
    pub u: Vec<u16>,
    pub v: Vec<u16>,
}

impl Yuv420p10 {
    pub fn chroma_width(&self) -> usize {
        self.width.div_ceil(2)
    }
    pub fn chroma_height(&self) -> usize {
        self.height.div_ceil(2)
    }

    /// Parses one frame of FFmpeg `-pix_fmt yuv420p10le -f rawvideo` output.
    pub fn from_yuv420p10le(bytes: &[u8], width: usize, height: usize) -> Option<Self> {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        let need = 2 * (width * height + 2 * cw * ch);
        if bytes.len() < need {
            return None;
        }
        let words: Vec<u16> = bytes[..need]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        let (y, rest) = words.split_at(width * height);
        let (u, v) = rest.split_at(cw * ch);
        Some(Self {
            width,
            height,
            y: y.to_vec(),
            u: u.to_vec(),
            v: v.to_vec(),
        })
    }

    pub fn frame_size_yuv420p10le(width: usize, height: usize) -> usize {
        2 * (width * height + 2 * width.div_ceil(2) * height.div_ceil(2))
    }
}

/// Reads pixel `(x, y)` of a BGRA8 buffer as non-linear R'G'B' in [0,1].
#[inline]
fn bgra_at(bgra: &[u8], width: usize, x: usize, y: usize) -> [f64; 3] {
    let i = (y * width + x) * 4;
    [
        bgra[i + 2] as f64 / 255.0,
        bgra[i + 1] as f64 / 255.0,
        bgra[i] as f64 / 255.0,
    ]
}

/// BGRA8 (DXGI_FORMAT_B8G8R8A8_UNORM, sRGB-encoded) → 4:2:0 10-bit full range.
pub fn bgra8_to_yuv420p10(bgra: &[u8], width: usize, height: usize) -> Yuv420p10 {
    assert_eq!(bgra.len(), width * height * 4);
    let mut y = vec![0u16; width * height];
    for py in 0..height {
        for px in 0..width {
            let [r, g, b] = bgra_at(bgra, width, px, py);
            y[py * width + px] = luma_code(rgb_to_ypbpr(r, g, b).0);
        }
    }

    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    const HW: [f64; 3] = [0.25, 0.5, 0.25];
    for j in 0..ch {
        for i in 0..cw {
            let mut acc = [0.0f64; 3];
            for dy in 0..2 {
                let yy = (2 * j + dy).min(height - 1);
                for (k, w) in HW.iter().enumerate() {
                    let xx = (2 * i + k).saturating_sub(1).min(width - 1);
                    let p = bgra_at(bgra, width, xx, yy);
                    for c in 0..3 {
                        acc[c] += 0.5 * w * p[c];
                    }
                }
            }
            let (_, pb, pr) = rgb_to_ypbpr(acc[0], acc[1], acc[2]);
            u[j * cw + i] = chroma_code(pb);
            v[j * cw + i] = chroma_code(pr);
        }
    }
    Yuv420p10 {
        width,
        height,
        y,
        u,
        v,
    }
}

/// Bilinear chroma upsampling matching the siting above, then Y'CbCr → R'G'B'.
/// Unclamped R'G'B' of pixel `(x, y)`; the client shader does the same per pixel.
///
/// Horizontal: even x is co-sited with chroma i = x/2; odd x is halfway between i and i+1.
/// Vertical: chroma row j sits at luma y = 2j + 0.5. Edges clamp.
pub fn yuv420p10_rgb_at(img: &Yuv420p10, x: usize, y: usize) -> [f64; 3] {
    let (cw, ch) = (img.chroma_width(), img.chroma_height());
    let cpix = |plane: &[u16], i: isize, j: isize| -> f64 {
        let i = i.clamp(0, cw as isize - 1) as usize;
        let j = j.clamp(0, ch as isize - 1) as usize;
        chroma_from_code(plane[j * cw + i])
    };
    let sample = |plane: &[u16]| -> f64 {
        let (i0, fx) = (
            (x / 2) as isize,
            if x.is_multiple_of(2) { 0.0 } else { 0.5 },
        );
        let (j0, fy) = if y.is_multiple_of(2) {
            ((y / 2) as isize - 1, 0.75) // between row j-1 (w .25) and j (w .75)
        } else {
            ((y / 2) as isize, 0.25) // between row j (w .75) and j+1 (w .25)
        };
        let row = |j: isize| (1.0 - fx) * cpix(plane, i0, j) + fx * cpix(plane, i0 + 1, j);
        (1.0 - fy) * row(j0) + fy * row(j0 + 1)
    };
    let yl = luma_from_code(img.y[y * img.width + x]);
    let (r, g, b) = ypbpr_to_rgb(yl, sample(&img.u), sample(&img.v));
    [r, g, b]
}

/// 4:2:0 → float R'G'B' clamped to [0,1], row-major `[r,g,b]`.
pub fn yuv420p10_to_rgb(img: &Yuv420p10) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(img.width * img.height);
    for y in 0..img.height {
        for x in 0..img.width {
            out.push(yuv420p10_rgb_at(img, x, y).map(|v| v.clamp(0.0, 1.0) as f32));
        }
    }
    out
}

/// R'G'B' value → 10-bit full-range code, as written to a `R10G10B10A2_UNORM` swapchain.
#[inline]
pub fn rgb10_code(v: f64) -> u16 {
    (v.clamp(0.0, 1.0) * MAX_CODE).round() as u16
}

/// 4:2:0 → 10-bit R'G'B' codes: the reference for the client's display shader.
pub fn yuv420p10_to_rgb10(img: &Yuv420p10) -> Vec<[u16; 3]> {
    let mut out = Vec::with_capacity(img.width * img.height);
    for y in 0..img.height {
        for x in 0..img.width {
            out.push(yuv420p10_rgb_at(img, x, y).map(rgb10_code));
        }
    }
    out
}

/// Float R'G'B' → BGRA8 (for previews / PNG output).
pub fn rgb_to_bgra8(rgb: &[[f32; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() * 4);
    for p in rgb {
        let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        out.extend_from_slice(&[q(p[2]), q(p[1]), q(p[0]), 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn flat_colours_survive_420_roundtrip() {
        // A flat field has no chroma detail, so 4:2:0 must be (almost) lossless.
        for c in [
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [40, 120, 200, 255],
        ] {
            let src = solid(8, 6, c);
            let yuv = bgra8_to_yuv420p10(&src, 8, 6);
            let back = rgb_to_bgra8(&yuv420p10_to_rgb(&yuv));
            for (a, b) in src.iter().zip(back.iter()) {
                assert!((*a as i32 - *b as i32).abs() <= 1, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn odd_dimensions_are_handled() {
        let src = solid(7, 5, [10, 20, 30, 255]);
        let yuv = bgra8_to_yuv420p10(&src, 7, 5);
        assert_eq!((yuv.chroma_width(), yuv.chroma_height()), (4, 3));
        assert_eq!(yuv420p10_to_rgb(&yuv).len(), 35);
    }

    #[test]
    fn yuv420p10le_parse_roundtrip() {
        let src = solid(4, 2, [1, 2, 3, 255]);
        let yuv = bgra8_to_yuv420p10(&src, 4, 2);
        let mut bytes = Vec::new();
        for p in [&yuv.y, &yuv.u, &yuv.v] {
            for w in p.iter() {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        assert_eq!(bytes.len(), Yuv420p10::frame_size_yuv420p10le(4, 2));
        let parsed = Yuv420p10::from_yuv420p10le(&bytes, 4, 2).unwrap();
        assert_eq!(parsed.y, yuv.y);
        assert_eq!(parsed.v, yuv.v);
    }

    #[test]
    fn rgb10_grey_ramp_is_exact() {
        // Neutral chroma: every 10-bit grey must come out as the same code on all channels.
        let w = 1024;
        let img = Yuv420p10 {
            width: w,
            height: 2,
            y: (0..2 * w).map(|i| (i % w) as u16).collect(),
            u: vec![512; w],
            v: vec![512; w],
        };
        for (i, p) in yuv420p10_to_rgb10(&img).iter().enumerate() {
            let c = (i % w) as u16;
            assert_eq!(*p, [c, c, c]);
        }
    }

    #[test]
    fn rgb10_flat_colours_match_8bit_source() {
        // 8-bit source code c maps to c * 1023 / 255 in 10 bit; 4:2:0 + Y'CbCr rounding
        // may move saturated colours by a couple of codes.
        for c in [
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [40, 120, 200, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
        ] {
            let yuv = bgra8_to_yuv420p10(&solid(8, 6, c), 8, 6);
            for p in yuv420p10_to_rgb10(&yuv) {
                for (ch, src) in [(0, c[2]), (1, c[1]), (2, c[0])] {
                    let want = f64::from(src) * 1023.0 / 255.0;
                    assert!((f64::from(p[ch]) - want).abs() <= 2.5, "{c:?}: {p:?}");
                }
            }
        }
    }

    #[test]
    fn rgb10_agrees_with_float_path() {
        let src: Vec<u8> = (0..16 * 8 * 4).map(|i| (i * 37 % 256) as u8).collect();
        let yuv = bgra8_to_yuv420p10(&src, 16, 8);
        for (a, b) in yuv420p10_to_rgb10(&yuv).iter().zip(yuv420p10_to_rgb(&yuv)) {
            for c in 0..3 {
                assert!((f64::from(a[c]) - f64::from(b[c]) * 1023.0).abs() <= 0.5 + 1e-4);
            }
        }
    }

    #[test]
    fn red_line_on_white_bleeds_chroma() {
        // A 1-px red vertical line on white: the classic 4:2:0 failure case.
        let (w, h) = (8, 4);
        let mut src = solid(w, h, [255, 255, 255, 255]);
        for y in 0..h {
            let i = (y * w + 3) * 4;
            src[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
        }
        let back = rgb_to_bgra8(&yuv420p10_to_rgb(&bgra8_to_yuv420p10(&src, w, h)));
        let i = (w + 3) * 4; // row 1, column 3
        // Red is no longer pure red: green/blue rise noticeably.
        assert!(back[i] > 30 && back[i + 1] > 30, "{:?}", &back[i..i + 4]);
    }
}
