//! Files of the user signed in on the console, touched on their behalf: the
//! host runs as SYSTEM, which may read anything — a file copied on the host
//! must be read only if that user may read it, and what the host writes
//! (`Downloads\Lumen`) must be theirs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    GetTokenInformation, ImpersonateLoggedOnUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, RevertToSelf,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::UI::Shell::{
    FOLDERID_Downloads, FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::{GUID, PCWSTR};

use crate::winsec::{Handle, psid_string, token_user, wide};

/// The user signed in on the console.
pub struct ConsoleUser {
    token: Handle,
    pub sid: String,
}

impl ConsoleUser {
    /// `None`: nobody is signed in (or we are not SYSTEM). The token is the
    /// one of the user's Explorer when it runs: `WTSQueryUserToken`'s may be
    /// of another signing-in of theirs (the owner's host, 2026-10-27: their
    /// network drive `Z:` not found, its share `\\192.168.100.56` access
    /// denied), and what is copied comes from Explorer.
    pub fn get() -> Option<ConsoleUser> {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        let mut token = HANDLE::default();
        unsafe { WTSQueryUserToken(session, &mut token) }.ok()?;
        // SAFETY: `token` comes from `WTSQueryUserToken`, which succeeded; the wrapper owns it now.
        let wts = unsafe { Handle::from_raw(token) };
        let sid = token_user(wts.get()).ok()?;
        let token = match shell_token(session, &sid) {
            Some((shell, pid)) => {
                note_tokens(&format!(
                    "explorer.exe {pid} ({}); WTSQueryUserToken: {}",
                    describe_token(shell.get()),
                    describe_token(wts.get())
                ));
                shell
            }
            None => {
                note_tokens(&format!(
                    "no explorer.exe in the session: WTSQueryUserToken ({})",
                    describe_token(wts.get())
                ));
                wts
            }
        };
        Some(ConsoleUser { token, sid })
    }

    fn known_folder(&self, id: &GUID) -> Result<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, Some(self.token.get()))
                .context("SHGetKnownFolderPath")?;
            let path = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            Ok(PathBuf::from(path))
        }
    }

    /// Their token (`explorer.exe`'s, else `WTSQueryUserToken`'s).
    pub fn token(&self) -> HANDLE {
        self.token.get()
    }

    /// Their `Downloads` (wherever they moved it).
    pub fn downloads(&self) -> Result<PathBuf> {
        self.known_folder(&FOLDERID_Downloads)
    }

    /// Their `%TEMP%` (`%LOCALAPPDATA%\Temp`).
    /// Starts `cmd` as the user, on their desktop, without a window.
    pub fn run(&self, cmd: &str) -> Result<Handle> {
        crate::winsec::start_on_desktop(self.token.get(), cmd, None, None)
    }

    pub fn temp(&self) -> Result<PathBuf> {
        Ok(self.known_folder(&FOLDERID_LocalAppData)?.join("Temp"))
    }

    /// Runs `f` on this thread as the user: files it opens are checked
    /// against their rights, files it creates are theirs.
    pub fn act<T>(&self, f: impl FnOnce() -> T) -> Result<T> {
        unsafe { ImpersonateLoggedOnUser(self.token.get()) }.context("ImpersonateLoggedOnUser")?;
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                unsafe {
                    let _ = RevertToSelf();
                }
            }
        }
        let _revert = Revert;
        Ok(f())
    }
}

