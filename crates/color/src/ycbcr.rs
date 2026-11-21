//! R'G'B' ↔ Y'CbCr conversion, BT.709 matrix, full range, 10 bit.
//!
//! This is the *reference* for the GPU shader in `gpu-convert`: the shader
//! must produce bit-identical codes (see `shaders/bgra_to_p010.hlsl`).
//!
//! Formulas follow ITU-T H.273 for full range (`video_full_range_flag = 1`):
//!   DY  = round((2^n - 1) * E'Y)
//!   DCb = round((2^n - 1) * E'Pb + 2^(n-1))
//! with n = 10.

/// BT.709 luma coefficients.
pub const KR: f64 = 0.2126;
pub const KB: f64 = 0.0722;
pub const KG: f64 = 1.0 - KR - KB;

pub const BITS: u32 = 10;
pub const MAX_CODE: f64 = ((1u32 << BITS) - 1) as f64; // 1023
pub const CHROMA_MID: f64 = (1u32 << (BITS - 1)) as f64; // 512

/// Non-linear R'G'B' in [0,1] → (Y', Pb, Pr), Y' in [0,1], Pb/Pr in [-0.5, 0.5].
#[inline]
pub fn rgb_to_ypbpr(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let y = KR * r + KG * g + KB * b;
    let pb = (b - y) / (2.0 * (1.0 - KB));
    let pr = (r - y) / (2.0 * (1.0 - KR));
    (y, pb, pr)
}

/// Inverse of [`rgb_to_ypbpr`].
#[inline]
pub fn ypbpr_to_rgb(y: f64, pb: f64, pr: f64) -> (f64, f64, f64) {
    let r = y + 2.0 * (1.0 - KR) * pr;
    let b = y + 2.0 * (1.0 - KB) * pb;
    let g = (y - KR * r - KB * b) / KG;
    (r, g, b)
}

#[inline]
fn quant(v: f64) -> u16 {
    v.round().clamp(0.0, MAX_CODE) as u16
}

/// Y' → 10-bit full-range code.
#[inline]
pub fn luma_code(y: f64) -> u16 {
    quant(MAX_CODE * y)
}

/// Pb/Pr → 10-bit full-range code.
#[inline]
pub fn chroma_code(c: f64) -> u16 {
    quant(MAX_CODE * c + CHROMA_MID)
}

#[inline]
pub fn luma_from_code(code: u16) -> f64 {
    code as f64 / MAX_CODE
}

#[inline]
pub fn chroma_from_code(code: u16) -> f64 {
    (code as f64 - CHROMA_MID) / MAX_CODE
}

/// P010 stores the 10-bit code in the high bits of a 16-bit word.
#[inline]
pub fn code_to_p010(code: u16) -> u16 {
    code << 6
}

#[inline]
pub fn p010_to_code(word: u16) -> u16 {
    word >> 6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_roundtrip() {
        for &(r, g, b) in &[
            (0.0, 0.0, 0.0),
            (1.0, 1.0, 1.0),
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (0.3, 0.6, 0.9),
        ] {
            let (y, pb, pr) = rgb_to_ypbpr(r, g, b);
            let (r2, g2, b2) = ypbpr_to_rgb(y, pb, pr);
            assert!((r - r2).abs() < 1e-12 && (g - g2).abs() < 1e-12 && (b - b2).abs() < 1e-12);
        }
    }

    #[test]
    fn white_and_black_codes() {
        let (y, pb, pr) = rgb_to_ypbpr(1.0, 1.0, 1.0);
        assert_eq!(
            (luma_code(y), chroma_code(pb), chroma_code(pr)),
            (1023, 512, 512)
        );
        let (y, pb, pr) = rgb_to_ypbpr(0.0, 0.0, 0.0);
        assert_eq!(
            (luma_code(y), chroma_code(pb), chroma_code(pr)),
            (0, 512, 512)
        );
    }

    #[test]
    fn pure_blue_saturates_cb() {
        let (_, pb, _) = rgb_to_ypbpr(0.0, 0.0, 1.0);
        assert!((pb - 0.5).abs() < 1e-12);
        // 1023*0.5 + 512 = 1023.5 → clamped to 1023
        assert_eq!(chroma_code(pb), 1023);
    }

    #[test]
    fn grey_8bit_survives_10bit_roundtrip() {
        // Every 8-bit grey must come back to the same 8-bit value.
        for v in 0..=255u16 {
            let e = v as f64 / 255.0;
            let (y, pb, pr) = rgb_to_ypbpr(e, e, e);
            let (r, g, b) = ypbpr_to_rgb(
                luma_from_code(luma_code(y)),
                chroma_from_code(chroma_code(pb)),
                chroma_from_code(chroma_code(pr)),
            );
            for c in [r, g, b] {
                assert_eq!((c * 255.0).round() as u16, v);
            }
        }
    }

    #[test]
    fn p010_packing() {
        assert_eq!(code_to_p010(1023), 0xFFC0);
        assert_eq!(p010_to_code(0xFFC0), 1023);
    }
}
