//! Paths on network drives as network paths: `Z:\a\b.mov` → `\\server\share\a\b.mov`.
//!
//! A drive letter of a network drive lives in the signing-in of the user who
//! mapped it: the host (SYSTEM, acting as that user) does not see `Z:` — «the
//! system cannot find the path» — though Explorer does. The clipboard helper
//! runs as the user and hands the host the network path instead.

use std::path::{Component, Path, PathBuf, Prefix};

use windows::Win32::Foundation::{ERROR_CONNECTION_UNAVAIL, NO_ERROR};
use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::core::{PCWSTR, PWSTR};

/// `GetDriveTypeW`: a network drive.
const DRIVE_REMOTE: u32 = 4;

/// `path` with its drive letter replaced by the network share the drive is
/// mapped to; any other path (a local disk, a network path already) as it is.
pub fn network_path(path: &Path) -> PathBuf {
    let Some(letter) = drive_letter(path) else {
        return path.to_path_buf();
    };
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_REMOTE {
        return path.to_path_buf();
    }
    match share_of(letter).and_then(|share| on_share(&share, path)) {
        Some(p) => p,
        None => path.to_path_buf(),
    }
}

/// The letter of `path`'s drive (`Z` of `Z:\a`); `None` for a path without
/// one or relative to the drive's current folder (`Z:a`).
fn drive_letter(path: &Path) -> Option<char> {
    let mut c = path.components();
    let Some(Component::Prefix(p)) = c.next() else {
        return None;
    };
    let Prefix::Disk(letter) = p.kind() else {
        return None;
    };
    matches!(c.next(), Some(Component::RootDir)).then_some(char::from(letter).to_ascii_uppercase())
}

/// `\\server\share` the drive is mapped to (also when Windows has not
/// reconnected it yet: a remembered mapping).
fn share_of(letter: char) -> Option<String> {
    let local: Vec<u16> = format!("{letter}:").encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let e = unsafe {
        WNetGetConnectionW(
            PCWSTR(local.as_ptr()),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if e != NO_ERROR && e != ERROR_CONNECTION_UNAVAIL {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let share = String::from_utf16_lossy(&buf[..end]);
    share.starts_with(r"\\").then_some(share)
}

/// `path` (with a drive letter) moved onto `share`.
fn on_share(share: &str, path: &Path) -> Option<PathBuf> {
    let mut c = path.components();
    c.next()?; // Z:
    c.next()?; // \
    let mut out = PathBuf::from(share.trim_end_matches('\\'));
    for part in c {
        out.push(part.as_os_str());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(drive_letter(Path::new(r"Z:\a\b.mov")), Some('Z'));
        assert_eq!(drive_letter(Path::new(r"c:\x")), Some('C'));
        assert_eq!(drive_letter(Path::new(r"Z:a")), None);
        assert_eq!(drive_letter(Path::new(r"\\srv\share\a")), None);
        assert_eq!(drive_letter(Path::new(r"a\b")), None);
    }

    #[test]
    fn onto_the_share() {
        assert_eq!(
            on_share(r"\\srv\share", Path::new(r"Z:\кадры\BRW_0030\v001.mov")),
            Some(PathBuf::from(r"\\srv\share\кадры\BRW_0030\v001.mov"))
        );
        assert_eq!(
            on_share(r"\\srv\share\", Path::new(r"Z:\a")),
            Some(PathBuf::from(r"\\srv\share\a"))
        );
        assert_eq!(
            on_share(r"\\srv\share", Path::new(r"Z:\")),
            Some(PathBuf::from(r"\\srv\share"))
        );
    }

    #[test]
    fn local_paths_stay() {
        let p = std::env::temp_dir().join("a.txt");
        assert_eq!(network_path(&p), p);
        let unc = PathBuf::from(r"\\srv\share\a");
        assert_eq!(network_path(&unc), unc);
    }
}
