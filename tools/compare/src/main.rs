//! lumen-compare: measures what the pipeline does to colour.
//!
//! Two modes:
//!   image   --png shot.png         4:2:0 loss alone, on any image (no GPU needed)
//!   barcode --dir bench-out        checks the frame-number barcode of every decoded frame
//!   capture --dir bench-out        full analysis of a lumen-bench run:
//!       A. 4:2:0 (CPU reference)      — the theoretical floor of the format
//!       B. 4:2:0 + codec (decoded)    — what the client actually gets
//!       C. GPU shader vs CPU reference (with bench --verify)

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use color::metrics::{QualityReport, compare, delta_e_heatmap_rgb8};
use color::subsample::{Yuv420p10, bgra8_to_yuv420p10, rgb_to_bgra8, yuv420p10_to_rgb};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "lumen-compare",
    about = "Colour accuracy analysis (ΔE2000, PSNR)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Analyse 4:2:0 loss on a PNG (screenshot of real work, test pattern...).
    Image {
        #[arg(long)]
        png: PathBuf,
        /// Where to write heat map and preview (default: next to the PNG).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Check that decoded frames carry consecutive barcode numbers 0, 1, 2, ...
    /// (bench run with --source synthetic).
    Barcode {
        #[arg(long)]
        dir: PathBuf,
        /// Decoded stream as yuv420p10le (default: <dir>/decoded.yuv).
        #[arg(long)]
        decoded: Option<PathBuf>,
    },
    /// Analyse a lumen-bench output directory.
    Capture {
        #[arg(long)]
        dir: PathBuf,
        /// Decoded stream as yuv420p10le (default: <dir>/decoded.yuv).
        #[arg(long)]
        decoded: Option<PathBuf>,
        /// Skip writing ΔE heat maps and decoded previews as PNG.
        #[arg(long)]
        no_heatmaps: bool,
        /// Also report B for the parts left and right of this column (a
        /// lumen-bench --qp-map run).
        #[arg(long)]
        split_x: Option<usize>,
        /// Also report D inside and outside this rectangle: X0,Y0,X1,Y1 (the
        /// moving window of lumen-bench --scene mixed).
        #[arg(long, value_parser = parse_rect)]
        region: Option<[usize; 4]>,
        /// Also report D for the blocks that moved and the rest: blocks (32 px)
        /// that changed in at least 10 % of the frames of lumen-bench's
        /// changed_blocks.bin.
        #[arg(long)]
        changed_mask: bool,
        /// The decoded file holds only the dumped frames, in their order
        /// (ffmpeg -vf select=...): 4K runs without the whole stream on disk.
        #[arg(long)]
        sparse: bool,
    },
}

#[derive(Deserialize)]
struct Meta {
    width: u32,
    height: u32,
    dumped_frames: Vec<u32>,
    #[serde(default)]
    frames: u32,
    #[serde(default)]
    verify: bool,
}

#[derive(Serialize)]
struct FrameResult {
    frame: u32,
    chroma_420_only: QualityReport,
    decoded: Option<QualityReport>,
    gpu_vs_reference: Option<ShaderCheck>,
}

#[derive(Serialize, Clone, Copy)]
struct ShaderCheck {
    samples: usize,
    mismatched: usize,
    max_abs_diff: u16,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Image { png, out } => image_mode(&png, out),
        Cmd::Barcode { dir, decoded } => barcode_mode(&dir, decoded),
        Cmd::Capture {
            dir,
            decoded,
            no_heatmaps,
            split_x,
            region,
            sparse,
            changed_mask,
        } => {
            let mask = if changed_mask {
                Some(load_changed_mask(&dir)?)
            } else {
                None
            };
            capture_mode(&dir, decoded, !no_heatmaps, split_x, region, sparse, mask)
        }
    }
}

fn load_png_bgra(path: &Path) -> Result<(Vec<u8>, usize, usize)> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(File::open(path)?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("png size")?];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = &buf[..info.buffer_size()];
    let mut bgra = Vec::with_capacity(w * h * 4);
    match info.color_type {
        png::ColorType::Rgba => px
            .as_chunks::<4>()
            .0
            .iter()
            .for_each(|p| bgra.extend_from_slice(&[p[2], p[1], p[0], 255])),
        png::ColorType::Rgb => px
            .as_chunks::<3>()
            .0
            .iter()
            .for_each(|p| bgra.extend_from_slice(&[p[2], p[1], p[0], 255])),
        png::ColorType::GrayscaleAlpha => px
            .as_chunks::<2>()
            .0
            .iter()
            .for_each(|p| bgra.extend_from_slice(&[p[0], p[0], p[0], 255])),
        png::ColorType::Grayscale => px
            .iter()
            .for_each(|&g| bgra.extend_from_slice(&[g, g, g, 255])),
        other => bail!("unsupported PNG colour type {other:?}"),
    }
    Ok((bgra, w, h))
}

