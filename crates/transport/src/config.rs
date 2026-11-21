//! Where a device keeps its files, and its config file.
//!
//! Everything (keys, server credentials, token, config) lives in the Windows
//! user's own `%APPDATA%\Lumen`, never next to the program: each account of a
//! machine is its own device on the server.
//!
//! The config file (`host.ini`, `client.ini`) holds default command-line
//! arguments as `name = value` lines: `server = lumen.example.com:7443` means
//! `--server lumen.example.com:7443`, `borderless = true` means `--borderless`.
//! `[section]` headers and lines starting with `#` or `;` are ignored.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// `$LUMEN_DATA` if set (tests: several devices on one machine), else
/// `%APPDATA%\Lumen` (Windows), `$HOME/.config/lumen` elsewhere, else the
/// current directory.
pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("LUMEN_DATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        return PathBuf::from(d).join("Lumen");
    }
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".config").join("lumen");
    }
    PathBuf::from(".")
}

/// Command-line arguments from the config file; none if it does not exist.
pub fn config_args(path: &Path) -> io::Result<Vec<OsString>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config(&text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// `args[0]`, then the config's arguments, then the rest of `args`: parsed
/// with `args_override_self`, the command line wins over the config.
pub fn with_config_args(args: Vec<OsString>, config: Vec<OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    args.next().into_iter().chain(config).chain(args).collect()
}

fn parse_config(text: &str) -> Result<Vec<OsString>, String> {
    let mut args = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(['#', ';']) || line.starts_with('[') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `name = value`", n + 1))?;
        let name = name.trim().replace('_', "-");
        let value = value.trim().trim_matches('"');
        if name.is_empty() || name.starts_with('-') {
            return Err(format!("line {}: bad name", n + 1));
        }
        match value {
            "true" => args.push(format!("--{name}").into()),
            // An empty value (e.g. a field left blank in the installer) or
            // `false` leaves the default.
            "false" | "" => {}
            v => {
                args.push(format!("--{name}").into());
                args.push(v.into());
            }
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: Vec<OsString>) -> Vec<String> {
        v.into_iter().map(|s| s.into_string().unwrap()).collect()
    }

    #[test]
    fn config_lines_become_arguments() {
        let text = "\u{feff}[lumen]\n# comment\n; comment\nserver = lumen.example.com:7443\n\
                    login=anna\nserver_ca =\nborderless = true\nvsync-loop = false\nhost = \"studio\"\n";
        assert_eq!(
            strings(parse_config(text).unwrap()),
            [
                "--server",
                "lumen.example.com:7443",
                "--login",
                "anna",
                "--borderless",
                "--host",
                "studio"
            ]
        );
        assert!(parse_config("server lumen\n").is_err());
        assert!(parse_config("--server = x\n").is_err());
    }

    #[test]
    fn command_line_comes_after_config() {
        let args = with_config_args(
            vec!["exe".into(), "--fps".into(), "30".into()],
            vec!["--fps".into(), "60".into()],
        );
        assert_eq!(strings(args), ["exe", "--fps", "60", "--fps", "30"]);
    }

    #[test]
    fn missing_config_is_empty() {
        let p = std::env::temp_dir().join("lumen-no-such-config.ini");
        assert!(config_args(&p).unwrap().is_empty());
    }
}
