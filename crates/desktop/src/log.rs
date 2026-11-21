//! An app's log file: what its child process prints, with the local time.
//! Files `<name>-YYYY-MM-DD-HH-MM-SS.log` (when the file was started) next to
//! the `<name>.log` the log is made for; the newest [`KEEP`] stay. A new file
//! starts with each session ([`Log::new_file`]) and when one grows over
//! [`MAX_BYTES`].

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Files kept, the current one included.
const KEEP: usize = 10;
const MAX_BYTES: u64 = 10 << 20;
/// What the window shows: the end of the file.
const TAIL_BYTES: u64 = 256 << 10;

pub struct Log {
    /// `<dir>\<name>.log`: the files are named after it.
    base: PathBuf,
    current: Mutex<Current>,
}

struct Current {
    path: PathBuf,
    file: Option<File>,
}

fn local_clock() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// `HH:MM:SS` of the local clock.
fn local_time() -> String {
    let t = local_clock();
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

/// `YYYY-MM-DD-HH-MM-SS` of the local clock.
fn file_stamp() -> String {
    let t = local_clock();
    format!(
        "{:04}-{:02}-{:02}-{:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn name_of(base: &Path) -> String {
    base.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn dir_of(base: &Path) -> &Path {
    base.parent().unwrap_or(Path::new("."))
}

/// The files of `base`'s log, oldest first (the names sort by time).
fn files(base: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}-", name_of(base));
    let mut v: Vec<PathBuf> = fs::read_dir(dir_of(base))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                n.starts_with(&prefix)
                    && n.ends_with(".log")
                    && n.as_bytes()
                        .get(prefix.len())
                        .is_some_and(u8::is_ascii_digit)
            })
        })
        .collect();
    v.sort();
    v
}

/// The newest file of `base`'s log (what a window shows), if there is one.
pub fn latest(base: &Path) -> Option<PathBuf> {
    files(base).pop()
}

/// A new file named by the time now; the oldest ones go, so that `KEEP`
/// stay with it.
fn start_file(base: &Path) -> PathBuf {
    let dir = dir_of(base);
    let _ = fs::create_dir_all(dir);
    let name = name_of(base);
    let stamp = file_stamp();
    let old = files(base);
    // Two in one second: `_002`, `_003`... (they sort after the plain name);
    // the new one is the newest by name.
    let path = (1..)
        .map(|k| match k {
            1 => dir.join(format!("{name}-{stamp}.log")),
            k => dir.join(format!("{name}-{stamp}_{k:03}.log")),
        })
        .find(|p| old.last().is_none_or(|last| p > last))
        .unwrap();
    for p in &old[..old.len().saturating_sub(KEEP - 1)] {
        let _ = fs::remove_file(p);
    }
    path
}

/// Removes the files of earlier builds: `<name>.log`, `<name>.log.1`, `<name>.N.log`.
fn remove_legacy(base: &Path) {
    let name = name_of(base);
    let _ = fs::remove_file(base);
    let _ = fs::remove_file(base.with_extension("log.1"));
    for n in 1..KEEP {
        let _ = fs::remove_file(dir_of(base).join(format!("{name}.{n}.log")));
    }
}

fn open(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

impl Log {
    /// Goes on in the newest file, if it is not full, else starts one.
    pub fn new(base: PathBuf) -> Log {
        remove_legacy(&base);
        let path = match latest(&base) {
            Some(p) if fs::metadata(&p).is_ok_and(|m| m.len() < MAX_BYTES) => p,
            _ => start_file(&base),
        };
        Log {
            base,
            current: Mutex::new(Current {
                file: open(&path),
                path,
            }),
        }
    }

    /// The file written now.
    pub fn path(&self) -> PathBuf {
        self.current.lock().unwrap().path.clone()
    }

    /// Starts a new file (a new session), unless the current one is empty;
    /// `header` is its first line.
    pub fn new_file(&self, header: &str) {
        let mut c = self.current.lock().unwrap();
        if fs::metadata(&c.path).is_ok_and(|m| m.len() > 0) {
            c.file = None;
            c.path = start_file(&self.base);
        }
        c.write(&format!("--- {header}"));
    }

    pub fn line(&self, text: &str) {
        let mut c = self.current.lock().unwrap();
        if c.file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() > MAX_BYTES)
        {
            c.file = None;
            c.path = start_file(&self.base);
            c.write("--- continued");
        }
        c.write(text);
    }

    /// The end of the current file, from a whole line on.
    pub fn tail(&self) -> String {
        tail(&self.path(), TAIL_BYTES)
    }
}