fn write_png_rgb(path: &Path, rgb: &[u8], w: usize, h: usize) -> Result<()> {
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect()
}

fn print_report(label: &str, r: &QualityReport) {
    let d = &r.delta_e;
    println!(
        "  {label:<26} PSNR {:6.2} dB | ΔE00 mean {:5.3} p95 {:5.2} p99 {:5.2} max {:5.2} | >1: {:6.3}%  >2: {:6.3}%  >3: {:6.3}%",
        r.psnr_rgb,
        d.mean,
        d.p95,
        d.p99,
        d.max,
        d.frac_over_1 * 100.0,
        d.frac_over_2 * 100.0,
        d.frac_over_3 * 100.0
    );
}

fn image_mode(png_path: &Path, out: Option<PathBuf>) -> Result<()> {
    let (bgra, w, h) = load_png_bgra(png_path)?;
    let yuv = bgra8_to_yuv420p10(&bgra, w, h);
    let rec = yuv420p10_to_rgb(&yuv);
    let (report, map) = compare(&bgra, &rec, w, h);
    println!("{} ({w}x{h})", png_path.display());
    print_report("4:2:0 10-bit (no codec)", &report);

    let out = out.unwrap_or_else(|| png_path.parent().unwrap_or(Path::new(".")).to_path_buf());
    std::fs::create_dir_all(&out)?;
    let stem = png_path.file_stem().unwrap().to_string_lossy();
    write_png_rgb(
        &out.join(format!("{stem}.de420.png")),
        &delta_e_heatmap_rgb8(&map),
        w,
        h,
    )?;
    write_png_rgb(
        &out.join(format!("{stem}.420.png")),
        &bgra_to_rgb(&rgb_to_bgra8(&rec)),
        w,
        h,
    )?;
    println!(
        "  heat map: {stem}.de420.png (black <0.5, green <1, yellow ~2, red ≥3), preview: {stem}.420.png"
    );
    Ok(())
}

fn read_decoded_frame(f: &mut File, idx: u32, w: usize, h: usize) -> Result<Option<Yuv420p10>> {
    let size = Yuv420p10::frame_size_yuv420p10le(w, h);
    let len = f.metadata()?.len();
    let off = idx as u64 * size as u64;
    if off + size as u64 > len {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; size];
    f.read_exact(&mut buf)?;
    Ok(Yuv420p10::from_yuv420p10le(&buf, w, h))
}

fn shader_check(gpu: &Yuv420p10, reference: &Yuv420p10) -> ShaderCheck {
    let mut c = ShaderCheck {
        samples: 0,
        mismatched: 0,
        max_abs_diff: 0,
    };
    for (a, b) in [
        (&gpu.y, &reference.y),
        (&gpu.u, &reference.u),
        (&gpu.v, &reference.v),
    ] {
        for (x, y) in a.iter().zip(b.iter()) {
            c.samples += 1;
            let d = x.abs_diff(*y);
            if d != 0 {
                c.mismatched += 1;
                c.max_abs_diff = c.max_abs_diff.max(d);
            }
        }
    }
    c
}

fn parse_rect(s: &str) -> Result<[usize; 4], String> {
    let v: Vec<usize> = s
        .split(',')
        .map(|p| p.trim().parse().map_err(|e| format!("{p}: {e}")))
        .collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected X0,Y0,X1,Y1".into())
}

/// The codec's own loss: PSNR (dB, 10-bit) of the decoded planes against the
/// 4:2:0 reference over the luma samples where `pick(x, y)` holds (chroma:
/// where it holds for the sample's top-left luma position).
fn codec_psnr(
    dec: &Yuv420p10,
    reference: &Yuv420p10,
    pick: &dyn Fn(usize, usize) -> bool,
) -> [f64; 3] {
    let plane = |a: &[u16], b: &[u16], w: usize, scale: usize| {
        let (mut se, mut n) = (0f64, 0usize);
        for (y, (ra, rb)) in a.chunks_exact(w).zip(b.chunks_exact(w)).enumerate() {
            for (x, (p, q)) in ra.iter().zip(rb).enumerate() {
                if !pick(x * scale, y * scale) {
                    continue;
                }
                let d = f64::from(*p) - f64::from(*q);
                se += d * d;
                n += 1;
            }
        }
        let mse = se / n.max(1) as f64;
        if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (1023.0f64 * 1023.0 / mse).log10()
        }
    };
    let cw = reference.width.div_ceil(2);
    [
        plane(&dec.y, &reference.y, reference.width, 1),
        plane(&dec.u, &reference.u, cw, 2),
        plane(&dec.v, &reference.v, cw, 2),
    ]
}

