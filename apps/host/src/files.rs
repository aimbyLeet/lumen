//! The host's files in the data folder: key, token, server credentials.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use signal::Credentials;
use transport::HostToken;

pub(crate) fn key_path(arg: &Option<PathBuf>) -> PathBuf {
    arg.clone()
        .unwrap_or_else(|| transport::default_dir().join("host.key"))
}

pub(crate) fn token_path() -> PathBuf {
    transport::default_dir().join("host.token")
}

pub(crate) fn credentials_path() -> PathBuf {
    transport::default_dir().join("host.creds")
}

/// Opens an optional log or dump file for writing.
pub(crate) fn create_log(p: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        File::create(p).with_context(|| format!("create {p:?}"))?,
    ))
}

pub(crate) fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

/// The file's text, `None` if there is no file.
pub(crate) fn read_file(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// The host's token, made on first use (`new`: replaced).
pub(crate) fn load_or_create_token(new: bool) -> Result<HostToken> {
    let path = token_path();
    if !new && let Some(t) = read_file(&path)? {
        return t
            .parse()
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()));
    }
    let token = HostToken::generate();
    write_file(&path, &format!("{token}\n"))?;
    Ok(token)
}

pub(crate) fn load_credentials() -> Result<Option<Credentials>> {
    let path = credentials_path();
    read_file(&path)?
        .map(|t| {
            t.parse()
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
        })
        .transpose()
}