/// A primary token like the one of `explorer.exe` of user `sid` in
/// `session`, and its process.
fn shell_token(session: u32, sid: &str) -> Option<(Handle, u32)> {
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, TOKEN_ACCESS_MASK, TOKEN_DUPLICATE, TOKEN_QUERY,
        TokenPrimary,
    };
    use windows::Win32::System::RemoteDesktop::{
        WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const MAXIMUM_ALLOWED: TOKEN_ACCESS_MASK = TOKEN_ACCESS_MASK(0x0200_0000);
    let pids: Vec<u32> = unsafe {
        let mut info: *mut WTS_PROCESS_INFOW = std::ptr::null_mut();
        let mut n = 0u32;
        WTSEnumerateProcessesW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut info, &mut n).ok()?;
        let pids = std::slice::from_raw_parts(info, n as usize)
            .iter()
            .filter(|p| {
                p.SessionId == session
                    && p.pProcessName
                        .to_string()
                        .is_ok_and(|name| name.eq_ignore_ascii_case("explorer.exe"))
            })
            .map(|p| p.ProcessId)
            .collect();
        WTSFreeMemory(info as *mut _);
        pids
    };
    pids.into_iter().find_map(|pid| unsafe {
        // SAFETY: the handle comes from `OpenProcess`; the wrapper owns it now.
        let process =
            Handle::from_raw(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut t = HANDLE::default();
        OpenProcessToken(process.get(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut t).ok()?;
        // SAFETY: `t` was opened by `OpenProcessToken` above; the wrapper owns it now.
        let t = Handle::from_raw(t);
        if token_user(t.get()).ok()? != sid {
            return None;
        }
        let mut dup = HANDLE::default();
        DuplicateTokenEx(
            t.get(),
            MAXIMUM_ALLOWED,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut dup,
        )
        .ok()?;
        // SAFETY: `dup` was made by `DuplicateTokenEx` above; the wrapper owns it now.
        Some((Handle::from_raw(dup), pid))
    })
}

/// A token's signing-in (its logon id) and elevation, for the log.
fn describe_token(token: HANDLE) -> String {
    use windows::Win32::Security::{
        TOKEN_ELEVATION_TYPE, TOKEN_STATISTICS, TokenElevationType, TokenStatistics,
    };
    let mut stats = TOKEN_STATISTICS::default();
    let mut elevation = TOKEN_ELEVATION_TYPE::default();
    let mut len = 0u32;
    unsafe {
        let _ = GetTokenInformation(
            token,
            TokenStatistics,
            Some(&mut stats as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_STATISTICS>() as u32,
            &mut len,
        );
        let _ = GetTokenInformation(
            token,
            TokenElevationType,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut len,
        );
    }
    let id = stats.AuthenticationId;
    let kind = match elevation.0 {
        1 => "no split token",
        2 => "elevated",
        3 => "limited",
        _ => "?",
    };
    format!("logon {:x}:{:08x}, {kind}", id.HighPart, id.LowPart)
}

/// Logs which token the host acts as, when it changes (asked every second).
fn note_tokens(what: &str) {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let mut last = LAST.lock().unwrap();
    if *last != what {
        println!("files: as the console user: {what}");
        *last = what.to_string();
    }
}

/// Where a session's received files go, and as whom: the signed-in user's
/// `Downloads\Lumen` as them when this is SYSTEM (the service); nobody
/// signed in, nowhere; run by a user (development), theirs. Windows locked:
/// nowhere, until a sign-in (the owner's choice 2026-10-27).
pub fn place(downloads: Option<&Path>) -> desktop::files::Place {
    use desktop::files::{Place, UserToken};
    if crate::netinfo::console_locked() {
        return Place::Locked;
    }
    if let Some(d) = downloads {
        return Place::Here {
            folder: d.to_path_buf(),
            temp: d.join("clip-temp"),
        };
    }
    if !is_system() {
        return match desktop::files::downloads(None) {
            Ok(folder) => Place::Here {
                folder,
                temp: std::env::temp_dir(),
            },
            Err(e) => {
                eprintln!("files: no Downloads folder: {e}");
                Place::Nobody
            }
        };
    }
    let Some(user) = ConsoleUser::get() else {
        return Place::Nobody;
    };
    let temp = match user.temp() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("files: no temp folder of the user: {e}");
            return Place::Nobody;
        }
    };
    let ConsoleUser { token, .. } = user;
    let raw = token.get();
    std::mem::forget(token);
    let user = unsafe { UserToken::from_raw(raw) };
    match desktop::files::downloads(Some(&user)) {
        Ok(folder) => Place::As { user, folder, temp },
        Err(e) => {
            eprintln!("files: no Downloads folder of the user: {e}");
            Place::Nobody
        }
    }
}

