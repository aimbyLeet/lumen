//! lumen-patterns: generates a test chart that stresses 4:2:0 and the codec.
//!
//! Open the PNG full-screen at 100% zoom on the host (native resolution,
//! no scaling) while lumen-bench is running, or feed it directly to
//! `lumen-compare image` to see the 4:2:0 floor without any GPU.
//!
//! Bands, top to bottom:
//!   1. grey / R / G / B / C / M / Y ramps          → banding, 10-bit headroom
//!   2. 1-px and 2-px coloured lines on white/black → chroma bleeding
//!   3. 1-px coloured checkerboards                 → worst case for 4:2:0
//!   4. coloured text on coloured backgrounds       → UI / annotation legibility
//!   5. ColorChecker-like patches (approx. sRGB)    → flat colour accuracy
//!   6. hue sweep + 2-D gradient                    → smooth colour transitions

use anyhow::Result;
use clap::Parser;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lumen-patterns", about = "Generate a colour test chart PNG")]
struct Args {
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    #[arg(long, short)]
    out: Option<PathBuf>,
}

type Rgb = [u8; 3];

/// The smallest chart: 8 px margins in each half of the lines band and in
/// the half-height of a band (a sixth of the height).
const MIN_WIDTH: usize = 32;
const MIN_HEIGHT: usize = 96;

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![128; w * h * 3],
        }
    }
    fn set(&mut self, x: usize, y: usize, c: Rgb) {
        if x < self.w && y < self.h {
            let i = (y * self.w + x) * 3;
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }
    fn rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Rgb) {
        for y in y0..(y0 + h).min(self.h) {
            for x in x0..(x0 + w).min(self.w) {
                self.set(x, y, c);
            }
        }
    }
    fn text(&mut self, x0: usize, y0: usize, s: &str, scale: usize, c: Rgb) -> usize {
        let mut x = x0;
        for ch in s.chars() {
            if let Some(g) = glyph(ch) {
                for (row, bits) in g.iter().enumerate() {
                    for col in 0..5 {
                        if bits & (0x10 >> col) != 0 {
                            self.rect(x + col * scale, y0 + row * scale, scale, scale, c);
                        }
                    }
                }
            }
            x += 6 * scale;
        }
        x
    }
}

/// Classic 5×7 bitmap font, rows top→bottom, bit 4 = leftmost column.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        ' ' => [0; 7],
        _ => return None,
    })
}

const WHITE: Rgb = [255, 255, 255];
const BLACK: Rgb = [0, 0, 0];
const RED: Rgb = [255, 0, 0];
const GREEN: Rgb = [0, 255, 0];
const BLUE: Rgb = [0, 0, 255];
const CYAN: Rgb = [0, 255, 255];
const MAGENTA: Rgb = [255, 0, 255];
const YELLOW: Rgb = [255, 255, 0];

/// Approximate sRGB values of the 24 ColorChecker Classic patches (D65).
const CHECKER: [Rgb; 24] = [
    [115, 82, 68],
    [194, 150, 130],
    [98, 122, 157],
    [87, 108, 67],
    [133, 128, 177],
    [103, 189, 170],
    [214, 126, 44],
    [80, 91, 166],
    [193, 90, 99],
    [94, 60, 108],
    [157, 188, 64],
    [224, 163, 46],
    [56, 61, 150],
    [70, 148, 73],
    [175, 54, 60],
    [231, 199, 31],
    [187, 86, 149],
    [8, 133, 161],
    [243, 243, 242],
    [200, 200, 200],
    [160, 160, 160],
    [122, 122, 121],
    [85, 85, 85],
    [52, 52, 52],
];

fn hsv(h: f64, s: f64, v: f64) -> Rgb {
    let c = v * s;
    let hp = (h / 60.0) % 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let q = |f: f64| ((f + m) * 255.0).round() as u8;
    [q(r), q(g), q(b)]
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (w, h) = (args.width as usize, args.height as usize);
    // The bands' margins: a smaller chart would run out of them (and in a
    // release build wrap around: the lines band then never ended).
    anyhow::ensure!(
        w >= MIN_WIDTH && h >= MIN_HEIGHT,
        "the chart needs at least {MIN_WIDTH}x{MIN_HEIGHT} pixels"
    );
    let mut cv = Canvas::new(w, h);
    let band = h / 6;

    draw_ramps(&mut cv, w, band);
    draw_lines(&mut cv, w, band);
    draw_checkerboards(&mut cv, w, band);
    draw_text_rows(&mut cv, w, band);
    draw_checker_patches(&mut cv, w, band);
    draw_hue_and_gradient(&mut cv, w, h, band);

    let out = args
        .out
        .unwrap_or_else(|| PathBuf::from(format!("chart_{w}x{h}.png")));
    write_png(&cv, &out)?;
    println!("Wrote {}", out.display());
    Ok(())
}

