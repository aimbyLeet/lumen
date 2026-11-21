use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use color::subsample::{Yuv420p10, yuv420p10_to_rgb10};
use decoder::{Decoder, annexb, frame_hash, read_frame};
use gpu_convert::{Yuv420ToRgb10, create_rgb10_target, read_rgb10, texture_srv};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

#[derive(Parser)]
#[command(about = "Decode an HEVC Main10 file on NVDEC and verify it against a reference")]
struct Args {
    /// Annex B HEVC stream.
    #[arg(long, default_value = "bench-out/stream.hevc")]
    input: PathBuf,
    /// yuv420p10le reference (e.g. from FFmpeg); omit to only measure.
    #[arg(long)]
    reference: Option<PathBuf>,
    /// Stop after this many frames.
    #[arg(long)]
    frames: Option<usize>,
    /// Every N-th frame: run the client display shader (4:2:0 -> R10G10B10A2)
    /// and compare it with the `color` reference. 0 = off.
    #[arg(long, default_value_t = 0)]
    rgb_every: usize,
    /// Directory with `src_NNNNNN.bgra` captures (lumen-bench dumps): adds ΔE00
    /// of the displayed picture against the original desktop.
    #[arg(long)]
    source_dir: Option<PathBuf>,
    /// Write "<frame> <FNV-1a hash>" per decoded frame (compare streams that
    /// should decode to the same pictures, e.g. with lumen-client --frame-hashes).
    #[arg(long)]
    hashes: Option<PathBuf>,
}

const NVIDIA_VENDOR_ID: u32 = 0x10DE;

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext, String)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let desc = adapter.GetDesc1()?;
            if desc.VendorId != NVIDIA_VENDOR_ID {
                continue;
            }
            let name = String::from_utf16_lossy(&desc.Description);
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &adapter.cast::<IDXGIAdapter>()?,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_1]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
            let name = name.trim_end_matches('\0').to_string();
            return Ok((device.context("device")?, ctx.context("context")?, name));
        }
    }
    bail!("no NVIDIA adapter found")
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub fn main() -> Result<()> {
    let args = Args::parse();
    let data = std::fs::read(&args.input).with_context(|| format!("read {:?}", args.input))?;
    let mut aus = annexb::split_access_units(&data);
    if let Some(n) = args.frames {
        aus.truncate(n);
    }
    ensure!(!aus.is_empty(), "no access units in {:?}", args.input);

    let (device, ctx, adapter) = create_device()?;
    let mut dec = Decoder::new(&device)?;
    println!("adapter: {adapter} (CUDA: {})", dec.device_name());
    println!(
        "input:   {:?}, {} bytes, {} access units",
        args.input,
        data.len(),
        aus.len()
    );

    let mut verifier = Verifier::new(&args, &device, &ctx)?;
    let (latency_ms, late) = decode_all(&mut dec, &aus, &mut verifier)?;
    print_report(&args, latency_ms, late, &verifier)?;
    verifier.finish(aus.len())
}

/// What is checked on each decoded frame: the reference file, the hashes
/// file and the display-shader comparison.
struct Verifier<'a> {
    args: &'a Args,
    device: &'a ID3D11Device,
    ctx: &'a ID3D11DeviceContext,
    // Declared in the order the locals of `main` used to be dropped.
    hashes: Option<BufWriter<File>>,
    rgb: Option<RgbCheck>,
    ref_buf: Vec<u8>,
    reference: Option<BufReader<File>>,
    frames: usize,
    mismatched: usize,
    max_diff: u16,
    size: (u32, u32),
}

impl<'a> Verifier<'a> {
    fn new(args: &'a Args, device: &'a ID3D11Device, ctx: &'a ID3D11DeviceContext) -> Result<Self> {
        let reference = match &args.reference {
            Some(p) => Some(BufReader::with_capacity(
                1 << 24,
                File::open(p).with_context(|| format!("open {p:?}"))?,
            )),
            None => None,
        };
        let rgb = match args.rgb_every {
            0 => None,
            _ => Some(RgbCheck::new(device)?),
        };
        let hashes = match &args.hashes {
            Some(p) => Some(BufWriter::new(
                File::create(p).with_context(|| format!("create {p:?}"))?,
            )),
            None => None,
        };
        Ok(Self {
            args,
            device,
            ctx,
            hashes,
            rgb,
            ref_buf: Vec::new(),
            reference,
            frames: 0,
            mismatched: 0,
            max_diff: 0,
            size: (0, 0),
        })
    }

