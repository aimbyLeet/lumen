//! The video window: `lumen-client.exe` next to this program, without a
//! console. Its output goes to the log; the `@status {json}` lines
//! (`--status-json`) are the session's state for the card, and go to the log
//! as `session: {json}` when it changes. When it ends badly,
//! the last error line says why. It is not tied to this app: closing the app
//! leaves it open. The window itself appears once the session is open
//! (`streaming`); until then the program connects without one.
//!
//! The overlays' API runs through it too: `@overlay
//! {json}` lines go to the overlay page, commands go to its stdin.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use desktop::log::Log;
use serde_json::{Value, json};

/// `CREATE_NO_WINDOW`: no console (the video window is the program's own).
const NO_WINDOW: u32 = 0x0800_0000;

/// Numbers the video windows of this run.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct Video {
    /// Tells this video window from the ones before it.
    pub id: u64,
    pub card: u64,
    /// Unix seconds.
    pub since: u64,
    child: Child,
    /// Commands of the overlay page, a line each.
    commands: Mutex<ChildStdin>,
    last_error: Arc<Mutex<String>>,
    /// The last `@status` (apps/client/src/status.rs).
    status: Arc<Mutex<Value>>,
    /// The window is on the screen (it said `streaming` once; it stays
    /// while it reconnects).
    shown: Arc<AtomicBool>,
}

/// Where the overlay page's state goes.
pub type OverlayFeed = Box<dyn Fn(Value) + Send>;

fn pump(
    out: impl Read + Send + 'static,
    log: Arc<Log>,
    last_error: Option<Arc<Mutex<String>>>,
    status: Option<(Arc<Mutex<Value>>, Arc<AtomicBool>, OverlayFeed)>,
) {
    thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Some((.., overlay)) = &status
                && let Some(json) = line.strip_prefix("@overlay ")
            {
                if let Ok(v) = serde_json::from_str::<Value>(json) {
                    overlay(v);
                }
                continue;
            }
            if let Some((s, shown, _)) = &status
                && let Some(json) = line.strip_prefix("@status ")
            {
                if let Ok(v) = serde_json::from_str::<Value>(json) {
                    if v["event"] == "streaming" {
                        shown.store(true, Ordering::Relaxed);
                    }
                    // The hub shows only a word: routes and subnets are for the log.
                    let mut last = s.lock().unwrap();
                    if *last != v {
                        log.line(&format!("session: {json}"));
                    }
                    *last = v;
                }
                continue;
            }
            if let Some(e) = &last_error
                && !line.trim().is_empty()
            {
                *e.lock().unwrap() = line.clone();
            }
            log.line(&line);
        }
    });
}

impl Video {
    pub fn start(
        card: u64,
        args: Vec<String>,
        token: &str,
        data: &Path,
        log: Arc<Log>,
        overlay: OverlayFeed,
    ) -> Result<Video, String> {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|e| Some(e.parent()?.join("lumen-client.exe")))
            .ok_or("no lumen-client.exe")?;
        // A file of its own per session.
        log.new_file(&format!("opening the video window for card {card}"));
        let mut child = Command::new(&exe)
            .args(&args)
            .env("LUMEN_DATA", data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(NO_WINDOW)
            .spawn()
            .map_err(|e| format!("err_video_start|{}: {e}", exe.display()))?;
        // Not on its command line: other programs can read that. A window
        // that did not get it says so itself, like any other failure.
        let mut stdin = child.stdin.take().expect("piped");
        let _ = writeln!(stdin, "{token}");
        let last_error = Arc::new(Mutex::new(String::new()));
        let status = Arc::new(Mutex::new(json!({ "event": "connecting" })));
        let shown = Arc::new(AtomicBool::new(false));
        pump(
            child.stdout.take().expect("piped"),
            log.clone(),
            None,
            Some((status.clone(), shown.clone(), overlay)),
        );
        pump(
            child.stderr.take().expect("piped"),
            log,
            Some(last_error.clone()),
            None,
        );
        Ok(Video {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            card,
            since: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            child,
            commands: Mutex::new(stdin),
            last_error,
            status,
            shown,
        })
    }

    /// A command for the video window (`apps/client/src/ui_api.rs`).
    pub fn command(&self, command: &Value) {
        let mut stdin = self.commands.lock().unwrap();
        let _ = writeln!(stdin, "{command}");
        let _ = stdin.flush();
    }

    /// The video window is on the screen.
    pub fn shown(&self) -> bool {
        self.shown.load(Ordering::Relaxed)
    }

    /// The host has no monitor and asks for ours.
    pub fn asking(&self) -> bool {
        self.status.lock().unwrap()["event"] == "monitor_wanted"
    }

    /// What the video window last said about its session.
    pub fn status(&self) -> Value {
        self.status.lock().unwrap().clone()
    }

    /// `Some(why)` once the window is closed: empty if it ended well.
    pub fn finished(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(code)) if code.success() => Some(String::new()),
            Ok(Some(code)) => {
                let e = self.last_error.lock().unwrap().clone();
                Some(if e.is_empty() {
                    format!("err_video_exit|{code}")
                } else {
                    e
                })
            }
            Ok(None) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
