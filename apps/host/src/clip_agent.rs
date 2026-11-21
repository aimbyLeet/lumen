//! The host's clipboard through a helper run as the signed-in user
//! (`lumen-host clip-agent`): the host runs as SYSTEM, which does not see
//! what programs put on the clipboard through OLE — Explorer's files, some
//! programs' text. The helper keeps the
//! clipboard in the user's session and passes it over two pipes only SYSTEM
//! and that user may open (`desktop::clipboard::Clipboard::over_pipes`).

use std::fs::{File, OpenOptions};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use desktop::clipboard::Clipboard;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::core::PCWSTR;

use crate::user_files::ConsoleUser;
use crate::winsec::{Handle, Security, wide};

/// How long the helper has to start and connect.
const CONNECT: Duration = Duration::from_secs(5);
/// Pipe buffers: a picture goes through in pieces anyway.
const PIPE_BUFFER: u32 = 1 << 20;

static NEXT: AtomicU32 = AtomicU32::new(0);

fn create(name: &str, inbound: bool, sec: &Security) -> Result<Handle> {
    let w = wide(name);
    let access = if inbound {
        PIPE_ACCESS_INBOUND
    } else {
        PIPE_ACCESS_OUTBOUND
    };
    let h = unsafe {
        CreateNamedPipeW(
            PCWSTR(w.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(access.0),
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            Some(&sec.attrs),
        )
    };
    if h.is_invalid() {
        bail!("pipe {name}: {}", windows::core::Error::from_thread());
    }
    // SAFETY: `h` is a valid pipe handle from `CreateNamedPipeW`; the wrapper owns it now.
    Ok(unsafe { Handle::from_raw(h) })
}

/// Waits for the helper on both pipes, at most [`CONNECT`].
fn connect(up: &Handle, down: &Handle, names: [String; 2]) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let raw = [up.get().0 as isize, down.get().0 as isize];
    std::thread::spawn(move || {
        let ok = raw.iter().all(|&h| {
            let h = HANDLE(h as *mut _);
            unsafe { ConnectNamedPipe(h, None) }.is_ok()
                || windows::core::Error::from_thread().code()
                    == windows::Win32::Foundation::ERROR_PIPE_CONNECTED.to_hresult()
        });
        let _ = tx.send(ok);
    });
    match rx.recv_timeout(CONNECT) {
        Ok(true) => Ok(()),
        Ok(false) => bail!("the helper did not connect"),
        Err(_) => {
            // Let the waiting thread go (closing a pipe it waits on would
            // wait too): connect to the pipes ourselves, as the helper would.
            let _up = OpenOptions::new().write(true).open(&names[0]);
            let _down = OpenOptions::new().read(true).open(&names[1]);
            let _ = rx.recv_timeout(Duration::from_secs(1));
            bail!("the helper did not connect in {CONNECT:?}")
        }
    }
}

/// The clipboard through a helper, when this is SYSTEM and someone is
/// signed in; `None` otherwise (the host keeps the clipboard itself).
pub fn start() -> Option<Clipboard> {
    if !crate::user_files::is_system() {
        return None;
    }
    let user = ConsoleUser::get()?;
    match start_for(&user) {
        Ok(c) => {
            println!("clipboard: through a helper run as the signed-in user");
            Some(c)
        }
        Err(e) => {
            eprintln!("clipboard: no helper ({e:#}): the host's own clipboard (OLE copies unseen)");
            None
        }
    }
}

fn start_for(user: &ConsoleUser) -> Result<Clipboard> {
    start_with(&user.sid, |cmd| user.run(cmd).map(drop))
}

/// Pipes for the user `sid`; `launch` starts the helper's command line.
fn start_with(sid: &str, launch: impl FnOnce(&str) -> Result<()>) -> Result<Clipboard> {
    let base = format!(
        r"\\.\pipe\lumen-clip-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let sec = Security::from_sddl(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"))?;
    let (up_name, down_name) = (format!("{base}-up"), format!("{base}-down"));
    let up = create(&up_name, true, &sec)?;
    let down = create(&down_name, false, &sec)?;
    let exe = std::env::current_exe().context("current exe")?;
    let cmd = format!("\"{}\" clip-agent --pipe \"{base}\"", exe.display());
    launch(&cmd).context("start the helper")?;
    connect(&up, &down, [up_name, down_name])?;
    let take = |h: Handle| {
        let raw = h.get().0;
        std::mem::forget(h);
        unsafe { File::from_raw_handle(raw) }
    };
    Ok(Clipboard::over_pipes(take(up), take(down)))
}

