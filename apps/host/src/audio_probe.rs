//! `lumen-host audio-probe`: what sound this machine can give, before it is
//! streamed. Lists the output devices, captures the default one for a while
//! (WASAPI loopback) and tells how the packets came: their size, silence,
//! gaps in the device's stream. With `--out` writes what was captured (gaps
//! as silence, at their place in the stream); with `--tone` checks a test
//! tone played meanwhile.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use audio::{Loopback, Packet};

/// How often the capture is read.
const POLL: Duration = Duration::from_millis(5);
/// No packet for this long: a stretch without data (nothing played).
const IDLE: Duration = Duration::from_millis(50);

pub fn run(seconds: f64, out: Option<&Path>, tone: Option<f64>) -> Result<()> {
    println!(
        "user: {}",
        std::env::var("USERNAME").unwrap_or_else(|_| "?".into())
    );
    let devices = audio::render_endpoints()?;
    println!("output devices: {}", devices.len());
    for d in &devices {
        println!(
            "  {}{}  {}",
            if d.default { "[default] " } else { "" },
            d.name,
            if d.active { "active" } else { "not active" }
        );
    }
    let mut lb = Loopback::open_default()?;
    let f = lb.format();
    println!("capturing «{}»: {f}, {seconds} s", lb.device());

    let mut samples = Vec::new();
    let mut packets: Vec<Packet> = Vec::new();
    let mut idle = 0usize;
    let mut ages: Vec<f64> = Vec::new();
    let mut last_data = Instant::now();
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let before = packets.len();
        lb.read(&mut samples, &mut packets)?;
        // How old each packet's first frame is when read, by its own time.
        let now = audio::qpc_now_100ns() as i64;
        ages.extend(
            packets[before..]
                .iter()
                .map(|p| (now - p.qpc_100ns as i64) as f64 / 1e4),
        );
        if packets.len() > before {
            if last_data.elapsed() >= IDLE && before > 0 {
                idle += 1;
            }
            last_data = Instant::now();
        }
        std::thread::sleep(POLL);
    }

    let ch = usize::from(f.channels);
    let Some(first) = packets.first().copied() else {
        println!("no packets: nothing played (loopback gives none in silence)");
        return Ok(());
    };
    // Each packet at its place in the device's stream: gaps become silence.
    let mut placed: Vec<f32> = Vec::with_capacity(samples.len());
    let (mut gaps, mut gap_frames, mut overlaps) = (0usize, 0u64, 0usize);
    let mut at = 0usize;
    for p in &packets {
        let n = p.frames as usize * ch;
        let want = (p.device_position.saturating_sub(first.device_position)) as usize * ch;
        if want > placed.len() {
            gaps += 1;
            gap_frames += ((want - placed.len()) / ch) as u64;
            placed.resize(want, 0.0);
        } else if want < placed.len() {
            // Again from where the device says it starts.
            overlaps += 1;
            placed.truncate(want);
        }
        placed.extend_from_slice(&samples[at..at + n]);
        at += n;
    }

    let frames: u64 = packets.iter().map(|p| u64::from(p.frames)).sum();
    let last = packets.last().copied().unwrap_or(first);
    let silent = packets.iter().filter(|p| p.silent).count();
    let discontinuities = packets.iter().filter(|p| p.discontinuity).count();
    let mut sizes: Vec<u32> = packets.iter().map(|p| p.frames).collect();
    sizes.sort_unstable();
    println!(
        "packets: {} ({} frames, {:.2} s of sound), size p50 {} frames ({:.1} ms), silent {silent}, discontinuity {discontinuities}",
        packets.len(),
        frames,
        frames as f64 / f64::from(f.rate),
        sizes[sizes.len() / 2],
        f64::from(sizes[sizes.len() / 2]) * 1000.0 / f64::from(f.rate),
    );
    println!(
        "device stream: {:.2} s from the first packet to the last; gaps {gaps} ({:.2} s), overlaps {overlaps}, stretches without packets ≥ {} ms: {idle}",
        (last.device_position - first.device_position + u64::from(last.frames)) as f64
            / f64::from(f.rate),
        gap_frames as f64 / f64::from(f.rate),
        IDLE.as_millis(),
    );
    ages.sort_by(f64::total_cmp);
    println!(
        "packet age when read, by its own time: p50 {:.1} ms (min {:.1}, max {:.1})",
        ages[ages.len() / 2],
        ages[0],
        ages[ages.len() - 1]
    );
    // The device's clock against the performance counter (drift, ppm).
    let qpc_s = (last.qpc_100ns.saturating_sub(first.qpc_100ns)) as f64 / 1e7;
    let pos_s = (last.device_position - first.device_position) as f64 / f64::from(f.rate);
    if qpc_s > 1.0 {
        println!(
            "device clock vs performance counter: {:+.0} ppm",
            (pos_s / qpc_s - 1.0) * 1e6
        );
    }
    if let Some(path) = out {
        audio::wav::write_f32(path, f.rate, f.channels, &placed)?;
        println!("written {}", path.display());
    }
    if let Some(freq) = tone {
        let left: Vec<f32> = placed.iter().step_by(ch).copied().collect();
        let r = audio::tone::analyse(&left, f.rate, freq);
        println!("tone {freq} Hz: {r}");
    }
    Ok(())
}