/// Band 1. Ramps: 7 strips (grey, R, G, B, C, M, Y).
fn draw_ramps(cv: &mut Canvas, w: usize, band: usize) {
    let ramps: [Rgb; 7] = [WHITE, RED, GREEN, BLUE, CYAN, MAGENTA, YELLOW];
    let strip = band / ramps.len();
    for (k, c) in ramps.iter().enumerate() {
        for x in 0..w {
            let t = x as f64 / (w - 1) as f64;
            let col = [
                (c[0] as f64 * t).round() as u8,
                (c[1] as f64 * t).round() as u8,
                (c[2] as f64 * t).round() as u8,
            ];
            for y in k * strip..(k + 1) * strip {
                cv.set(x, y, col);
            }
        }
    }
}

/// Band 2. Coloured lines (1 px and 2 px) on white and on black.
fn draw_lines(cv: &mut Canvas, w: usize, band: usize) {
    let y0 = band;
    cv.rect(0, y0, w / 2, band, WHITE);
    cv.rect(w / 2, y0, w - w / 2, band, BLACK);
    let line_colours = [
        RED,
        GREEN,
        BLUE,
        MAGENTA,
        CYAN,
        YELLOW,
        [255, 128, 0],
        [128, 0, 255],
    ];
    for half in 0..2 {
        let x_start = half * w / 2 + 8;
        let x_end = (half + 1) * w / 2 - 8;
        let mut x = x_start;
        let mut i = 0;
        while x + 2 < x_end {
            let thick = if (i / line_colours.len()).is_multiple_of(2) {
                1
            } else {
                2
            };
            cv.rect(
                x,
                y0 + 4,
                thick,
                band / 2 - 8,
                line_colours[i % line_colours.len()],
            );
            x += thick + 3;
            i += 1;
        }
        // horizontal lines below
        for (j, c) in line_colours.iter().enumerate() {
            let yy = y0 + band / 2 + 4 + j * 4;
            cv.rect(x_start, yy, x_end - x_start, 1 + j % 2, *c);
        }
    }
}

/// Band 3. 1-px checkerboards of complementary colours.
fn draw_checkerboards(cv: &mut Canvas, w: usize, band: usize) {
    let y0 = 2 * band;
    let pairs: [(Rgb, Rgb); 6] = [
        (RED, BLUE),
        (RED, GREEN),
        (MAGENTA, GREEN),
        (YELLOW, BLUE),
        (RED, CYAN),
        (BLACK, RED),
    ];
    let cell = w / pairs.len();
    for (k, (a, b)) in pairs.iter().enumerate() {
        for y in y0..y0 + band {
            for x in k * cell..(k + 1) * cell {
                cv.set(x, y, if (x + y) % 2 == 0 { *a } else { *b });
            }
        }
    }
}

/// Band 4. Coloured text on coloured backgrounds, scale 1 and 2.
fn draw_text_rows(cv: &mut Canvas, w: usize, band: usize) {
    let y0 = 3 * band;
    let combos: [(Rgb, Rgb); 8] = [
        (RED, BLUE),
        (BLUE, RED),
        (GREEN, MAGENTA),
        (BLACK, RED),
        (RED, BLACK),
        (WHITE, BLUE),
        (YELLOW, WHITE),
        ([0, 160, 0], [230, 230, 230]),
    ];
    let row_h = band / combos.len();
    for (k, (fg, bg)) in combos.iter().enumerate() {
        let y = y0 + k * row_h;
        cv.rect(0, y, w, row_h, *bg);
        let x = cv.text(
            6,
            y + (row_h.saturating_sub(7)) / 2,
            "COLOR 4:2:0 TEST 0123456789 ",
            1,
            *fg,
        );
        if row_h >= 16 {
            cv.text(x + 12, y + (row_h - 14) / 2, "GRADE LUT 709 P3", 2, *fg);
        }
    }
}

/// Band 5. ColorChecker-like patches, 6 x 4.
fn draw_checker_patches(cv: &mut Canvas, w: usize, band: usize) {
    let y0 = 4 * band;
    let pw = w / 6;
    let ph = band / 4;
    for (i, c) in CHECKER.iter().enumerate() {
        let (cx, cy) = (i % 6, i / 6);
        cv.rect(cx * pw + 2, y0 + cy * ph + 2, pw - 4, ph - 4, *c);
    }
}

/// Band 6. Hue sweep (top half) and 2-D gradient (bottom half).
fn draw_hue_and_gradient(cv: &mut Canvas, w: usize, h: usize, band: usize) {
    let y0 = 5 * band;
    let rest = h - y0;
    for x in 0..w {
        let c = hsv(360.0 * x as f64 / w as f64, 1.0, 1.0);
        for y in y0..y0 + rest / 2 {
            cv.set(x, y, c);
        }
    }
    for y in y0 + rest / 2..h {
        let v = (y - (y0 + rest / 2)) as f64 / (h - (y0 + rest / 2)).max(1) as f64;
        for x in 0..w {
            cv.set(x, y, hsv(360.0 * x as f64 / w as f64, 0.6, 0.35 + 0.6 * v));
        }
    }
}

fn write_png(cv: &Canvas, out: &std::path::Path) -> Result<()> {
    let mut enc = png::Encoder::new(BufWriter::new(File::create(out)?), cv.w as u32, cv.h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    enc.write_header()?.write_image_data(&cv.px)?;
    Ok(())
}