    fn check(&mut self, frame: &decoder::Frame) -> Result<()> {
        let index = self.frames;
        self.frames += 1;
        self.size = (frame.width, frame.height);
        let rgb_due = self.rgb.is_some() && index.is_multiple_of(self.args.rgb_every);
        if self.reference.is_none() && !rgb_due && self.hashes.is_none() {
            return Ok(());
        }
        let got = read_frame(self.device, self.ctx, frame)?;
        if let Some(h) = self.hashes.as_mut() {
            writeln!(h, "{index} {:016x}", frame_hash(&got))?;
        }
        if let Some(r) = self.reference.as_mut() {
            let n = got.y.len() + got.u.len() + got.v.len();
            self.ref_buf.resize(n * 2, 0);
            r.read_exact(&mut self.ref_buf)
                .context("reference is shorter than the stream")?;
            let ours = got.y.iter().chain(&got.u).chain(&got.v);
            let theirs = self
                .ref_buf
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b));
            let diff = ours
                .zip(theirs)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or(0);
            if diff != 0 {
                self.mismatched += 1;
                if self.mismatched <= 5 {
                    eprintln!("frame {index}: max diff {diff}");
                }
            }
            self.max_diff = self.max_diff.max(diff);
        }
        if let Some(rc) = self.rgb.as_mut().filter(|_| rgb_due) {
            let source = self
                .args
                .source_dir
                .as_ref()
                .map(|d| d.join(format!("src_{index:06}.bgra")))
                .filter(|p| p.exists());
            rc.check(self.device, self.ctx, frame, &got, index, source)?;
        }
        Ok(())
    }

    /// The checks that end the run: the shader report, then the frame count.
    fn finish(&self, au_count: usize) -> Result<()> {
        if let Some(rc) = &self.rgb {
            rc.report()?;
        }
        ensure!(
            self.frames == au_count,
            "{} AUs but {} frames",
            au_count,
            self.frames
        );
        Ok(())
    }
}

/// Feeds every access unit to the decoder, flushes it and checks each picture
/// that comes out. Returns the submit -> texture latency per AU and the number
/// of AUs whose picture did not come out of the same `decode()` call.
fn decode_all(
    dec: &mut Decoder,
    aus: &[&[u8]],
    verifier: &mut Verifier,
) -> Result<(Vec<f64>, usize)> {
    let mut latency_ms = Vec::with_capacity(aus.len());
    let mut late = 0usize; // AUs whose picture did not come out of the same decode() call
    for (i, au) in aus.iter().enumerate() {
        let t = Instant::now();
        dec.decode(au, i as i64)?;
        let first = dec.next_frame();
        latency_ms.push(t.elapsed().as_secs_f64() * 1e3);
        match first {
            Some(f) => {
                ensure!(
                    f.timestamp == i as i64,
                    "AU {i} produced picture {}",
                    f.timestamp
                );
                verifier.check(&f)?;
            }
            None => late += 1,
        }
        while let Some(f) = dec.next_frame() {
            verifier.check(&f)?;
        }
    }
    dec.flush()?;
    while let Some(f) = dec.next_frame() {
        verifier.check(&f)?;
    }
    Ok((latency_ms, late))
}

