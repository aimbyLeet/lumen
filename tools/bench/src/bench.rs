use anyhow::{Context, Result, bail};
use capture::qpmap::{QpMapParams, StaticQpMap};
use capture::regions::Changes;
use capture::{Acquired, DesktopDuplicator, GpuDevice, Scene, SyntheticSource};
use clap::{Parser, ValueEnum};
use gpu_convert::{BgraToP010, raw, read_p010, wait_gpu};
use nvenc::{Codec, EncodedFrame, Encoder, EncoderConfig, InputHandle, IvfWriter, RateControl};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use transport::CapController;

#[derive(Copy, Clone, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum CodecArg {
    Hevc,
    Av1,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum RcArg {
    /// Constant QP (use --qp). Best for quality studies.
    Cqp,
    /// Constant bitrate (use --mbps).
    Cbr,
    /// Capped VBR (use --mbps as average, --max-mbps as cap).
    Vbr,
    /// QP never below --qp, bitrate never above --max-mbps.
    Cq,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum SourceArg {
    /// DXGI Desktop Duplication of --adapter/--output.
    Desktop,
    /// GPU-rendered synthetic test frames with a frame-number barcode.
    Synthetic,
}

fn parse_range(s: &str) -> Result<(u32, u32), String> {
    let (a, b) = s.split_once("..").ok_or("expected FROM..TO")?;
    Ok((
        a.parse().map_err(|e| format!("from: {e}"))?,
        b.parse().map_err(|e| format!("to: {e}"))?,
    ))
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH")?;
    Ok((
        w.parse().map_err(|e| format!("width: {e}"))?,
        h.parse().map_err(|e| format!("height: {e}"))?,
    ))
}

#[derive(Parser, Debug)]
#[command(
    name = "lumen-bench",
    about = "Capture → P010 → NVENC quality/latency bench"
)]
struct Args {
    /// List adapters and monitors and exit.
    #[arg(long)]
    list: bool,
    /// Frame source.
    #[arg(long, value_enum, default_value_t = SourceArg::Desktop)]
    source: SourceArg,
    /// Frame size for --source synthetic (even numbers).
    #[arg(long, value_parser = parse_size, default_value = "3840x2160")]
    size: (u32, u32),
    #[arg(long, default_value_t = 0)]
    adapter: u32,
    #[arg(long, default_value_t = 0)]
    output: u32,
    #[arg(long, value_enum, default_value_t = CodecArg::Hevc)]
    codec: CodecArg,
    #[arg(long, value_enum, default_value_t = RcArg::Cqp)]
    rc: RcArg,
    /// QP for --rc cqp (HEVC 0..51; lower = better). 10-bit: ~14 is near-transparent.
    #[arg(long, default_value_t = 16)]
    qp: u32,
    #[arg(long, default_value_t = 150.0)]
    mbps: f64,
    #[arg(long, default_value_t = 250.0)]
    max_mbps: f64,
    /// --rc cq: rate-control buffer in frames at the cap.
    #[arg(long, default_value_t = 3)]
    vbv_frames: u32,
    /// --rc cq: steer the encoder to the real cap like lumen-host does.
    #[arg(long)]
    cap_control: bool,
    /// Decoded picture buffer size (0 = driver default).
    #[arg(long, default_value_t = 16)]
    ref_frames: u32,
    /// Also write stream_dropped.* without frames FROM..TO (exclusive end) — a
    /// lost-reference test for loss recovery (see --invalidate-at).
    #[arg(long, value_parser = parse_range)]
    drop: Option<(u32, u32)>,
    /// Before encoding this frame, invalidate the --drop frames as references.
    #[arg(long)]
    invalidate_at: Option<u32>,
    /// NVENC preset 1..7.
    #[arg(long, default_value_t = 4)]
    preset: u32,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    #[arg(long, default_value_t = 10.0)]
    seconds: f64,
    /// Dump source frame (and GPU P010 with --verify) every N frames. 0 = only frame 0.
    #[arg(long, default_value_t = 120)]
    dump_every: u32,
    /// Write no frame dumps (not even frame 0) and no stream: a run on a real
    /// desktop that should leave no picture of it on disk.
    #[arg(long)]
    no_dumps: bool,
    /// Also read back the GPU converter output to check it against the CPU reference.
    #[arg(long)]
    verify: bool,
    #[arg(long, default_value = "bench-out")]
    out_dir: PathBuf,
    /// QP delta map: LEFT,RIGHT — deltas for the left and the right half of the
    /// frame (by map block columns), e.g. -6,0.
    #[arg(long, value_parser = parse_pair, allow_hyphen_values = true)]
    qp_map: Option<(i8, i8)>,
    /// QP map from the changed regions (capture::qpmap): --qp is the QP still
    /// blocks may reach, moving ones are coded at --moving-qp while the cap
    /// allows.
    #[arg(long, conflicts_with = "qp_map")]
    static_qp_map: bool,
    /// --static-qp-map: QP of moving blocks.
    #[arg(long, default_value_t = 16)]
    moving_qp: u8,
    /// --static-qp-map: a block unchanged this many frames is still (default:
    /// half a second).
    #[arg(long)]
    still_after: Option<u16>,
    /// --source synthetic: what it shows (see capture::Scene).
    #[arg(long, value_enum, default_value_t = SceneArg::Moving)]
    scene: SceneArg,
    /// The still picture of --scene still / mixed.
    #[arg(long, default_value_t = 100)]
    still_frame: u32,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum SceneArg {
    /// Everything moves.
    Moving,
    /// One frame, again and again.
    Still,
    /// A still picture with a moving window of a quarter of it.
    Mixed,
}

fn parse_pair(s: &str) -> Result<(i8, i8), String> {
    let (a, b) = s.split_once(',').ok_or("expected LEFT,RIGHT")?;
    Ok((
        a.trim().parse().map_err(|e| format!("left: {e}"))?,
        b.trim().parse().map_err(|e| format!("right: {e}"))?,
    ))
}

#[derive(Serialize)]
struct Meta {
    width: u32,
    height: u32,
    fps: u32,
    codec: CodecArg,
    rate_control: String,
    preset: u32,
    adapter: String,
    frames: u32,
    stream: String,
    dumped_frames: Vec<u32>,
    verify: bool,
}

#[derive(Default)]
struct Stats {
    values: Vec<f64>,
}
impl Stats {
    fn push(&mut self, v: f64) {
        self.values.push(v);
    }
    fn summary(&self) -> String {
        if self.values.is_empty() {
            return "-".into();
        }
        let mut v = self.values.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        format!(
            "mean {:6.2}  p50 {:6.2}  p95 {:6.2}  p99 {:6.2}  max {:6.2}",
            mean,
            p(0.5),
            p(0.95),
            p(0.99),
            v[v.len() - 1]
        )
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub fn main() -> Result<()> {
    let args = Args::parse();
    capture::init_process();

    if args.list {
        return list_outputs();
    }

    std::fs::create_dir_all(&args.out_dir)?;
    let (mut dup, mut synth, gpu) = open_source(&args)?;
    let (w, h) = match (&dup, &synth) {
        (Some(d), _) => (d.width, d.height),
        (_, Some(s)) => (s.width, s.height),
        _ => unreachable!(),
    };
    println!(
        "{} {w}x{h} @ {} fps on {}",
        if synth.is_some() {
            "Synthetic source"
        } else {
            "Capturing"
        },
        args.fps,
        gpu.adapter_name
    );

    // The texture types are not nameable here (no `windows` dependency), so
    // the frame texture and its view stay in `main`.
    let frame_tex = match (&dup, &synth) {
        (Some(d), _) => d.create_frame_texture()?,
        (_, Some(s)) => s.create_frame_texture()?,
        _ => unreachable!(),
    };
    let converter = BgraToP010::new(&gpu.device, w, h)?;
    let src_srv = converter.source_view(&frame_tex)?;

    let rc = rate_control(&args);
    let codec = match args.codec {
        CodecArg::Hevc => Codec::Hevc,
        CodecArg::Av1 => Codec::Av1,
    };
    let (mut encoder, input) = open_encoder(&args, &gpu, &converter, codec, rc, (w, h))?;

    let stream_name = match codec {
        Codec::Hevc => "stream.hevc",
        Codec::Av1 => "stream.ivf",
    };
    let mut out = open_outputs(&args, codec, stream_name, (w, h))?;

    let mut cap_control = make_cap_controller(&args);
    let total_frames = (args.seconds * args.fps as f64).round() as u32;
    let blocks = (w.div_ceil(32) * h.div_ceil(32)) as usize;
    let mut m = Measured::new(blocks);
    let mut static_map = make_static_map(&args, &encoder, (w, h));
    let mut block_mask = vec![false; blocks];
    let mut have_frame = false;
    let mut bitstream = Vec::with_capacity(4 << 20);

    let mut ticker = Ticker::start(args.fps);
    let mut frame = 0u32;
    while frame < total_frames {
        // 1. Capture: wait for a desktop update until the next tick.
        let t_cap = Instant::now();
        let mut new_frame = true;
        if let Some(dup) = dup.as_mut() {
            let remaining = ticker.next_tick.saturating_duration_since(t_cap);
            let acquired = dup.acquire_into(remaining.as_millis() as u32, &frame_tex)?;
            new_frame = matches!(acquired, Acquired::NewFrame { .. });
            have_frame |= new_frame;
        }
        if dup.is_some() && !have_frame {
            // Nothing captured yet (static desktop at start): force a redraw-free first grab.
            if t_cap.duration_since(ticker.t0) > Duration::from_secs(2) {
                bail!("no desktop frame within 2 s — move a window or the mouse");
            }
            continue;
        }
        let t_tick = ticker.wait();
        if let Some(synth) = synth.as_mut() {
            // Synthetic frames are produced exactly at the requested rate.
            let t_render = Instant::now();
            synth.render_into(frame, &frame_tex)?;
            wait_gpu(&gpu.device, &gpu.context)?;
            m.s_render.push(ms(t_render.elapsed()));
        }
        // What changed since the last encoded frame, in 32-px blocks.
        let changes: Changes = match (dup.as_mut(), synth.as_mut()) {
            (Some(d), _) => d.take_changes(),
            (_, Some(s)) => s.take_changes(),
            _ => unreachable!(),
        };
        let changed_blocks = changes.mark_blocks(w, h, 32, &mut block_mask);
        m.count_changes(frame, args.fps, &block_mask);
        let (moving_blocks, map_delta) =
            update_qp_map(&mut static_map, &mut encoder, &changes, &mut m.s_map);
        m.s_changed
            .push(100.0 * changed_blocks as f64 / block_mask.len() as f64);

        // 2. Convert BGRA → P010 on the GPU. The measured time also covers the
        //    GPU copy out of the duplication surface queued by acquire_into().
        let t_conv = Instant::now();
        converter.convert(&gpu.context, &src_srv);
        wait_gpu(&gpu.device, &gpu.context)?;
        let conv_ms = ms(t_conv.elapsed());

        // Optional dumps (outside timed region).
        if dump_due(&args, frame) {
            let bgra = capture::read_bgra(&gpu, &frame_tex)?;
            dump_frame(&args, &gpu, &converter, frame, &bgra, &mut m.dumped)?;
        }

        // 3. Encode.
        let (info, enc_ms) =
            encode_frame(&args, &mut encoder, input, frame, &mut bitstream, &mut out)?;
        m.s_kb.push(info.size as f64 / 1024.0);
        steer_cap(&mut cap_control, &mut static_map, &mut encoder, &info)?;

        m.record_frame(new_frame, info.size, conv_ms, enc_ms);
        writeln!(
            out.csv,
            "{frame},{:.3},{},{changed_blocks},{moving_blocks},{map_delta},{conv_ms:.3},{enc_ms:.3},{},{},{},{}",
            ms(t_tick.duration_since(ticker.t0)),
            new_frame as u8,
            info.size,
            info.avg_qp,
            info.is_keyframe as u8,
            cap_control.as_ref().map_or(0, |c| c.nominal_bps())
        )?;
        frame += 1;
    }
    drop(out.sink);
    drop(out.dropped_sink);
    out.csv.flush()?;

    report(&args, &m, frame)?;
    write_meta(
        &args,
        &gpu.adapter_name,
        (w, h),
        rc,
        stream_name,
        frame,
        m.dumped,
    )?;
    print_next_steps(&args, stream_name);
    Ok(())
}

fn list_outputs() -> Result<()> {
    for o in capture::list_outputs()? {
        println!(
            "--adapter {} --output {}   {}x{}   {}   [{}]",
            o.adapter_index, o.output_index, o.width, o.height, o.device_name, o.adapter_name
        );
    }
    Ok(())
}

/// Opens the desktop duplication or the synthetic source and checks that its
/// GPU is an NVIDIA one. Returns `(dup, synth, gpu)`, in the order the old
/// locals of `main` were declared (so they drop in the same order).
fn open_source(
    args: &Args,
) -> Result<(
    Option<DesktopDuplicator>,
    Option<SyntheticSource>,
    GpuDevice,
)> {
    let mut dup = None;
    let mut synth = None;
    let gpu: GpuDevice = match args.source {
        SourceArg::Desktop => {
            let d = DesktopDuplicator::new(args.adapter, args.output)?;
            let g = d.gpu().clone();
            dup = Some(d);
            g
        }
        SourceArg::Synthetic => {
            let g = GpuDevice::open_nvidia()?;
            let mut s = SyntheticSource::new(g.clone(), args.size.0, args.size.1)?;
            s.set_scene(match args.scene {
                SceneArg::Moving => Scene::Moving,
                SceneArg::Still => Scene::Still {
                    frame: args.still_frame,
                },
                SceneArg::Mixed => Scene::Mixed {
                    still_frame: args.still_frame,
                },
            });
            if args.scene == SceneArg::Mixed {
                println!("Moving window: {:?} (x0, y0, x1, y1)", s.mixed_window());
            }
            synth = Some(s);
            g
        }
    };
    if !gpu.is_nvidia {
        bail!(
            "monitor {} is driven by '{}', not an NVIDIA GPU. On Optimus laptops connect the monitor to the dGPU \
             or pick another output (--list).",
            args.output,
            gpu.adapter_name
        );
    }
    Ok((dup, synth, gpu))
}

fn rate_control(args: &Args) -> RateControl {
    match args.rc {
        RcArg::Cqp => RateControl::ConstQp { qp: args.qp },
        RcArg::Cbr => RateControl::Cbr {
            bitrate_bps: (args.mbps * 1e6) as u32,
        },
        RcArg::Vbr => RateControl::Vbr {
            avg_bps: (args.mbps * 1e6) as u32,
            max_bps: (args.max_mbps * 1e6) as u32,
        },
        RcArg::Cq => RateControl::CappedQuality {
            min_qp: args.qp,
            max_bps: (args.max_mbps * 1e6) as u32,
            vbv_frames: args.vbv_frames,
        },
    }
}

/// Creates the encoder, fills the fixed QP map (`--qp-map`), registers the
/// converter output and prints the capability line.
fn open_encoder(
    args: &Args,
    gpu: &GpuDevice,
    converter: &BgraToP010,
    codec: Codec,
    rc: RateControl,
    (w, h): (u32, u32),
) -> Result<(Encoder, InputHandle)> {
    let mut encoder = unsafe {
        Encoder::new(
            raw(&gpu.device),
            EncoderConfig {
                codec,
                width: w,
                height: h,
                fps: args.fps,
                preset: args.preset,
                rate_control: rc,
                ref_frames: args.ref_frames,
                qp_map: args.qp_map.is_some() || args.static_qp_map,
            },
        )
    }
    .context("NVENC init")?;
    if let Some((left, right)) = args.qp_map {
        let (cols, rows) = encoder.qp_map_size();
        let block = encoder.qp_map_block();
        let map = encoder.qp_map_mut().context("qp map")?;
        for r in 0..rows {
            for c in 0..cols {
                map[(r * cols + c) as usize] = if c < cols / 2 { left } else { right };
            }
        }
        println!(
            "QP map {cols}x{rows} blocks of {block} px: left {left:+}, right {right:+} (split at x = {})",
            cols / 2 * block
        );
    }
    let input = unsafe { encoder.register_d3d11_p010(raw(&converter.output)) }
        .context("register P010 texture")?;
    println!(
        "NVENC caps: ref pic invalidation {}",
        encoder.cap(nvenc::sys::NV_ENC_CAPS_SUPPORT_REF_PIC_INVALIDATION)?
    );
    Ok((encoder, input))
}

/// Where the encoded stream goes: a file, or nowhere (`--no-dumps`).
enum Sink {
    Raw(BufWriter<File>),
    Ivf(IvfWriter<BufWriter<File>>),
    Null,
}

impl Sink {
    fn open(codec: Codec, file: BufWriter<File>, (w, h): (u32, u32), fps: u32) -> Result<Sink> {
        Ok(match codec {
            Codec::Hevc => Sink::Raw(file),
            Codec::Av1 => Sink::Ivf(IvfWriter::new(file, w as u16, h as u16, fps)?),
        })
    }

    fn write(&mut self, data: &[u8], frame: u32) -> Result<()> {
        match self {
            Sink::Raw(f) => f.write_all(data)?,
            Sink::Ivf(f) => f.write_frame(data, frame as u64)?,
            Sink::Null => {}
        }
        Ok(())
    }
}

/// The files written while encoding. Field order = the old drop order.
struct Outputs {
    csv: BufWriter<File>,
    dropped_sink: Option<Sink>,
    sink: Sink,
}

/// Creates the stream file(s) and `frames.csv` (with its header line).
fn open_outputs(args: &Args, codec: Codec, stream_name: &str, size: (u32, u32)) -> Result<Outputs> {
    let stream_file = BufWriter::new(File::create(args.out_dir.join(stream_name))?);
    let sink = if args.no_dumps {
        drop(stream_file);
        std::fs::remove_file(args.out_dir.join(stream_name))?;
        Sink::Null
    } else {
        Sink::open(codec, stream_file, size, args.fps)?
    };
    let dropped_sink = match args.drop {
        Some(_) => Some(Sink::open(
            codec,
            BufWriter::new(File::create(
                args.out_dir
                    .join(format!("stream_dropped.{}", &stream_name[7..])),
            )?),
            size,
            args.fps,
        )?),
        None => None,
    };

    let mut csv = BufWriter::new(File::create(args.out_dir.join("frames.csv"))?);
    writeln!(
        csv,
        "frame,t_ms,new_frame,changed_blocks,moving_blocks,map_delta,copy_convert_ms,encode_ms,bytes,avg_qp,keyframe,nominal_bps"
    )?;
    Ok(Outputs {
        csv,
        dropped_sink,
        sink,
    })
}

fn make_cap_controller(args: &Args) -> Option<CapController> {
    (args.cap_control && matches!(args.rc, RcArg::Cq)).then(|| {
        // A fixed map: the controller expects the floor plus the larger
        // delta. The static map: it sees the encoder's base QP (below) and
        // expects the floor.
        let delta = args.qp_map.map_or(0, |(l, r)| l.max(r).max(0) as u32);
        CapController::new(
            (args.max_mbps * 1e6) as u32,
            args.qp + delta,
            Instant::now(),
        )
    })
}

fn make_static_map(args: &Args, encoder: &Encoder, (w, h): (u32, u32)) -> Option<StaticQpMap> {
    args.static_qp_map.then(|| {
        let params = QpMapParams {
            still_qp: args.qp as u8,
            moving_qp: args.moving_qp,
            still_after: args.still_after.unwrap_or((args.fps / 2) as u16),
        };
        println!(
            "Static QP map: still blocks down to QP {}, moving at {} (still after {} frames)",
            params.still_qp, params.moving_qp, params.still_after
        );
        StaticQpMap::new(w, h, encoder.qp_map_block(), params)
    })
}

/// The frame clock: one tick per frame interval, no bursts after a stall.
struct Ticker {
    t0: Instant,
    next_tick: Instant,
    interval: Duration,
}

impl Ticker {
    fn start(fps: u32) -> Self {
        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let t0 = Instant::now();
        Self {
            t0,
            next_tick: t0,
            interval,
        }
    }

    /// Sleeps until the next tick, moves it on and returns the time it was reached.
    fn wait(&mut self) -> Instant {
        if Instant::now() < self.next_tick {
            std::thread::sleep(self.next_tick - Instant::now());
        }
        let t_tick = Instant::now();
        self.next_tick += self.interval;
        if self.next_tick < t_tick {
            self.next_tick = t_tick + self.interval; // we fell behind; don't burst
        }
        t_tick
    }
}

/// Everything the run measures, for the report and `meta.json`.
struct Measured {
    s_conv: Stats,
    s_enc: Stats,
    s_total: Stats,
    s_render: Stats,
    s_kb: Stats,
    s_changed: Stats,
    s_map: Stats,
    bytes_changed: u64,
    n_changed: u32,
    bytes_static: u64,
    n_static: u32,
    dumped: Vec<u32>,
    change_counts: Vec<u32>,
    counted_frames: u32,
}

impl Measured {
    fn new(blocks: usize) -> Self {
        Self {
            s_conv: Stats::default(),
            s_enc: Stats::default(),
            s_total: Stats::default(),
            s_render: Stats::default(),
            s_kb: Stats::default(),
            s_changed: Stats::default(),
            s_map: Stats::default(),
            bytes_changed: 0,
            n_changed: 0,
            bytes_static: 0,
            n_static: 0,
            dumped: Vec::new(),
            change_counts: vec![0u32; blocks],
            counted_frames: 0,
        }
    }

    /// How often each block changed after the first second (the first
    /// frame is new throughout): changed_blocks.bin, for lumen-compare.
    fn count_changes(&mut self, frame: u32, fps: u32, block_mask: &[bool]) {
        if frame >= fps {
            for (n, &m) in self.change_counts.iter_mut().zip(block_mask) {
                *n += u32::from(m);
            }
            self.counted_frames += 1;
        }
    }

    fn record_frame(&mut self, new_frame: bool, size: usize, conv_ms: f64, enc_ms: f64) {
        if new_frame {
            self.bytes_changed += size as u64;
            self.n_changed += 1;
        } else {
            self.bytes_static += size as u64;
            self.n_static += 1;
        }
        self.s_conv.push(conv_ms);
        self.s_enc.push(enc_ms);
        self.s_total.push(conv_ms + enc_ms);
    }
}

/// Feeds the changed regions to the static QP map (if any) and returns the
/// numbers for the CSV: moving blocks and the map delta.
fn update_qp_map(
    static_map: &mut Option<StaticQpMap>,
    encoder: &mut Encoder,
    changes: &Changes,
    s_map: &mut Stats,
) -> (usize, i8) {
    match (static_map.as_mut(), encoder.qp_map_mut()) {
        (Some(m), Some(out)) => {
            let t_map = Instant::now();
            let s = m.update(changes, out);
            s_map.push(ms(t_map.elapsed()));
            (s.moving, s.delta)
        }
        _ => (0, 0),
    }
}

fn dump_due(args: &Args, frame: u32) -> bool {
    !args.no_dumps && (frame == 0 || (args.dump_every > 0 && frame.is_multiple_of(args.dump_every)))
}

/// Writes the source frame (`bgra`) and, with `--verify`, the GPU P010 output.
fn dump_frame(
    args: &Args,
    gpu: &GpuDevice,
    converter: &BgraToP010,
    frame: u32,
    bgra: &[u8],
    dumped: &mut Vec<u32>,
) -> Result<()> {
    std::fs::write(args.out_dir.join(format!("src_{frame:06}.bgra")), bgra)?;
    if args.verify {
        let p = read_p010(&gpu.device, &gpu.context, &converter.output)?;
        let mut out = Vec::with_capacity(2 * (p.y.len() + p.u.len() * 2));
        for plane in [&p.y, &p.u, &p.v] {
            for v in plane.iter() {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write(args.out_dir.join(format!("gpu_{frame:06}.yuv")), out)?;
    }
    dumped.push(frame);
    Ok(())
}

/// Encodes one frame (invalidating the `--drop` references first when it is
/// `--invalidate-at`) and writes it to the sinks. Returns the encoder's info
/// and the time of the encode call.
fn encode_frame(
    args: &Args,
    encoder: &mut Encoder,
    input: InputHandle,
    frame: u32,
    bitstream: &mut Vec<u8>,
    out: &mut Outputs,
) -> Result<(EncodedFrame, f64)> {
    let t_enc = Instant::now();
    bitstream.clear();
    if let (Some(at), Some((from, to))) = (args.invalidate_at, args.drop)
        && at == frame
    {
        for lost in from..to {
            encoder.invalidate_ref_frame(lost as u64)?;
        }
    }
    let info = encoder.encode(input, frame as u64, frame == 0, bitstream)?;
    let enc_ms = ms(t_enc.elapsed());
    out.sink.write(bitstream, frame)?;
    if let (Some(d), Some((from, to))) = (out.dropped_sink.as_mut(), args.drop)
        && !(from..to).contains(&frame)
    {
        d.write(bitstream, frame)?;
    }
    Ok((info, enc_ms))
}

/// Feeds the encoded frame to the cap controller and applies its bitrate.
fn steer_cap(
    cap_control: &mut Option<CapController>,
    static_map: &mut Option<StaticQpMap>,
    encoder: &mut Encoder,
    info: &EncodedFrame,
) -> Result<()> {
    if let Some(c) = cap_control.as_mut() {
        let qp = match static_map.as_mut() {
            Some(m) => m.observe(info.avg_qp).round() as u32,
            None => info.avg_qp,
        };
        c.on_frame(info.size, qp);
        if let Some(bps) = c.poll(Instant::now()) {
            encoder.set_bitrate(bps)?;
        }
    }
    Ok(())
}

/// Prints the statistics and writes `changed_blocks.bin` (in the middle of
/// the printout, as before).
fn report(args: &Args, m: &Measured, frames: u32) -> Result<()> {
    let mbps = |bytes: u64, n: u32| {
        if n == 0 {
            0.0
        } else {
            bytes as f64 * 8.0 * args.fps as f64 / n as f64 / 1e6
        }
    };
    let (frame, n_changed, n_static) = (frames, m.n_changed, m.n_static);
    println!("\nFrames: {frame} ({n_changed} with desktop changes, {n_static} static repeats)");
    if !m.s_render.values.is_empty() {
        println!("Synthetic render, ms: {}", m.s_render.summary());
    }
    println!(
        "Changed 32-px blocks, % of the frame: {}",
        m.s_changed.summary()
    );
    // Per 32-px block, raster order: frames (after the first second) it
    // changed in; the count of such frames first.
    let mut counts = Vec::with_capacity(4 * (m.change_counts.len() + 1));
    for v in std::iter::once(m.counted_frames).chain(m.change_counts.iter().copied()) {
        counts.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(args.out_dir.join("changed_blocks.bin"), counts)?;
    let busy = m
        .change_counts
        .iter()
        .filter(|&&n| n > 0 && n * 10 >= m.counted_frames)
        .count();
    println!(
        "Blocks that changed in >= 10 % of the frames after the first second: {busy} of {}",
        m.change_counts.len()
    );
    if !m.s_map.values.is_empty() {
        println!("QP map update, ms:  {}", m.s_map.summary());
    }
    println!("Copy+convert, ms:   {}", m.s_conv.summary());
    println!("Encode, ms:         {}", m.s_enc.summary());
    println!("Convert+encode, ms: {}", m.s_total.summary());
    println!("Frame size, KiB:    {}", m.s_kb.summary());
    println!(
        "Bitrate @ {} fps:   changing {:.1} Mbit/s, static {:.1} Mbit/s, overall {:.1} Mbit/s",
        args.fps,
        mbps(m.bytes_changed, m.n_changed),
        mbps(m.bytes_static, m.n_static),
        mbps(m.bytes_changed + m.bytes_static, frames)
    );
    Ok(())
}

fn write_meta(
    args: &Args,
    adapter: &str,
    (w, h): (u32, u32),
    rc: RateControl,
    stream_name: &str,
    frames: u32,
    dumped: Vec<u32>,
) -> Result<()> {
    let meta = Meta {
        width: w,
        height: h,
        fps: args.fps,
        codec: args.codec,
        rate_control: format!("{:?}", rc),
        preset: args.preset,
        adapter: adapter.to_string(),
        frames,
        stream: stream_name.into(),
        dumped_frames: dumped,
        verify: args.verify,
    };
    std::fs::write(
        args.out_dir.join("meta.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;
    Ok(())
}

fn print_next_steps(args: &Args, stream_name: &str) {
    println!("\nWrote {}. Next:", args.out_dir.display());
    println!(
        "  ffmpeg -i {0}/{1} -fps_mode passthrough -pix_fmt yuv420p10le -f rawvideo {0}/decoded.yuv",
        args.out_dir.display(),
        stream_name
    );
    println!("  lumen-compare capture --dir {}", args.out_dir.display());
}
