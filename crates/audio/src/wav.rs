//! 32-bit float WAV files (format 3), interleaved: dumps and checks.

use std::io::{self, Write};
use std::path::Path;

/// Writes `samples` (interleaved, `channels` per frame) at `rate` to `path`.
pub fn write_f32(path: &Path, rate: u32, channels: u16, samples: &[f32]) -> io::Result<()> {
    let data_len = u32::try_from(samples.len() * 4)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too long for a WAV file"))?;
    let block = u32::from(channels) * 4;
    let mut out = Vec::with_capacity(44 + samples.len() * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::File::create(path)?.write_all(&out)
}

/// Writes a 16-bit PCM WAV of `samples` (mono, -1..1) at `rate`: something any
/// player can play (the test tone).
pub fn write_i16_mono(path: &Path, rate: u32, samples: &[f32]) -> io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::File::create(path)?.write_all(&out)
}

/// Reads a float WAV that [`write_f32`] wrote: (rate, channels, samples).
pub fn read_f32(path: &Path) -> io::Result<(u32, u16, Vec<f32>)> {
    let d = std::fs::read(path)?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a float WAV of ours");
    if d.len() < 44 || &d[..4] != b"RIFF" || &d[8..16] != b"WAVEfmt " {
        return Err(bad());
    }
    let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    if u16_at(20) != 3 || u16_at(34) != 32 || &d[36..40] != b"data" {
        return Err(bad());
    }
    let n = (u32_at(40) as usize).min(d.len() - 44);
    let samples = d[44..44 + n]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Ok((u32_at(24), u16_at(22), samples))
}

#[cfg(test)]
mod tests {
    #[test]
    fn float_wav_roundtrip() {
        let path = std::env::temp_dir().join(format!("lumen-wav-{}.wav", std::process::id()));
        let v: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        super::write_f32(&path, 44_100, 2, &v).unwrap();
        assert_eq!(super::read_f32(&path).unwrap(), (44_100, 2, v));
        let _ = std::fs::remove_file(path);
    }
}