/// Prints the latency summary and the reference verdict; fails when the
/// decoded output differs from the reference.
fn print_report(
    args: &Args,
    mut latency_ms: Vec<f64>,
    late: usize,
    verifier: &Verifier,
) -> Result<()> {
    let (frames, size) = (verifier.frames, verifier.size);
    // The first AU also creates the decoder; report it separately.
    let first_ms = latency_ms[0];
    let mut latency_ms = latency_ms.split_off(1.min(latency_ms.len() - 1));
    latency_ms.sort_by(f64::total_cmp);
    println!("output:  {}x{}, {frames} frames", size.0, size.1);
    println!("first AU (incl. decoder creation): {first_ms:.2} ms");
    println!(
        "decode latency per AU (submit -> D3D11 texture), ms: p50 {:.2}  p95 {:.2}  p99 {:.2}  max {:.2}",
        percentile(&latency_ms, 0.50),
        percentile(&latency_ms, 0.95),
        percentile(&latency_ms, 0.99),
        latency_ms[latency_ms.len() - 1]
    );
    println!("AUs without immediate picture: {late}");
    if args.reference.is_some() {
        let (mismatched, max_diff) = (verifier.mismatched, verifier.max_diff);
        println!("reference: {mismatched} mismatched frames, max diff {max_diff} codes");
        ensure!(mismatched == 0, "decoded output differs from the reference");
        println!("BIT-EXACT");
    }
    Ok(())
}

/// Client display shader vs the `color` reference, on offscreen R10G10B10A2.
struct RgbCheck {
    shader: Yuv420ToRgb10,
    target: Option<(ID3D11Texture2D, ID3D11RenderTargetView, u32, u32)>,
    frames: usize,
    pixels: u64,
    off_by_one: u64,
    worse: u64,
    max_diff: u16,
}

impl RgbCheck {
    fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            shader: Yuv420ToRgb10::new(device)?,
            target: None,
            frames: 0,
            pixels: 0,
            off_by_one: 0,
            worse: 0,
            max_diff: 0,
        })
    }

    fn check(
        &mut self,
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        frame: &decoder::Frame,
        yuv: &Yuv420p10,
        index: usize,
        source: Option<PathBuf>,
    ) -> Result<()> {
        let (w, h) = (frame.width, frame.height);
        if !matches!(self.target, Some((_, _, tw, th)) if (tw, th) == (w, h)) {
            let (t, v) = create_rgb10_target(device, w, h)?;
            self.target = Some((t, v, w, h));
        }
        let (tex, rtv, _, _) = self.target.as_ref().expect("target");
        let luma = texture_srv(device, &frame.luma)?;
        let chroma = texture_srv(device, &frame.chroma)?;
        self.shader.draw(ctx, &luma, &chroma, rtv, w, h);
        let gpu = read_rgb10(device, ctx, tex)?;
        let cpu = yuv420p10_to_rgb10(yuv);

        let (mut off1, mut worse, mut max) = (0u64, 0u64, 0u16);
        for (a, b) in gpu.iter().zip(&cpu) {
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            match d {
                0 => {}
                1 => off1 += 1,
                _ => worse += 1,
            }
            max = max.max(d);
        }
        self.frames += 1;
        self.pixels += gpu.len() as u64;
        self.off_by_one += off1;
        self.worse += worse;
        self.max_diff = self.max_diff.max(max);
        print!("rgb frame {index}: max diff {max}, {off1} px off by 1");

        if let Some(path) = source {
            let src = std::fs::read(&path).with_context(|| format!("read {path:?}"))?;
            let as_f32 = |v: &[[u16; 3]]| -> Vec<[f32; 3]> {
                v.iter().map(|p| p.map(|c| f32::from(c) / 1023.0)).collect()
            };
            let (w, h) = (w as usize, h as usize);
            let (g, _) = color::metrics::compare(&src, &as_f32(&gpu), w, h);
            let (c, _) = color::metrics::compare(&src, &as_f32(&cpu), w, h);
            print!(
                "; dE00 vs desktop: GPU mean {:.4} p99 {:.3} max {:.3} | reference mean {:.4} p99 {:.3} max {:.3}",
                g.delta_e.mean,
                g.delta_e.p99,
                g.delta_e.max,
                c.delta_e.mean,
                c.delta_e.p99,
                c.delta_e.max
            );
        }
        println!();
        Ok(())
    }

    fn report(&self) -> Result<()> {
        println!(
            "rgb check: {} frames, {} px: {} off by 1 ({:.4} %), {} off by more, max diff {}",
            self.frames,
            self.pixels,
            self.off_by_one,
            self.off_by_one as f64 * 100.0 / self.pixels.max(1) as f64,
            self.worse,
            self.max_diff
        );
        ensure!(
            self.worse == 0,
            "display shader differs from the reference by more than 1 code"
        );
        Ok(())
    }
}