/// `lumen-host clip-probe --agent`: the helper started as this user, a
/// text through it onto the clipboard, files from the clipboard back.
pub fn probe() -> Result<()> {
    use desktop::clipboard::Content;
    let sid = crate::user_files::process_sid();
    let board = start_with(&sid, |cmd| {
        // `cmd` is "<exe>" clip-agent --pipe "<base>": the same, directly.
        let base = cmd.rsplit('"').nth(1).context("pipe name")?;
        std::process::Command::new(std::env::current_exe()?)
            .args(["clip-agent", "--pipe", base])
            .spawn()
            .map(drop)
            .context("spawn")
    })?;
    let local = Clipboard::start(false).map_err(anyhow::Error::msg)?;
    let text = Content::Text("lumen clip-agent: проверка 🎨".into());
    let written = board.set(text.clone());
    std::thread::sleep(Duration::from_millis(400));
    let seen = local.changes();
    println!(
        "through the helper: written {written}, seen here {}",
        seen.contains(&text)
    );
    let files = Content::Files(vec![std::env::current_exe()?]);
    local.set(files.clone());
    std::thread::sleep(Duration::from_millis(500));
    let up = board.changes();
    println!("from here through the helper: {}", up.contains(&files));
    if !(written && seen.contains(&text) && up.contains(&files)) {
        bail!("the helper does not pass the clipboard");
    }
    // A helper that never comes: given up on, not waited for forever.
    let t = std::time::Instant::now();
    let none = start_with(&sid, |_| Ok(()));
    println!(
        "a helper that never comes: {} after {:.1} s",
        if none.is_err() {
            "given up"
        } else {
            "CONNECTED?"
        },
        t.elapsed().as_secs_f64()
    );
    if none.is_ok() || t.elapsed() > CONNECT * 2 {
        bail!("waiting for a missing helper");
    }
    println!("RESULT: ok");
    Ok(())
}

/// `lumen-host clip-probe --agent-system` (as SYSTEM, like the service's
/// host): the helper started as the signed-in user; a text put through it
/// (paste it somewhere), then for 30 s every copy made by the user, as the
/// host would see it; files copied are read as the user, as the files thread
/// reads them (a network drive: its network path).
pub fn probe_system() -> Result<()> {
    use desktop::clipboard::Content;
    let Some(board) = start() else {
        bail!("no helper: not SYSTEM, nobody signed in, or it did not start (see above)");
    };
    let user = ConsoleUser::get().context("the console user")?;
    let text = "Lumen: the clip-agent works";
    println!(
        "put through the helper: {} (paste it in Notepad)",
        board.set(Content::Text(text.into()))
    );
    let end = std::time::Instant::now() + Duration::from_secs(30);
    println!("copy something now (files in Explorer, text, a picture): 30 s");
    while std::time::Instant::now() < end {
        for c in board.changes() {
            println!("seen: {}", c.describe());
            if let Content::Files(paths) = &c {
                for p in paths {
                    let r = user.act(|| read_some(p))?;
                    match r {
                        Ok((len, read)) => println!(
                            "  {}: {len} bytes, read {read} as the user — ok",
                            p.display()
                        ),
                        Err(e) => println!("  {}: as the user — {e}", p.display()),
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("done");
    Ok(())
}

/// The length of the file at `p` and how much of its first 16 MB could be
/// read (a folder: its entries).
fn read_some(p: &std::path::Path) -> std::io::Result<(u64, u64)> {
    use std::io::Read;
    if p.is_dir() {
        let n = std::fs::read_dir(p)?.count() as u64;
        return Ok((n, n));
    }
    let f = File::open(p)?;
    let len = f.metadata()?.len();
    let read = std::io::copy(&mut f.take(16 << 20), &mut std::io::sink())?;
    Ok((len, read))
}

/// `lumen-host clip-agent --pipe <base>`: the helper's side.
pub fn serve(base: &str) -> Result<()> {
    let up = OpenOptions::new()
        .write(true)
        .open(format!("{base}-up"))
        .context("the up pipe")?;
    let down = OpenOptions::new()
        .read(true)
        .open(format!("{base}-down"))
        .context("the down pipe")?;
    desktop::clipboard::serve_agent(up, down).map_err(anyhow::Error::msg)
}
