//! The installer the server offers through the update door
//! (`protocol::update`): `lumen-setup-<version>.exe`
//! in the update folder (in the image: `/usr/share/lumen/update`). Its version
//! comes from its name — the server's own in a release; a test installer of
//! another version next to a server is how the update is checked.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use protocol::update::{CHUNK_LEN, SHA256_LEN, file_sha256, parse_version};

/// Downloads at once (owner's decision 2026-11-07); more get `Busy`.
pub const MAX_DOWNLOADS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFile {
    pub path: PathBuf,
    pub version: String,
    pub size: u64,
    pub sha256: [u8; SHA256_LEN],
}

impl UpdateFile {
    /// The installer in `dir`: the one `lumen-setup-<version>.exe` there
    /// (with several, the highest version). None: there is none.
    pub fn find(dir: &Path) -> Result<Option<UpdateFile>> {
        let mut best: Option<((u32, u32, u32), PathBuf, String)> = None;
        let entries = std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
        for e in entries {
            let path = e?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(version) = name
                .strip_prefix("lumen-setup-")
                .and_then(|r| r.strip_suffix(".exe"))
            else {
                continue;
            };
            let Some(v) = parse_version(version) else {
                continue;
            };
            if best.as_ref().is_none_or(|(b, _, _)| v > *b) {
                best = Some((v, path.clone(), version.to_string()));
            }
        }
        let Some((_, path, version)) = best else {
            return Ok(None);
        };
        let (size, sha256) =
            file_sha256(&path).with_context(|| format!("read {}", path.display()))?;
        if size == 0 {
            bail!("{} is empty", path.display());
        }
        Ok(Some(UpdateFile {
            path,
            version,
            size,
            sha256,
        }))
    }

    /// The installer's bytes from `offset` on, [`CHUNK_LEN`] at a time.
    pub fn chunks(&self, offset: u64) -> io::Result<Chunks> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset.min(self.size)))?;
        Ok(Chunks {
            file,
            left: self.size.saturating_sub(offset),
        })
    }
}

pub struct Chunks {
    file: File,
    left: u64,
}

impl Iterator for Chunks {
    type Item = io::Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        let n = self.left.min(CHUNK_LEN as u64) as usize;
        let mut buf = vec![0u8; n];
        if let Err(e) = self.file.read_exact(&mut buf) {
            self.left = 0;
            return Some(Err(e));
        }
        self.left -= n as u64;
        Some(Ok(buf))
    }
}

/// Counts downloads under way; a slot frees itself when dropped.
#[derive(Default)]
pub struct Downloads(AtomicUsize);

pub struct Slot<'a>(&'a AtomicUsize);

impl Downloads {
    pub fn take(&self) -> Option<Slot<'_>> {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_DOWNLOADS).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(&self.0))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_the_highest_installer() {
        let d = dir("find");
        assert_eq!(UpdateFile::find(&d).unwrap(), None);
        std::fs::write(d.join("lumen-setup-0.7.0.exe"), b"old").unwrap();
        std::fs::write(d.join("lumen-setup-0.10.1.exe"), b"abc").unwrap();
        std::fs::write(d.join("lumen-setup-x.exe"), b"no").unwrap();
        std::fs::write(d.join("readme.txt"), b"no").unwrap();
        let f = UpdateFile::find(&d).unwrap().unwrap();
        assert_eq!(f.version, "0.10.1");
        assert_eq!(f.size, 3);
        // SHA-256("abc")
        assert_eq!(f.sha256[..4], [0xba, 0x78, 0x16, 0xbf], "{:02x?}", f.sha256);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn chunks_from_an_offset() {
        let d = dir("chunks");
        let data: Vec<u8> = (0..CHUNK_LEN * 2 + 100).map(|i| i as u8).collect();
        std::fs::write(d.join("lumen-setup-0.7.1.exe"), &data).unwrap();
        let f = UpdateFile::find(&d).unwrap().unwrap();
        let all: Vec<u8> = f.chunks(0).unwrap().flat_map(|c| c.unwrap()).collect();
        assert_eq!(all, data);
        assert_eq!(f.chunks(0).unwrap().count(), 3);
        let tail: Vec<u8> = f
            .chunks(CHUNK_LEN as u64 + 7)
            .unwrap()
            .flat_map(|c| c.unwrap())
            .collect();
        assert_eq!(tail, data[CHUNK_LEN + 7..]);
        assert_eq!(f.chunks(f.size + 5).unwrap().count(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn download_slots() {
        let d = Downloads::default();
        let slots: Vec<_> = (0..MAX_DOWNLOADS).map(|_| d.take().unwrap()).collect();
        assert!(d.take().is_none());
        drop(slots);
        assert!(d.take().is_some());
    }
}
