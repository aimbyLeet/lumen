//! What Lumen Host and Lumen Client share of updating from the server
//!: what their footer shows, and the installer run
//! silently with something to do after it.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

use serde::Serialize;

/// `CREATE_NO_WINDOW`.
pub const NO_WINDOW: u32 = 0x0800_0000;
/// `CREATE_NEW_PROCESS_GROUP`: not tied to this program's console.
pub const NEW_GROUP: u32 = 0x0000_0200;
/// `DETACHED_PROCESS`.
pub const DETACHED: u32 = 0x0000_0008;
/// `CREATE_BREAKAWAY_FROM_JOB`: out of a job the starting program may be in.
pub const BREAKAWAY: u32 = 0x0100_0000;

/// What the footer and the update's window show (web/client/src/Update.tsx, `UpdateInfo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UpdateView {
    /// The server's version ("": not known yet, or no installer there).
    pub server_version: String,
    /// It is higher than ours: the footer is yellow.
    pub newer: bool,
    /// "" — nothing under way; "downloading"; "installing" (the program closes).
    pub phase: &'static str,
    /// The last try failed: its code (`err_update_…`).
    pub error: String,
}

/// Which part the installer sets up (its `/TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Lumen Host, for all users (the service runs it).
    Host,
    /// Lumen Client, for this user (no UAC).
    Client,
}

/// `cmd.exe`'s arguments: the installer `setup`, silent, its log in `log`,
/// then `then` whatever the installer did.
pub fn installer_line(setup: &Path, part: Part, log: &Path, then: &str) -> String {
    let part = match part {
        Part::Host => "/ALLUSERS /TYPE=host",
        Part::Client => "/TYPE=client",
    };
    format!(
        r#"/c ""{}" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART {part} "/LOG={}" & {then}""#,
        setup.display(),
        log.display(),
    )
}

/// Starts `cmd.exe` (`%ComSpec%`) with this line as it is, with these creation flags.
pub fn start_cmd(line: &str, flags: u32) -> io::Result<Child> {
    Command::new(std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()))
        .raw_arg(line)
        .creation_flags(flags)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_as_before() {
        assert_eq!(
            installer_line(
                Path::new(r"C:\u\lumen-setup-0.7.1.exe"),
                Part::Host,
                Path::new(r"C:\u\install.log"),
                "sc.exe start LumenHost"
            ),
            r#"/c ""C:\u\lumen-setup-0.7.1.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /ALLUSERS /TYPE=host "/LOG=C:\u\install.log" & sc.exe start LumenHost""#
        );
        assert_eq!(
            installer_line(
                Path::new(r"C:\u\lumen-setup-0.7.1.exe"),
                Part::Client,
                Path::new(r"C:\u\install.log"),
                r#"start "" "C:\p\lumen-client-app.exe""#
            ),
            r#"/c ""C:\u\lumen-setup-0.7.1.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /TYPE=client "/LOG=C:\u\install.log" & start "" "C:\p\lumen-client-app.exe"""#
        );
    }
}