/// Blocks that moved in a lumen-bench run (changed_blocks.bin).
#[derive(Clone)]
struct ChangedMask {
    cols: usize,
    moved: std::sync::Arc<Vec<bool>>,
}

impl ChangedMask {
    fn moved(&self, x: usize, y: usize) -> bool {
        self.moved
            .get((y / 32) * self.cols + x / 32)
            .copied()
            .unwrap_or(false)
    }
}

fn load_changed_mask(dir: &Path) -> Result<ChangedMask> {
    let meta: Meta = serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json"))?)?;
    let raw = std::fs::read(dir.join("changed_blocks.bin")).context("changed_blocks.bin")?;
    let v: Vec<u32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let (frames, counts) = v.split_first().context("empty changed_blocks.bin")?;
    let cols = (meta.width as usize).div_ceil(32);
    anyhow::ensure!(
        counts.len() == cols * (meta.height as usize).div_ceil(32),
        "changed_blocks.bin does not match the frame size"
    );
    let moved: Vec<bool> = counts.iter().map(|&n| n > 0 && n * 10 >= *frames).collect();
    println!(
        "changed mask: {} of {} blocks moved (>= 10 % of {frames} frames)",
        moved.iter().filter(|&&m| m).count(),
        moved.len()
    );
    Ok(ChangedMask {
        cols,
        moved: std::sync::Arc::new(moved),
    })
}

/// Columns `x0..x1` of a BGRA image and of its reconstruction.
fn crop_columns(
    src: &[u8],
    rec: &[[f32; 3]],
    w: usize,
    x0: usize,
    x1: usize,
) -> (Vec<u8>, Vec<[f32; 3]>) {
    let (mut s, mut r) = (Vec::new(), Vec::new());
    for (row_s, row_r) in src.chunks_exact(w * 4).zip(rec.chunks_exact(w)) {
        s.extend_from_slice(&row_s[x0 * 4..x1 * 4]);
        r.extend_from_slice(&row_r[x0..x1]);
    }
    (s, r)
}

