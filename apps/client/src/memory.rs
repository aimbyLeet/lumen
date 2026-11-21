//! The view of a machine remembered between sessions with several monitors at the host, the mode, which monitor mode 2
//! shows and every window's place. Lumen Client gives each machine's card a
//! file of its own (`--view-memory`); the next session with the same monitors
//! at the host starts as the last one was, other monitors start in mode 1.
//!
//! Lumen Client ends the window by killing the process: the file is written
//! whenever the view changes, not at exit.

use std::path::{Path, PathBuf};

use protocol::Monitor;
use serde_json::{Value, json};

use crate::render::Mode;

/// Where a window is: its framed place (`WINDOWPLACEMENT` workspace
/// coordinates), maximised or not, and borderless over the monitor of this
/// machine around a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub normal: (i32, i32, i32, i32),
    pub maximized: bool,
    pub borderless: Option<(i32, i32)>,
}

/// One session's view: the host's monitors, the mode, the windows' places
/// (the main window first, then mode 3's in the order of their monitors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub monitors: Vec<Monitor>,
    pub mode: Mode,
    pub windows: Vec<Place>,
}

fn mode_text(m: Mode) -> String {
    match m {
        Mode::All => "all".into(),
        Mode::One(i) => format!("one:{i}"),
        Mode::Windows => "windows".into(),
    }
}

fn parse_mode(s: &str, monitors: usize) -> Option<Mode> {
    match s {
        "all" => Some(Mode::All),
        "windows" => Some(Mode::Windows),
        _ => {
            let i: usize = s.strip_prefix("one:")?.parse().ok()?;
            (i < monitors).then_some(Mode::One(i))
        }
    }
}

impl View {
    pub fn to_json(&self) -> Value {
        json!({
            "monitors": self.monitors.iter().map(|m| json!([m.stream, m.x, m.y, m.width, m.height])).collect::<Vec<_>>(),
            "mode": mode_text(self.mode),
            "windows": self.windows.iter().map(|p| json!({
                "normal": [p.normal.0, p.normal.1, p.normal.2, p.normal.3],
                "maximized": p.maximized,
                "borderless": p.borderless.map(|(x, y)| json!([x, y])),
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<View> {
        let int = |v: &Value| v.as_i64().and_then(|n| i32::try_from(n).ok());
        let monitors = v["monitors"]
            .as_array()?
            .iter()
            .map(|m| {
                Some(Monitor {
                    stream: u8::try_from(m[0].as_u64()?).ok()?,
                    x: int(&m[1])?,
                    y: int(&m[2])?,
                    width: u32::try_from(m[3].as_u64()?).ok()?,
                    height: u32::try_from(m[4].as_u64()?).ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let mode = parse_mode(v["mode"].as_str()?, monitors.len())?;
        let windows = v["windows"]
            .as_array()?
            .iter()
            .map(|w| {
                let n = w["normal"].as_array()?;
                let b = &w["borderless"];
                Some(Place {
                    normal: (int(&n[0])?, int(&n[1])?, int(&n[2])?, int(&n[3])?),
                    maximized: w["maximized"].as_bool()?,
                    borderless: if b.is_null() {
                        None
                    } else {
                        Some((int(&b[0])?, int(&b[1])?))
                    },
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(View {
            monitors,
            mode,
            windows,
        })
    }
}

/// A card's memory file.
pub struct Memory {
    path: PathBuf,
    /// As read at the start (none: no file, or unreadable).
    pub remembered: Option<View>,
    written: Option<View>,
}

impl Memory {
    pub fn open(path: &Path) -> Memory {
        let remembered = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| View::from_json(&v));
        Memory {
            path: path.to_path_buf(),
            written: remembered.clone(),
            remembered,
        }
    }

    /// Writes the view if it changed since the last write (several monitors only:
    /// one is shown as always). Errors are printed, the session goes on.
    pub fn keep(&mut self, view: View) {
        if view.monitors.len() < 2 || self.written.as_ref() == Some(&view) {
            return;
        }
        let text = view.to_json().to_string();
        let tmp = self.path.with_extension("tmp");
        let r = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &self.path));
        match r {
            Ok(()) => self.written = Some(view),
            Err(e) => {
                eprintln!("view memory {:?}: {e}", self.path);
                self.written = Some(view); // not again every half second
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(mode: Mode) -> View {
        View {
            monitors: vec![
                Monitor {
                    stream: 0,
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
                Monitor {
                    stream: 1,
                    x: 2560,
                    y: -200,
                    width: 1920,
                    height: 1080,
                },
            ],
            mode,
            windows: vec![
                Place {
                    normal: (10, 20, 1300, 760),
                    maximized: false,
                    borderless: None,
                },
                Place {
                    normal: (-1500, 40, -100, 900),
                    maximized: true,
                    borderless: Some((-1000, 500)),
                },
            ],
        }
    }

    #[test]
    fn a_view_comes_back_as_written() {
        for mode in [Mode::All, Mode::One(1), Mode::Windows] {
            let v = view(mode);
            let text = v.to_json().to_string();
            let back = View::from_json(&serde_json::from_str(&text).unwrap());
            assert_eq!(back, Some(v));
        }
    }

    #[test]
    fn a_broken_view_is_none() {
        let mut j = view(Mode::One(1)).to_json();
        j["mode"] = json!("one:5"); // no such monitor
        assert_eq!(View::from_json(&j), None);
        assert_eq!(View::from_json(&json!({"mode": "all"})), None);
    }
}
