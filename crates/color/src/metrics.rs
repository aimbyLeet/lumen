//! Image quality metrics: PSNR on R'G'B' and CIEDE2000 statistics.

use crate::lab::{delta_e_2000, linear_srgb_to_xyz, srgb_to_lab, srgb8_linear_lut, xyz_to_lab};
use rayon::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct DeltaEStats {
    pub mean: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    /// Fraction of pixels with ΔE00 above 1 (just noticeable difference).
    pub frac_over_1: f64,
    /// Above 2: visible to a trained eye side by side.
    pub frac_over_2: f64,
    /// Above 3: clearly visible.
    pub frac_over_3: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QualityReport {
    pub width: usize,
    pub height: usize,
    /// PSNR on 8-bit scale (peak 255) for R', G', B' and all channels.
    pub psnr_r: f64,
    pub psnr_g: f64,
    pub psnr_b: f64,
    pub psnr_rgb: f64,
    pub delta_e: DeltaEStats,
}

fn psnr(mse: f64) -> f64 {
    if mse <= 1e-12 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Compares a BGRA8 source with a reconstructed float R'G'B' image.
///
/// The reconstruction is *not* re-quantised to 8 bits: we measure the signal
/// the client gets, before the client's display path.
/// Returns the report and the per-pixel ΔE00 map.
pub fn compare(
    src_bgra: &[u8],
    rec: &[[f32; 3]],
    width: usize,
    height: usize,
) -> (QualityReport, Vec<f32>) {
    assert_eq!(src_bgra.len(), width * height * 4);
    assert_eq!(rec.len(), width * height);
    let lut = srgb8_linear_lut();

    let rows: Vec<([f64; 3], Vec<f32>)> = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut se = [0.0f64; 3];
            let mut de = Vec::with_capacity(width);
            for x in 0..width {
                let i = y * width + x;
                let s = &src_bgra[i * 4..i * 4 + 4];
                let (sr, sg, sb) = (s[2], s[1], s[0]);
                let r = rec[i];
                for (c, (sv, rv)) in [(sr, r[0]), (sg, r[1]), (sb, r[2])].into_iter().enumerate() {
                    let d = sv as f64 - rv as f64 * 255.0;
                    se[c] += d * d;
                }
                let lab_s = xyz_to_lab(linear_srgb_to_xyz(
                    lut[sr as usize],
                    lut[sg as usize],
                    lut[sb as usize],
                ));
                let lab_r = srgb_to_lab(r[0] as f64, r[1] as f64, r[2] as f64);
                de.push(delta_e_2000(lab_s, lab_r) as f32);
            }
            (se, de)
        })
        .collect();

    let n = (width * height) as f64;
    let mut se = [0.0f64; 3];
    let mut map = Vec::with_capacity(width * height);
    for (row_se, row_de) in rows {
        for c in 0..3 {
            se[c] += row_se[c];
        }
        map.extend(row_de);
    }

    let mut sorted = map.clone();
    sorted.par_sort_unstable_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize] as f64;
    let frac = |t: f32| map.iter().filter(|&&v| v > t).count() as f64 / n;

    let report = QualityReport {
        width,
        height,
        psnr_r: psnr(se[0] / n),
        psnr_g: psnr(se[1] / n),
        psnr_b: psnr(se[2] / n),
        psnr_rgb: psnr((se[0] + se[1] + se[2]) / (3.0 * n)),
        delta_e: DeltaEStats {
            mean: map.iter().map(|&v| v as f64).sum::<f64>() / n,
            p95: pct(0.95),
            p99: pct(0.99),
            max: *sorted.last().unwrap() as f64,
            frac_over_1: frac(1.0),
            frac_over_2: frac(2.0),
            frac_over_3: frac(3.0),
        },
    };
    (report, map)
}

/// Turns a ΔE map into an RGB8 heat map: black = 0, green ≈ 1, yellow ≈ 2, red ≥ 3+.
pub fn delta_e_heatmap_rgb8(map: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.len() * 3);
    for &d in map {
        let (r, g, b) = if d < 0.5 {
            (0.0, 0.0, 0.0)
        } else if d < 1.0 {
            (0.0, 0.25 + 0.5 * (d - 0.5) as f64, 0.0)
        } else if d < 2.0 {
            ((d - 1.0) as f64, 1.0, 0.0)
        } else if d < 3.0 {
            (1.0, 1.0 - (d - 2.0) as f64, 0.0)
        } else {
            (1.0, 0.0, ((d - 3.0) / 5.0).min(1.0) as f64)
        };
        out.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_are_perfect() {
        let src: Vec<u8> = (0..16 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let rec: Vec<[f32; 3]> = src
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                [
                    p[2] as f32 / 255.0,
                    p[1] as f32 / 255.0,
                    p[0] as f32 / 255.0,
                ]
            })
            .collect();
        let (r, _) = compare(&src, &rec, 4, 4);
        assert!(r.psnr_rgb > 100.0);
        assert!(r.delta_e.max < 1e-3);
    }

    #[test]
    fn one_code_error_is_tiny() {
        let src = vec![128u8; 4 * 4];
        let rec = vec![[129.0 / 255.0f32; 3]; 4];
        let (r, _) = compare(&src, &rec, 2, 2);
        assert!((r.psnr_rgb - 48.13).abs() < 0.01);
        assert!(r.delta_e.max < 0.6);
    }
}