/// `lumen-host audio-probe --write-tone`: the test tone as a WAV any player
/// plays (1 s of silence, the tone, 1 s of silence).
pub fn write_tone(path: &Path, freq: f64, seconds: f64) -> Result<()> {
    let rate = 48_000;
    let mut v = vec![0.0; rate as usize];
    v.extend(audio::tone::sine(rate, freq, 0.1, seconds));
    v.extend(vec![0.0; rate as usize]);
    audio::wav::write_i16_mono(path, rate, &v)?;
    Ok(())
}

/// `lumen-host audio-probe --decode`: a `--dump-audio` file decoded as the
/// client would (frames missing from it as silence), written to `out`, the
/// tone checked.
pub fn decode(dump: &Path, out: Option<&Path>, tone: Option<f64>) -> Result<()> {
    use audio::opus::{CHANNELS, Decoder, FRAME, RATE};
    let records = crate::audio::read_dump(dump)?;
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        println!("no frames in {}", dump.display());
        return Ok(());
    };
    let mut d = Decoder::new()?;
    let mut pcm = [0f32; FRAME * CHANNELS];
    let mut all = Vec::with_capacity(records.len() * FRAME * CHANNELS);
    let (mut missing, mut next) = (0u64, first.0);
    for (index, _, packet) in &records {
        while next < *index {
            all.extend(std::iter::repeat_n(0.0, FRAME * CHANNELS));
            missing += 1;
            next += 1;
        }
        let n = d.decode(Some(packet), &mut pcm)?;
        all.extend_from_slice(&pcm[..n * CHANNELS]);
        next = index + 1;
    }
    let mut sizes: Vec<usize> = records.iter().map(|r| r.2.len()).collect();
    sizes.sort_unstable();
    // Capture times should step by 10 ms.
    let mut steps: Vec<i64> = records
        .windows(2)
        .filter(|w| w[1].0 == w[0].0 + 1)
        .map(|w| w[1].1 as i64 - w[0].1 as i64)
        .collect();
    steps.sort_unstable();
    println!(
        "{} frames ({} .. {}), missing {missing}; packet p50 {} bytes (max {}); capture step p50 {} us (min {}, max {})",
        records.len(),
        first.0,
        last.0,
        sizes[sizes.len() / 2],
        sizes[sizes.len() - 1],
        steps.get(steps.len() / 2).copied().unwrap_or(0),
        steps.first().copied().unwrap_or(0),
        steps.last().copied().unwrap_or(0),
    );
    if let Some(path) = out {
        audio::wav::write_f32(path, RATE, CHANNELS as u16, &all)?;
        println!("written {}", path.display());
    }
    if let Some(freq) = tone {
        let left: Vec<f32> = all.iter().step_by(CHANNELS).copied().collect();
        let r = audio::tone::analyse(&left, RATE, freq);
        println!("tone {freq} Hz: {r}");
    }
    Ok(())
}

/// `lumen-host audio-probe --wav`: the tone in a float WAV (its left channel).
pub fn check_wav(path: &Path, freq: f64) -> Result<()> {
    let (rate, ch, samples) = audio::wav::read_f32(path)?;
    let left: Vec<f32> = samples
        .iter()
        .step_by(usize::from(ch.max(1)))
        .copied()
        .collect();
    let r = audio::tone::analyse(&left, rate, freq);
    println!(
        "{}: {:.2} s at {rate} Hz; tone {freq} Hz: {r}",
        path.display(),
        left.len() as f64 / f64::from(rate),
    );
    Ok(())
}