/// The SID of a file's owner.
pub fn owner(path: &Path) -> Result<String> {
    let w = wide(&path.to_string_lossy());
    let mut sid = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        let e = GetNamedSecurityInfoW(
            PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut sid),
            None,
            None,
            None,
            &mut sd,
        );
        if e.is_err() {
            bail!("GetNamedSecurityInfo: {e:?}");
        }
        let s = psid_string(sid);
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        s
    }
}

/// `lumen-host clip-probe --system`: what the host as SYSTEM can do with the
/// console user's files (a probe). Files copied in
/// Explorer beforehand are read as the user, copied to their
/// `Downloads\Lumen` and `%TEMP%`, and the `%TEMP%` copies are put on the
/// clipboard for a paste in Explorer.
pub fn system_probe(given: Vec<PathBuf>) -> Result<()> {
    println!(
        "process account: {}; {}",
        process_sid(),
        crate::user_files::where_we_are()
    );
    let Some(user) = ConsoleUser::get() else {
        bail!("nobody is signed in on the console (or this is not SYSTEM)");
    };
    println!("console user: {}", user.sid);
    let downloads = user.downloads()?.join("Lumen");
    let temp = user.temp()?.join("Lumen").join("clip").join("probe");
    println!(
        "downloads: {}\ntemp: {}",
        downloads.display(),
        temp.display()
    );

    // A file only SYSTEM may read: the user must not get it.
    let denied = PathBuf::from(r"C:\ProgramData\Lumen\probe-denied.txt");
    write_system_only(&denied)?;
    let as_system = std::fs::read(&denied).map(|b| b.len());
    let as_user = user.act(|| std::fs::read(&denied).map(|b| b.len()))?;
    println!("SYSTEM-only file: as SYSTEM {as_system:?}; as the user {as_user:?}");
    let _ = std::fs::remove_file(&denied);
    let denied_ok = as_system.is_ok()
        && matches!(&as_user, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied);

    println!("{}", desktop::clip_probe::formats());
    let files = if given.is_empty() {
        desktop::clip_probe::read_files().map_err(anyhow::Error::msg)?
    } else {
        given
    };
    if files.is_empty() {
        println!("no files on the clipboard: copy some in Explorer first, then run again");
    }
    let mut copies = Vec::new();
    let mut all_ok = denied_ok;
    for f in &files {
        let name = f.file_name().unwrap_or_default();
        let r = user.act(|| -> std::io::Result<(u64, u64, PathBuf)> {
            let size = std::fs::metadata(f)?.len();
            if !f.is_file() {
                return Err(std::io::Error::other("a folder: not in this probe"));
            }
            std::fs::create_dir_all(&downloads)?;
            std::fs::create_dir_all(&temp)?;
            let d = downloads.join(name);
            let read = std::fs::copy(f, &d)?;
            let t = temp.join(name);
            std::fs::copy(f, &t)?;
            Ok((size, read, t))
        })?;
        match r {
            Ok((size, read, t)) => {
                let d = downloads.join(name);
                let od = owner(&d).unwrap_or_else(|e| e.to_string());
                let ot = owner(&t).unwrap_or_else(|e| e.to_string());
                let mine = od == user.sid && ot == user.sid && size == read;
                all_ok &= mine;
                println!(
                    "{}: {size} bytes, copied {read}; owner in Downloads {od}, in temp {ot} — {}",
                    f.display(),
                    if mine { "ok" } else { "NOT the user's" }
                );
                copies.push(t);
            }
            Err(e) => println!("{}: as the user — {e}", f.display()),
        }
    }
    if !copies.is_empty() {
        desktop::clip_probe::put_files(&copies).map_err(anyhow::Error::msg)?;
        println!(
            "on the clipboard: {} file(s) from temp — press Ctrl+V in an Explorer folder: they must appear",
            copies.len()
        );
    }
    println!(
        "{}",
        if all_ok {
            "RESULT: ok"
        } else {
            "RESULT: something is wrong (see above)"
        }
    );
    Ok(())
}

