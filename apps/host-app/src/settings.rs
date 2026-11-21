//! The app's own settings: what the user sets in the window and what
//! `host-app.json` keeps.

use std::path::Path;

use desktop::computer_name;
use desktop::hostsvc;
use serde::{Deserialize, Serialize};

/// What the user sets in the window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// `host:port` of the Lumen server.
    pub(crate) server: String,
    pub(crate) machine: String,
    /// Announce these `ip:port` (comma separated) instead of the machine's addresses.
    pub(crate) manual_addrs: String,
    /// Development: the CA of the server's certificate (PEM file). The service
    /// keeps its text; the path stays here (`host-app.json`) for the window.
    pub(crate) server_ca: String,
    /// The key the user trusted, of a server with its own certificate (a
    /// local network; `signal::pin`), or empty.
    pub(crate) server_fingerprint: String,
}

/// What `host-app.json` keeps: the settings' copy and the window's language.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Local {
    #[serde(flatten)]
    pub(crate) settings: Settings,
    /// `desktop::lang`: this Windows user's choice.
    pub(crate) lang: String,
}

impl Default for Local {
    fn default() -> Self {
        Local {
            settings: Settings::default(),
            lang: desktop::lang::default(),
        }
    }
}

/// The name offered for a new machine: the computer's, when it is one the server takes (Latin only); else
/// none — the user types one.
pub(crate) fn default_machine() -> String {
    let name = computer_name();
    if signal::name_allowed(&name) {
        name
    } else {
        String::new()
    }
}

/// The machine's name: Latin only (owner's decision 2026-11-09), the server's own rule.
pub(crate) fn check_name(s: &str) -> Result<String, String> {
    if !signal::name_allowed(s) {
        return Err("err_machine_name".into());
    }
    Ok(s.trim().to_string())
}

/// The window's settings → what the service keeps (the CA file read here, as the user).
pub(crate) fn to_service(s: &Settings) -> Result<hostsvc::Settings, String> {
    let ca = s.server_ca.trim();
    Ok(hostsvc::Settings {
        server: signal::server_address(&s.server)?,
        server_ca_pem: if ca.is_empty() {
            String::new()
        } else {
            std::fs::read_to_string(Path::new(ca)).map_err(|e| format!("err_ca_file|{ca}: {e}"))?
        },
        server_fingerprint: s.server_fingerprint.trim().to_string(),
        machine: check_name(&s.machine)?,
        manual_addrs: s.manual_addrs.trim().to_string(),
    })
}
