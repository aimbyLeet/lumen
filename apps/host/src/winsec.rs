//! Windows bits the service needs: account SIDs, security descriptors from
//! SDDL, handles, and starting a process on the console user's desktop.

use std::ffi::c_void;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOW,
};
use windows::core::{PCWSTR, PWSTR};

/// LocalSystem: the account of the service and its hosts.
pub const SYSTEM_SID: &str = "S-1-5-18";
/// BUILTIN\Administrators.
pub const ADMINS_SID: &str = "S-1-5-32-544";

/// A NUL-terminated UTF-16 string, as the `W` functions take it.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A handle closed when dropped.
pub use desktop::Handle;

/// Starts `cmd` with `token` on `winsta0\default` (the desktop of the
/// session the token is for), without a window. `output`: an inheritable
/// handle for its stdout and stderr; `env`: a `CREATE_UNICODE_ENVIRONMENT`
/// block, else ours. Returns the process.
pub fn start_on_desktop(
    token: HANDLE,
    cmd: &str,
    output: Option<HANDLE>,
    env: Option<&[u16]>,
) -> Result<Handle> {
    let mut cmd = wide(cmd);
    let mut desktop = wide(r"winsta0\default");
    let mut si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    if let Some(out) = output {
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdOutput = out;
        si.hStdError = out;
    }
    let mut flags = CREATE_NO_WINDOW;
    if env.is_some() {
        flags |= CREATE_UNICODE_ENVIRONMENT;
    }
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessAsUserW(
            Some(token),
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            output.is_some(),
            flags,
            env.map(|e| e.as_ptr() as *const c_void),
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .context("CreateProcessAsUserW")?;
    }
    // SAFETY: both handles come from `CreateProcessAsUserW` just above; the wrappers own them now.
    drop(unsafe { Handle::from_raw(pi.hThread) });
    Ok(unsafe { Handle::from_raw(pi.hProcess) })
}

/// Starts `cmd` as the token's user on their desktop, with their own
/// environment (`%APPDATA%`, `%LOCALAPPDATA%`: theirs, not SYSTEM's) — a
/// program of theirs, such as Lumen Host after an update.
pub fn start_as_user(token: HANDLE, cmd: &str) -> Result<Handle> {
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    let mut block: *mut c_void = std::ptr::null_mut();
    unsafe { CreateEnvironmentBlock(&mut block, Some(token), false) }
        .context("CreateEnvironmentBlock")?;
    // The block: NUL-separated strings, ending with two NULs.
    let env: Vec<u16> = unsafe {
        let p = block as *const u16;
        let mut n = 0;
        while !(*p.add(n) == 0 && *p.add(n + 1) == 0) {
            n += 1;
        }
        std::slice::from_raw_parts(p, n + 2).to_vec()
    };
    unsafe {
        let _ = DestroyEnvironmentBlock(block);
    }
    start_on_desktop(token, cmd, None, Some(&env))
}

/// The account a token belongs to, as an SID string (`S-1-5-21-...`).
pub fn token_user(token: HANDLE) -> Result<String> {
    let mut buf = vec![0u8; 256];
    let mut len = 0u32;
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            buf.len() as u32,
            &mut len,
        )
        .context("GetTokenInformation(TokenUser)")?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        psid_string(user.User.Sid)
    }
}

pub fn psid_string(sid: PSID) -> Result<String> {
    let mut s = PWSTR::null();
    unsafe {
        ConvertSidToStringSidW(sid, &mut s).context("ConvertSidToStringSid")?;
        let text = s.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        Ok(text)
    }
}

/// An SID string of a user account.
pub fn is_user_sid(sid: &str) -> bool {
    sid.starts_with("S-1-5-21-") && sid[9..].chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Ctrl+Alt+Del on this machine, as if pressed (`SendSAS` of sas.dll). Works
/// only in a service, and only if the policy `SoftwareSASGeneration` lets
/// services do it (the installer sets it).
pub fn send_sas() -> Result<()> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{s, w};
    let policy = sas_policy();
    if policy & 1 == 0 {
        anyhow::bail!(
            "Windows ignores it: the policy SoftwareSASGeneration ({policy}) does not let services \
             send Ctrl+Alt+Del (reinstall Lumen, or set it to 1)"
        );
    }
    unsafe {
        let lib = LoadLibraryW(w!("sas.dll")).context("load sas.dll")?;
        let f = GetProcAddress(lib, s!("SendSAS")).context("SendSAS in sas.dll")?;
        // VOID SendSAS(BOOL AsUser): FALSE — as a service.
        let send: unsafe extern "system" fn(i32) = std::mem::transmute(f);
        send(0);
    }
    Ok(())
}

/// `HKLM\...\Policies\System\SoftwareSASGeneration`: 1 or 3 let services send Ctrl+Alt+Del.
fn sas_policy() -> u32 {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut v = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"),
            w!("SoftwareSASGeneration"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut v).cast()),
            Some(&mut len),
        )
    };
    if r.is_ok() { v } else { 0 }
}

/// A security descriptor made from SDDL, with `SECURITY_ATTRIBUTES` over it.
pub struct Security {
    sd: PSECURITY_DESCRIPTOR,
    pub attrs: SECURITY_ATTRIBUTES,
}

impl Security {
    pub fn from_sddl(sddl: &str) -> Result<Self> {
        let w = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(w.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .with_context(|| format!("security descriptor {sddl}"))?;
        }
        Ok(Security {
            sd,
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            },
        })
    }

    pub fn descriptor(&self) -> PSECURITY_DESCRIPTOR {
        self.sd
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.sd.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_sids_only() {
        assert!(is_user_sid("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(!is_user_sid(SYSTEM_SID));
        assert!(!is_user_sid(r"S-1-5-21-1\..\x"));
        assert!(!is_user_sid("test"));
    }

    #[test]
    fn sddl_parses() {
        Security::from_sddl("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)").unwrap();
        assert!(Security::from_sddl("D:(garbage").is_err());
    }
}
