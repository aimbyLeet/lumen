//! Worker process of the service: `lumen-host.exe` run
//! as SYSTEM in the console session, for the machine's registration (`registry`).
//!
//! SYSTEM in the user's session sees all of its desktops (`Default`, and
//! `Winlogon`: sign-in, lock screen, UAC prompt) and injects input into any
//! window, elevated ones too. The worker's data folder (`LUMEN_DATA`) is the
//! registration's; its output goes to `<folder>\logs\lumen-host-<time>.log`,
//! its `@status` lines to [`Statuses`].

use std::ffi::c_void;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use desktop::log::Log;

use crate::registry::{Statuses, host_log};
use crate::winsec::{Handle, start_on_desktop};
use windows::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::Security::{
    DuplicateTokenEx, SECURITY_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
    TOKEN_ALL_ACCESS, TokenPrimary, TokenSessionId,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// A worker exited this soon after its start: wait before the next one.
const RESTART_DELAY: Duration = Duration::from_secs(2);

/// What the worker should be: its folder and command line. The token is
/// compared too: a new one restarts the worker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spec {
    pub dir: PathBuf,
    pub args: Vec<String>,
    pub token: String,
}

/// The session on the physical console; `None` while it is switching.
pub fn console_session() -> Option<u32> {
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    (id != u32::MAX).then_some(id)
}

/// Why a running worker is stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Why {
    Exited,
    NoLongerConfigured,
    SessionChanged,
    RestartAsked,
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Why::Exited => "exited",
            Why::NoLongerConfigured => "no longer configured",
            Why::SessionChanged => "the console session changed",
            Why::RestartAsked => "restart asked",
        })
    }
}

struct Running {
    spec: Spec,
    session: u32,
    process: Handle,
    started: Instant,
}

impl Running {
    fn exited(&self) -> bool {
        (unsafe { WaitForSingleObject(self.process.get(), 0) }) == WAIT_OBJECT_0
    }

    fn kill(self) {
        unsafe {
            let _ = TerminateProcess(self.process.get(), 1);
            let _ = WaitForSingleObject(self.process.get(), 5000);
        }
    }
}

/// Keeps the worker running in the console session until `stop` says so.
pub struct Workers {
    running: Option<Running>,
    /// When the worker may start again.
    not_before: Option<Instant>,
    /// Kills the worker if the service dies.
    job: Option<Handle>,
    exe: PathBuf,
    log: Arc<Log>,
    statuses: Statuses,
}

impl Workers {
    pub fn new(log: Arc<Log>, statuses: Statuses) -> Result<Self> {
        Ok(Workers {
            running: None,
            not_before: None,
            job: kill_on_close_job(),
            exe: std::env::current_exe().context("current_exe")?,
            log,
            statuses,
        })
    }

    /// One look: starts the worker if it is missing, restarts it if it died
    /// or sits in an old console session, stops it if no longer asked for.
    pub fn tick(&mut self) {
        let wanted = crate::registry::worker_spec();
        let session = console_session();
        if let Some(r) = self.running.take() {
            match self.stop_reason(&r, wanted.as_ref(), session) {
                None => self.running = Some(r),
                Some(why) => self.stop(r, why),
            }
        }
        let (Some(session), Some(spec)) = (session, wanted) else {
            return;
        };
        let now = Instant::now();
        if self.running.is_some() || self.not_before.is_some_and(|t| t > now) {
            return;
        }
        self.not_before = None;
        match self.start(&spec, session) {
            Ok(r) => self.running = Some(r),
            Err(e) => {
                self.log
                    .line(&format!("worker {}: {e:#}", spec.dir.display()));
                self.not_before = Some(now + RESTART_DELAY);
            }
        }
    }

    fn stop_reason(&self, r: &Running, wanted: Option<&Spec>, session: Option<u32>) -> Option<Why> {
        if r.exited() {
            Some(Why::Exited)
        } else if wanted != Some(&r.spec) {
            Some(Why::NoLongerConfigured)
        } else if session.is_some_and(|s| s != r.session) {
            Some(Why::SessionChanged)
        } else {
            self.statuses.take_restart().then_some(Why::RestartAsked)
        }
    }

