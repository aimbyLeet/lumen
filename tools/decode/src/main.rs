//! lumen-decode: runs `bench-out\stream.hevc` through the client decoder
//! (NVDEC → D3D11 textures), one access unit at a time as in the live path,
//! and compares every frame bit-for-bit with a yuv420p10le reference from FFmpeg.
//! Reports per-AU decode latency (submit → picture in the D3D11 texture).

#[cfg(any(windows, lumen_check))]
mod run;

#[cfg(any(windows, lumen_check))]
fn main() -> anyhow::Result<()> {
    run::main()
}

#[cfg(not(any(windows, lumen_check)))]
fn main() {
    eprintln!("lumen-decode runs on Windows with an NVIDIA GPU.");
    std::process::exit(1);
}