fn capture_mode(
    dir: &Path,
    decoded: Option<PathBuf>,
    heatmaps: bool,
    split_x: Option<usize>,
    region: Option<[usize; 4]>,
    sparse: bool,
    mask: Option<ChangedMask>,
) -> Result<()> {
    let meta: Meta = serde_json::from_str(
        &std::fs::read_to_string(dir.join("meta.json")).context("meta.json")?,
    )?;
    let (w, h) = (meta.width as usize, meta.height as usize);
    let decoded_path = decoded.unwrap_or_else(|| dir.join("decoded.yuv"));
    let mut decoded_file = File::open(&decoded_path).ok();
    if decoded_file.is_none() {
        println!(
            "note: {} not found — only the 4:2:0 reference will be analysed",
            decoded_path.display()
        );
    }

    let mut results = Vec::new();
    for (pos, &idx) in meta.dumped_frames.iter().enumerate() {
        let src = std::fs::read(dir.join(format!("src_{idx:06}.bgra")))?;
        anyhow::ensure!(src.len() == w * h * 4, "src_{idx:06}.bgra has wrong size");
        println!("frame {idx}:");

        let reference = bgra8_to_yuv420p10(&src, w, h);
        let (r420, _) = compare(&src, &yuv420p10_to_rgb(&reference), w, h);
        print_report("A. 4:2:0 only (reference)", &r420);

        let mut dec_report = None;
        if let Some(f) = decoded_file.as_mut() {
            let at = if sparse { pos as u32 } else { idx };
            match read_decoded_frame(f, at, w, h)? {
                Some(dec) => {
                    let rec = yuv420p10_to_rgb(&dec);
                    let (r, map) = compare(&src, &rec, w, h);
                    print_report("B. 4:2:0 + codec", &r);
                    type Pick = Box<dyn Fn(usize, usize) -> bool>;
                    let mut parts: Vec<(&str, Pick)> = vec![("all", Box::new(|_, _| true))];
                    if let Some(s) = split_x.filter(|&x| x > 0 && x < w) {
                        parts.push(("left", Box::new(move |x, _| x < s)));
                        parts.push(("right", Box::new(move |x, _| x >= s)));
                    }
                    if let Some([x0, y0, x1, y1]) = region {
                        let inside = move |x, y| (x0..x1).contains(&x) && (y0..y1).contains(&y);
                        parts.push(("in", Box::new(inside)));
                        parts.push(("out", Box::new(move |x, y| !inside(x, y))));
                    }
                    if let Some(m) = mask.clone() {
                        let m2 = m.clone();
                        parts.push(("moved", Box::new(move |x, y| m.moved(x, y))));
                        parts.push(("still", Box::new(move |x, y| !m2.moved(x, y))));
                    }
                    for (label, pick) in parts {
                        let [py, pu, pv] = codec_psnr(&dec, &reference, &pick);
                        println!(
                            "  D. codec only, {label:<5}       PSNR Y {py:6.2}  U {pu:6.2}  V {pv:6.2} dB (decoded vs 4:2:0 reference)"
                        );
                        if label != "all" {
                            let mut de: Vec<f32> = map
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| pick(i % w, i / w))
                                .map(|(_, &d)| d)
                                .collect();
                            de.sort_by(f32::total_cmp);
                            if !de.is_empty() {
                                let mean =
                                    de.iter().map(|&d| f64::from(d)).sum::<f64>() / de.len() as f64;
                                let p = |q: f64| de[((de.len() - 1) as f64 * q).round() as usize];
                                let over1 = de.iter().filter(|&&d| d > 1.0).count() as f64
                                    / de.len() as f64;
                                println!(
                                    "  E. 4:2:0 + codec, {label:<5}    ΔE00 mean {mean:5.3} p95 {:5.2} p99 {:5.2} | >1: {:6.3}%",
                                    p(0.95),
                                    p(0.99),
                                    over1 * 100.0
                                );
                            }
                        }
                    }
                    if let Some(x) = split_x.filter(|&x| x > 0 && x < w) {
                        for (label, x0, x1) in
                            [("B. left of split", 0, x), ("B. right of split", x, w)]
                        {
                            let (s, r) = crop_columns(&src, &rec, w, x0, x1);
                            print_report(label, &compare(&s, &r, x1 - x0, h).0);
                        }
                    }
                    if heatmaps {
                        write_png_rgb(
                            &dir.join(format!("de_{idx:06}.png")),
                            &delta_e_heatmap_rgb8(&map),
                            w,
                            h,
                        )?;
                        write_png_rgb(
                            &dir.join(format!("decoded_{idx:06}.png")),
                            &bgra_to_rgb(&rgb_to_bgra8(&rec)),
                            w,
                            h,
                        )?;
                    }
                    dec_report = Some(r);
                }
                None => println!("  B. frame {idx} not present in decoded stream"),
            }
        }

        let mut check = None;
        if meta.verify {
            let gpu_bytes = std::fs::read(dir.join(format!("gpu_{idx:06}.yuv")))?;
            let gpu = Yuv420p10::from_yuv420p10le(&gpu_bytes, w, h).context("gpu dump size")?;
            let c = shader_check(&gpu, &reference);
            println!(
                "  C. GPU shader vs reference: {} of {} samples differ ({:.4}%), max diff {} code(s)",
                c.mismatched,
                c.samples,
                100.0 * c.mismatched as f64 / c.samples as f64,
                c.max_abs_diff
            );
            check = Some(c);
        }
        results.push(FrameResult {
            frame: idx,
            chroma_420_only: r420,
            decoded: dec_report,
            gpu_vs_reference: check,
        });
    }

    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    println!("\nWrote {}", dir.join("report.json").display());
    Ok(())
}

fn barcode_mode(dir: &Path, decoded: Option<PathBuf>) -> Result<()> {
    let meta: Meta = serde_json::from_str(
        &std::fs::read_to_string(dir.join("meta.json")).context("meta.json")?,
    )?;
    let (w, h) = (meta.width as usize, meta.height as usize);
    let path = decoded.unwrap_or_else(|| dir.join("decoded.yuv"));
    let mut f = BufReader::new(File::open(&path).with_context(|| path.display().to_string())?);
    let frame_bytes = Yuv420p10::frame_size_yuv420p10le(w, h);
    let luma_bytes = w * h * 2;
    let mut buf = vec![0u8; frame_bytes];
    let (mut ok, mut failed, mut idx) = (0u32, 0u32, 0u32);
    loop {
        match f.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let luma: Vec<u16> = buf[..luma_bytes]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        match capture::barcode::read_from_luma(&luma, w) {
            Some(n) if n == idx => ok += 1,
            other => {
                failed += 1;
                if failed <= 10 {
                    println!("frame {idx}: barcode read {other:?}");
                }
            }
        }
        idx += 1;
    }
    println!(
        "{idx} decoded frames: {ok} ok, {failed} failed (meta.json says {} encoded)",
        meta.frames
    );
    if failed > 0 || idx != meta.frames {
        bail!("barcode check failed");
    }
    Ok(())
}
