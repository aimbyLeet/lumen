//! sRGB → CIELAB and the CIEDE2000 colour difference.
//!
//! Assumption: pixel values are sRGB-encoded (IEC 61966-2-1, D65). For
//! wide-gamut workflows the primaries matrix should come from the display
//! profile; that is a later extension (see `docs/architecture.md`).

/// sRGB electro-optical transfer function (non-linear → linear).
#[inline]
pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// D65 reference white (CIE 1931 2°), Y = 1.
pub const D65: [f64; 3] = [0.950_47, 1.0, 1.088_83];

/// Linear sRGB → XYZ (IEC 61966-2-1 matrix).
#[inline]
pub fn linear_srgb_to_xyz(r: f64, g: f64, b: f64) -> [f64; 3] {
    [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b,
        0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b,
    ]
}

#[inline]
fn lab_f(t: f64) -> f64 {
    const D: f64 = 6.0 / 29.0;
    if t > D * D * D {
        t.cbrt()
    } else {
        t / (3.0 * D * D) + 4.0 / 29.0
    }
}

/// XYZ → CIELAB relative to D65.
#[inline]
pub fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let fx = lab_f(xyz[0] / D65[0]);
    let fy = lab_f(xyz[1] / D65[1]);
    let fz = lab_f(xyz[2] / D65[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Non-linear sRGB in [0,1] → CIELAB.
#[inline]
pub fn srgb_to_lab(r: f64, g: f64, b: f64) -> [f64; 3] {
    xyz_to_lab(linear_srgb_to_xyz(
        srgb_to_linear(r),
        srgb_to_linear(g),
        srgb_to_linear(b),
    ))
}

/// Lookup table for the 8-bit source side (sRGB 8-bit → linear).
pub fn srgb8_linear_lut() -> [f64; 256] {
    let mut lut = [0.0; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = srgb_to_linear(i as f64 / 255.0);
    }
    lut
}

/// CIEDE2000 (kL = kC = kH = 1), Sharma, Wu, Dalal (2005).
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;

    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let c_bar = 0.5 * (c1 + c2);
    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);

    let hue = |b: f64, ap: f64| -> f64 {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let h1p = hue(b1, a1p);
    let h2p = hue(b2, a2p);

    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh_deg = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dh = 2.0 * (c1p * c2p).sqrt() * (dh_deg.to_radians() / 2.0).sin();

    let l_bar = 0.5 * (l1 + l2);
    let cp_bar = 0.5 * (c1p + c2p);
    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        0.5 * (h1p + h2p)
    } else if h1p + h2p < 360.0 {
        0.5 * (h1p + h2p + 360.0)
    } else {
        0.5 * (h1p + h2p - 360.0)
    };

    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let cp_bar7 = cp_bar.powi(7);
    let r_c = 2.0 * (cp_bar7 / (cp_bar7 + 25f64.powi(7))).sqrt();
    let l50 = (l_bar - 50.0).powi(2);
    let s_l = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let s_c = 1.0 + 0.045 * cp_bar;
    let s_h = 1.0 + 0.015 * cp_bar * t;
    let r_t = -(2.0 * d_theta * PI / 180.0).sin() * r_c;

    let tl = dl / s_l;
    let tc = dc / s_c;
    let th = dh / s_h;
    (tl * tl + tc * tc + th * th + r_t * tc * th).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test data from Sharma, Wu, Dalal, "The CIEDE2000 Color-Difference Formula" (2005).
    const SHARMA: &[([f64; 3], [f64; 3], f64)] = &[
        ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
        ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
        ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ([50.0, -1.3802, -84.2814], [50.0, 0.0, -82.7485], 1.0000),
        ([50.0, -1.1848, -84.8006], [50.0, 0.0, -82.7485], 1.0000),
        ([50.0, -0.9009, -85.5211], [50.0, 0.0, -82.7485], 1.0000),
        ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
        ([50.0, -1.0, 2.0], [50.0, 0.0, 0.0], 2.3669),
        ([50.0, 2.49, -0.001], [50.0, -2.49, 0.0009], 7.1792),
        ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        ([50.0, 2.5, 0.0], [61.0, -5.0, 29.0], 22.8977),
        ([50.0, 2.5, 0.0], [56.0, -27.0, -3.0], 31.9030),
        ([50.0, 2.5, 0.0], [58.0, 24.0, 15.0], 19.4535),
        (
            [60.2574, -34.0099, 36.2677],
            [60.4626, -34.1751, 39.4387],
            1.2644,
        ),
        (
            [63.0109, -31.0961, -5.8663],
            [62.8187, -29.7946, -4.0864],
            1.2630,
        ),
        (
            [22.7233, 20.0904, -46.6940],
            [23.0331, 14.9730, -42.5619],
            2.0373,
        ),
        (
            [2.0776, 0.0795, -1.1350],
            [0.9033, -0.0636, -0.5514],
            0.9082,
        ),
    ];

    #[test]
    fn ciede2000_matches_sharma_reference() {
        for (a, b, expected) in SHARMA {
            let got = delta_e_2000(*a, *b);
            assert!(
                (got - expected).abs() < 1e-4,
                "{a:?} {b:?}: got {got}, want {expected}"
            );
        }
    }

    #[test]
    fn white_is_l100() {
        let lab = srgb_to_lab(1.0, 1.0, 1.0);
        assert!((lab[0] - 100.0).abs() < 1e-3 && lab[1].abs() < 1e-2 && lab[2].abs() < 1e-2);
    }

    #[test]
    fn srgb_red_lab() {
        // Well-known value: sRGB red ≈ L 53.24, a 80.09, b 67.20
        let lab = srgb_to_lab(1.0, 0.0, 0.0);
        assert!((lab[0] - 53.24).abs() < 0.05);
        assert!((lab[1] - 80.09).abs() < 0.1);
        assert!((lab[2] - 67.20).abs() < 0.1);
    }
}