impl Current {
    fn write(&mut self, text: &str) {
        if self.file.is_none() {
            self.file = open(&self.path);
        }
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{} {text}", local_time());
        }
    }
}

/// The last `bytes` of `path`, from a whole line on.
pub fn tail(path: &Path, bytes: u64) -> String {
    let Ok(mut f) = File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let from = len.saturating_sub(bytes);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_ok() {
        let _ = f.read_to_end(&mut buf);
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    match (from > 0, text.find('\n')) {
        (true, Some(i)) => text[i + 1..].to_string(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_per_session_named_by_time_the_newest_kept() {
        let dir = temp("ring");
        let base = dir.join("x.log");
        fs::write(&base, "old build").unwrap();
        fs::write(base.with_extension("log.1"), "old build").unwrap();
        fs::write(dir.join("x.3.log"), "old build").unwrap();
        fs::write(dir.join("other.log"), "not ours").unwrap();
        let log = Log::new(base.clone());
        log.new_file("empty: no new file");
        for i in 0..KEEP + 3 {
            log.new_file(&format!("session {i}"));
            log.line("a line");
        }
        let ours = files(&base);
        assert_eq!(ours.len(), KEEP, "{ours:?}");
        assert!(!base.exists() && !dir.join("x.3.log").exists());
        assert!(dir.join("other.log").exists());
        let name = ours[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("x-20") && name.ends_with(".log"), "{name}");
        assert_eq!(latest(&base), Some(log.path()));
        assert!(log.tail().contains(&format!("session {}", KEEP + 2)));
        // Many in one second get _002, _003...: the order still holds.
        assert!(fs::read_to_string(&ours[0]).unwrap().contains("session 3"));
        drop(log);
        // A new start goes on in the newest file.
        let again = Log::new(base.clone());
        assert_eq!(Some(again.path()), latest(&base));
        drop(again);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_session_goes_on_in_the_next_file() {
        let dir = temp("big");
        let base = dir.join("x.log");
        let log = Log::new(base.clone());
        log.new_file("session");
        let first = log.path();
        let text = "y".repeat(1 << 20);
        for _ in 0..=MAX_BYTES >> 20 {
            log.line(&text);
        }
        log.line("after");
        assert_ne!(log.path(), first);
        assert!(fs::metadata(log.path()).unwrap().len() < MAX_BYTES);
        let current = fs::read_to_string(log.path()).unwrap();
        assert!(current.lines().next().unwrap().ends_with("--- continued"));
        assert!(current.ends_with("after\n"));
        assert!(fs::read_to_string(&first).unwrap().contains("session"));
        drop(log);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_goes_on_in_a_file_under_the_limit_and_leaves_a_full_one() {
        let dir = temp("limit");
        let base = dir.join("x.log");
        let file = dir.join("x-2000-01-01-00-00-00.log");
        fs::write(&file, "a line\n").unwrap();
        let log = Log::new(base.clone());
        assert_eq!(log.path(), file);
        drop(log);
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(MAX_BYTES)
            .unwrap();
        let log = Log::new(base.clone());
        assert_ne!(log.path(), file);
        assert!(log.path() > file);
        assert_eq!(latest(&base), Some(log.path()));
        drop(log);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_file_in_one_second_gets_a_suffix_and_sorts_after() {
        let dir = temp("suffix");
        let base = dir.join("x.log");
        let first = dir.join(format!("x-{}.log", file_stamp()));
        fs::write(&first, "a line\n").unwrap();
        let second = start_file(&base);
        assert!(second > first, "{second:?}");
        // Unless the clock ticked over between the two stamps.
        if second.to_string_lossy().contains(&file_stamp()) {
            assert!(second.to_string_lossy().ends_with("_002.log"), "{second:?}");
        }
        fs::write(&second, "a line\n").unwrap();
        let third = start_file(&base);
        assert!(third > second, "{third:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_starts_at_a_whole_line() {
        let dir = temp("tail");
        let path = dir.join("t.log");
        fs::write(&path, "first line\nsecond line\nthird line\n").unwrap();
        // 15 bytes: "...ond line\nthird line\n" cut inside "second line".
        assert_eq!(tail(&path, 15), "third line\n");
        // Everything fits: nothing is cut.
        assert_eq!(tail(&path, 1000), "first line\nsecond line\nthird line\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