/// This process is SYSTEM (the service's host).
pub fn is_system() -> bool {
    process_sid() == crate::winsec::SYSTEM_SID
}

pub fn process_sid() -> String {
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut t = HANDLE::default();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t).is_err() {
            return "?".into();
        }
    }
    // SAFETY: `t` was opened by `OpenProcessToken` above (checked); the wrapper owns it now.
    let t = unsafe { Handle::from_raw(t) };
    token_user(t.get()).unwrap_or_else(|e| e.to_string())
}

/// `lumen-host clip-probe --formats`: what this process sees on the
/// clipboard without COM, then with OLE initialized on this thread.
pub fn formats_probe() {
    println!("{}", where_we_are());
    let files = |when: &str| {
        println!("{when}: {}", desktop::clip_probe::formats());
        match desktop::clip_probe::read_files() {
            Ok(f) => println!("{when}: files {f:?}"),
            Err(e) => println!("{when}: files — {e}"),
        }
    };
    files("no COM");
    // The text the way the host's session reads it (its clipboard thread).
    match desktop::clipboard::Clipboard::start(true) {
        Ok(b) => {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let t = b.changes();
            println!(
                "host clipboard thread: {:?}",
                t.iter().map(|c| c.describe()).collect::<Vec<_>>()
            );
        }
        Err(e) => println!("host clipboard thread: {e}"),
    }
    let r = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
    println!("OleInitialize: {r:?}");
    files("OLE");
}

/// Our session and window station (each has a clipboard of its own).
pub fn where_we_are() -> String {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_NAME,
    };
    let mut session = u32::MAX;
    let mut name = [0u16; 128];
    unsafe {
        let _ = ProcessIdToSessionId(std::process::id(), &mut session);
        if let Ok(ws) = GetProcessWindowStation() {
            let _ = GetUserObjectInformationW(
                HANDLE(ws.0),
                UOI_NAME,
                Some(name.as_mut_ptr().cast()),
                (name.len() * 2) as u32,
                None,
            );
        }
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    format!(
        "session {session} (console {}), window station {}",
        unsafe { WTSGetActiveConsoleSessionId() },
        String::from_utf16_lossy(&name[..end])
    )
}

/// Creates `path` readable by SYSTEM only.
fn write_system_only(path: &Path) -> Result<()> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let sec = crate::winsec::Security::from_sddl("D:P(A;;FA;;;SY)")?;
    let w = wide(&path.to_string_lossy());
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ,
            Some(&sec.attrs),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .context("CreateFileW")?;
    let mut f = unsafe { std::fs::File::from_raw_handle(h.0) };
    std::io::Write::write_all(&mut f, b"only SYSTEM may read this")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This user's Explorer, as the service finds the console user's: run
    /// by hand in a signed-in session,
    /// `cargo test -p lumen-host -- --ignored shell_of_this_user`.
    #[test]
    #[ignore]
    fn shell_of_this_user() {
        use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
        let mut session = 0u32;
        unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }.unwrap();
        let sid = process_sid();
        let (token, pid) = shell_token(session, &sid).expect("explorer.exe of this user");
        assert_eq!(token_user(token.get()).unwrap(), sid);
        println!("explorer.exe {pid}: {}", describe_token(token.get()));
        // Usable the way the files thread uses it.
        let user = ConsoleUser { token, sid };
        let temp = user.act(|| std::env::temp_dir().exists()).unwrap();
        assert!(temp);
        assert!(user.downloads().is_ok());
    }
}
