//! lumen-bench: host-side quality & latency bench.
//!
//! capture (DXGI DD) → BGRA→P010 shader → NVENC (HEVC Main10 / AV1) → file
//!
//! Writes into `--out-dir`:
//!   stream.hevc | stream.ivf   encoded bitstream (decode with FFmpeg, see README)
//!   frames.csv                 per-frame timings, sizes, QP
//!   src_NNNNNN.bgra            raw source frames (every --dump-every frames)
//!   gpu_NNNNNN.yuv             GPU converter output as yuv420p10le (with --verify)
//!   meta.json                  everything lumen-compare needs

#[cfg(any(windows, lumen_check))]
mod bench;

#[cfg(any(windows, lumen_check))]
fn main() -> anyhow::Result<()> {
    bench::main()
}

#[cfg(not(any(windows, lumen_check)))]
fn main() {
    eprintln!(
        "lumen-bench runs on Windows with an NVIDIA GPU. Use lumen-compare / lumen-patterns elsewhere."
    );
    std::process::exit(1);
}