    fn stop(&mut self, r: Running, why: Why) {
        if why == Why::NoLongerConfigured {
            self.statuses.stopped();
        }
        self.log
            .line(&format!("worker {}: {why}; stopping", r.spec.dir.display()));
        if why == Why::Exited && r.started.elapsed() < RESTART_DELAY * 5 {
            self.not_before = Some(Instant::now() + RESTART_DELAY);
        }
        r.kill();
    }

    pub fn stop_all(&mut self) {
        if let Some(r) = self.running.take() {
            r.kill();
        }
    }

    fn start(&self, spec: &Spec, session: u32) -> Result<Running> {
        let log = Arc::new(Log::new(host_log(&spec.dir)));
        log.line(&format!(
            "--- starting in console session {session}: {}",
            spec.args.join(" ")
        ));
        let run = self.statuses.started();
        let output = Output {
            log,
            statuses: self.statuses.clone(),
            run,
        };
        let process = spawn_in_session(&self.exe, &spec.args, &spec.dir, session, output)?;
        if let Some(job) = &self.job {
            unsafe {
                let _ = AssignProcessToJobObject(job.get(), process.get());
            }
        }
        self.log.line(&format!(
            "worker {}: started in console session {session}",
            spec.dir.display()
        ));
        Ok(Running {
            spec: spec.clone(),
            session,
            process,
            started: Instant::now(),
        })
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn kill_on_close_job() -> Option<Handle> {
    unsafe {
        // SAFETY: the handle comes from `CreateJobObjectW`; the wrapper owns it now.
        let job = Handle::from_raw(CreateJobObjectW(None, PCWSTR::null()).ok()?);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job.get(),
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of_val(&info) as u32,
        )
        .ok()?;
        Some(job)
    }
}

/// Windows command-line quoting of one argument (CommandLineToArgvW rules).
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.extend(std::iter::repeat_n('\\', slashes));
            slashes = 0;
            out.push(c);
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// This process's environment plus `LUMEN_DATA`, as `CreateProcessAsUserW` takes it.
fn environment(data: &Path) -> Vec<u16> {
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("LUMEN_DATA"))
        .collect();
    vars.push(("LUMEN_DATA".into(), data.display().to_string()));
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// Where a worker's output goes: lines into its log, `@status` ones into the
/// status of its run.
struct Output {
    log: Arc<Log>,
    statuses: Statuses,
    run: u64,
}

impl Output {
    /// Reads the worker's output until it ends.
    fn pump(self, out: impl BufRead) {
        let mut last = String::new();
        for line in out.lines() {
            let Ok(line) = line else { break };
            if let Some(json) = line.strip_prefix("@status ") {
                if let Ok(v) = serde_json::from_str(json) {
                    self.statuses.event(self.run, &v);
                }
                continue;
            }
            if !line.trim().is_empty() {
                last.clone_from(&line);
            }
            // A file of its own per session.
            if line.starts_with("incoming session ") {
                self.log.new_file(&line);
                continue;
            }
            self.log.line(&line);
        }
        self.statuses.exited(self.run, &last);
    }
}

/// A primary token of this process (SYSTEM) for `session`.
pub fn session_token(session: u32) -> Result<Handle> {
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)
            .context("OpenProcessToken")?;
        // SAFETY: `own` was opened by `OpenProcessToken` above; the wrapper owns it now.
        let own = Handle::from_raw(own);
        let mut token = HANDLE::default();
        DuplicateTokenEx(
            own.get(),
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut token,
        )
        .context("DuplicateTokenEx")?;
        // SAFETY: `token` was made by `DuplicateTokenEx` above; the wrapper owns it now.
        let token = Handle::from_raw(token);
        SetTokenInformation(
            token.get(),
            TokenSessionId,
            &session as *const u32 as *const c_void,
            4,
        )
        .context("SetTokenInformation(TokenSessionId) (not running as SYSTEM?)")?;
        Ok(token)
    }
}

