//! Names and paths of transferred files, and small helpers.

use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::PCWSTR;

/// A name from the other side, made safe: a relative path inside the
/// downloads folder, nothing that climbs out of it or that Windows refuses.
pub fn safe_name(name: &str) -> PathBuf {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        let mut p: String = part
            .chars()
            .map(|c| {
                if c < ' ' || "<>:\"|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        while p.ends_with(['.', ' ']) {
            p.pop();
        }
        let p = p.trim_start().to_string();
        if p.is_empty() {
            continue;
        }
        let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
        out.push(if RESERVED.contains(&stem.as_str()) {
            format!("_{p}")
        } else {
            p
        });
    }
    if out.as_os_str().is_empty() {
        out.push("file");
    }
    out
}

/// `path`, or `name (1).ext`, `name (2).ext`... — one that is not taken,
/// neither it nor its `.part`.
pub(super) fn free_path(path: &Path) -> PathBuf {
    let taken = |p: &Path| p.exists() || part_of(p).exists();
    if !taken(path) {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|k| path.with_file_name(format!("{stem} ({k}){ext}")))
        .find(|p| !taken(p))
        .expect("some number is free")
}

pub(super) fn part_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

pub(super) fn free_bytes(dir: &Path) -> Option<u64> {
    let w: Vec<u16> = dir
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// Files under `path` (itself, or everything in a folder), with their names
/// relative to `path`'s parent (`/` between folders); `unlisted`: the names
/// of folders that could not be listed.
pub(super) fn walk(
    path: &Path,
    prefix: &str,
    out: &mut Vec<(PathBuf, String)>,
    unlisted: &mut Vec<String>,
) {
    let name = match path.file_name() {
        Some(n) => format!("{prefix}{}", n.to_string_lossy()),
        None => return,
    };
    if path.is_dir() {
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("files: cannot list {}: {e}", path.display());
                unlisted.push(name);
                return;
            }
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            walk(&e, &format!("{name}/"), out, unlisted);
        }
    } else {
        out.push((path.to_path_buf(), name));
    }
}

/// The first part of a transferred name: the file or folder that was copied.
pub(super) fn top_of(name: &str) -> String {
    name.split('/').next().unwrap_or(name).to_string()
}

/// Where this run numbers its transfers from: random, so that the other side
/// (the host's program outlives the client's) does not take a new transfer
/// for an old one of the same number (saved: «done» at once; cancelled:
/// refused). Below 2^31: the refused rows count down from `u32::MAX`.
pub(super) fn first_transfer_id() -> u32 {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 4];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    (u32::from_le_bytes(b) & 0x7FFF_FFFF).max(1)
}

pub(super) fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn mbps(bytes: u64, since: Instant) -> f64 {
    bytes as f64 * 8.0 / since.elapsed().as_secs_f64().max(1e-3) / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside() {
        assert_eq!(safe_name("a/b/c.txt"), PathBuf::from(r"a\b\c.txt"));
        assert_eq!(safe_name("../../Windows/x"), PathBuf::from(r"Windows\x"));
        assert_eq!(safe_name(r"C:\x"), PathBuf::from(r"C_\x"));
        assert_eq!(safe_name("con.txt"), PathBuf::from("_con.txt"));
        assert_eq!(safe_name("a?b*. "), PathBuf::from("a_b_"));
        assert_eq!(safe_name("//"), PathBuf::from("file"));
        assert_eq!(safe_name("кадр 001.exr"), PathBuf::from("кадр 001.exr"));
    }

    #[test]
    fn free_names() {
        let dir = std::env::temp_dir().join(format!("lumen-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        assert_eq!(free_path(&p), p);
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(free_path(&p), dir.join("a (1).txt"));
        std::fs::write(dir.join("a (1).txt.part"), b"x").unwrap();
        assert_eq!(free_path(&p), dir.join("a (2).txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