/// A pipe for a child's output: (our read end, its inheritable write end).
fn output_pipe() -> Result<(Handle, Handle)> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    unsafe {
        CreatePipe(&mut read, &mut write, Some(&sa), 0).context("CreatePipe")?;
        let _ = SetHandleInformation(read, HANDLE_FLAG_INHERIT.0, Default::default());
    }
    // SAFETY: both ends come from `CreatePipe` above; the wrappers own them now.
    Ok(unsafe { (Handle::from_raw(read), Handle::from_raw(write)) })
}

fn command_line(exe: &Path, args: &[String]) -> String {
    let mut cmd = quote(&exe.display().to_string());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&quote(a));
    }
    cmd
}

/// Runs this program with `args` and `token` (a primary token for the
/// console session) on that session's desktop; waits up to `timeout`
/// (then kills it). Returns its exit code and output.
pub fn run_and_wait(token: HANDLE, args: &[String], timeout: Duration) -> Result<(u32, String)> {
    let exe = std::env::current_exe().context("current_exe")?;
    let (read, write) = output_pipe()?;
    let process = start_on_desktop(token, &command_line(&exe, args), Some(write.get()), None)?;
    drop(write);
    let out = unsafe { File::from_raw_handle(read.get().0) };
    std::mem::forget(read);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(out).read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let mut code = 0u32;
    unsafe {
        if WaitForSingleObject(process.get(), timeout.as_millis() as u32) != WAIT_OBJECT_0 {
            let _ = TerminateProcess(process.get(), 1);
            let _ = WaitForSingleObject(process.get(), 5000);
        }
        GetExitCodeProcess(process.get(), &mut code).context("GetExitCodeProcess")?;
    }
    let text = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    Ok((code, text))
}

/// Starts `exe args` as SYSTEM (this service's account) in `session`, on its
/// `Default` desktop, its output into `output`.
fn spawn_in_session(
    exe: &Path,
    args: &[String],
    data: &Path,
    session: u32,
    output: Output,
) -> Result<Handle> {
    let token = session_token(session)?;
    let (read, write) = output_pipe()?;
    let cmd = command_line(exe, args);
    let process = start_on_desktop(
        token.get(),
        &cmd,
        Some(write.get()),
        Some(&environment(data)),
    )?;
    // Only the child's copy of the write end stays: the reader ends with it.
    drop(write);
    let out = unsafe { File::from_raw_handle(read.get().0) };
    std::mem::forget(read);
    std::thread::spawn(move || output.pump(BufReader::new(out)));
    Ok(process)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_follows_command_line_to_argv() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(
            quote(r"C:\Program Files\Lumen\lumen-host.exe"),
            r#""C:\Program Files\Lumen\lumen-host.exe""#
        );
        assert_eq!(quote(r"a b\"), r#""a b\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    #[test]
    fn output_goes_to_the_log_and_the_status() {
        let dir = std::env::temp_dir().join(format!("lumen-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("lumen-host.log");
        let statuses = Statuses::default();
        let run = statuses.started();
        let output = Output {
            log: Arc::new(Log::new(base.clone())),
            statuses: statuses.clone(),
            run,
        };
        let text = "starting\n@status {\"event\":\"online\"}\nincoming session 1 from anna\n\
                    a line\n\n";
        output.pump(std::io::Cursor::new(text));
        // The session's line starts a file of its own.
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 2);
        let current = std::fs::read_to_string(desktop::log::latest(&base).unwrap()).unwrap();
        assert!(current.contains("--- incoming session 1 from anna") && current.contains("a line"));
        assert!(!current.contains("@status"));
        // Ended: an error with the last line said.
        let st = statuses.status().unwrap();
        assert_eq!((st.state.as_str(), st.detail.as_str()), ("error", "a line"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
