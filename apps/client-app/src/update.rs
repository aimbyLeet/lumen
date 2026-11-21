//! Updating Lumen Client from its server:
//! the server's version comes through the update door (`signal::update`) at
//! start, after the server is set and whenever the link to it is made again.
//! A newer one makes the footer yellow; it is installed at start (the
//! «Update automatically» switch) or on the footer's click: downloaded,
//! checked (SHA-256, a higher version), and the installer run silently for
//! this user (no UAC); this program closes, and starts again after it.

use std::path::PathBuf;
use std::time::Duration;

use desktop::update::{NEW_GROUP, NO_WINDOW, Part, UpdateView, installer_line, start_cmd};
use signal::APP_VERSION;
use signal::update::{Offer, download, query};

use crate::App;

/// How long the start waits for the server's answer (then it starts as it is).
pub(crate) const START_CHECK: Duration = Duration::from_secs(3);
/// A check later on, and each wait of a download.
const CHECK: Duration = Duration::from_secs(10);

/// The server's version and this program's update.
#[derive(Debug, Default)]
pub(crate) struct UpdateLive {
    /// What the footer and the update's window show.
    pub(crate) shown: UpdateView,
    /// What the server offered last.
    pub(crate) offer: Option<Offer>,
}

impl App {
    /// Where installers are downloaded: `%LOCALAPPDATA%\Lumen\updates` (another
    /// `LUMEN_DATA`: its own `updates`, checks on one machine).
    fn updates_dir(&self) -> PathBuf {
        match std::env::var_os("LOCALAPPDATA") {
            Some(local) if std::env::var_os("LUMEN_DATA").is_none() => {
                PathBuf::from(local).join("Lumen").join("updates")
            }
            _ => self.data.join("updates"),
        }
    }

    /// Asks the server what it has; keeps the answer. None: no server set,
    /// no answer, or nothing there.
    pub(crate) fn check_update(&self, timeout: Duration) -> Option<Offer> {
        let cfg = self.server_config().ok()?;
        let offer = match query(&cfg.server, &cfg.trust, timeout) {
            Ok(o) => o,
            Err(e) => {
                self.log
                    .line(&format!("update: {} did not say ({e})", cfg.server));
                return None;
            }
        };
        let mut u = self.update.lock().unwrap();
        u.shown.server_version = offer
            .as_ref()
            .map(|o| o.version.clone())
            .unwrap_or_default();
        u.shown.newer = offer.as_ref().is_some_and(Offer::is_newer);
        if u.shown.newer && u.offer.as_ref() != offer.as_ref() {
            self.log.line(&format!(
                "update: the server has Lumen {}, this is {APP_VERSION}",
                u.shown.server_version
            ));
        }
        u.offer = offer.clone();
        offer
    }

    /// At start: a newer version is installed at once if the user lets it
    /// (and it was not tried already: an installer that does not make this
    /// program newer is not run again and again). True: this program closes.
    pub(crate) fn update_at_start(&self) -> bool {
        let Some(offer) = self.check_update(START_CHECK) else {
            return false;
        };
        let (auto, tried) = {
            let s = self.settings.lock().unwrap();
            (s.auto_update, s.update_tried.clone())
        };
        if !offer.is_newer() || !auto || tried == offer.version {
            return false;
        }
        let mut s = self.settings.lock().unwrap();
        s.update_tried = offer.version.clone();
        self.keep_settings(s);
        self.install_update().is_ok()
    }

    /// Downloads the server's newer version, checks it, starts its installer
    /// (this program starts again after it). Ok: this program must close now.
    pub(crate) fn install_update(&self) -> Result<(), String> {
        let offer = {
            let mut u = self.update.lock().unwrap();
            if !u.shown.phase.is_empty() {
                return Err("err_update_running".into());
            }
            let Some(offer) = u.offer.clone().filter(Offer::is_newer) else {
                return Err("err_update_none".into());
            };
            u.shown.phase = "downloading";
            u.shown.error.clear();
            offer
        };
        let result = self.download_and_run(&offer);
        let mut u = self.update.lock().unwrap();
        match &result {
            Ok(()) => u.shown.phase = "installing",
            Err(code) => {
                u.shown.phase = "";
                u.shown.error = code.clone();
            }
        }
        result
    }

    fn download_and_run(&self, offer: &Offer) -> Result<(), String> {
        let setup = self.fetch(offer)?;
        self.run_installer(offer, &setup)
    }

    /// The offer's installer, downloaded (or resumed) and checked.
    pub(crate) fn fetch(&self, offer: &Offer) -> Result<PathBuf, String> {
        let cfg = self.server_config()?;
        self.log.line(&format!(
            "update: downloading Lumen {} from {}",
            offer.version, cfg.server
        ));
        download(&cfg.server, &cfg.trust, offer, &self.updates_dir(), CHECK).map_err(|e| {
            self.log.line(&format!("update: {e}"));
            e.code().to_string()
        })
    }

    fn run_installer(&self, offer: &Offer, setup: &std::path::Path) -> Result<(), String> {
        // The video window goes first: the installer replaces it too.
        if let Some(v) = self.video.lock().unwrap().take() {
            v.stop();
        }
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        // The installer, then this program again, whatever the installer did
        // (it closes this one itself if it is still there: Restart Manager).
        let line = installer_line(
            setup,
            Part::Client,
            &self.updates_dir().join("install.log"),
            &format!(r#"start "" "{}""#, me.display()),
        );
        // Not tied to this process: it is about to exit.
        start_cmd(&line, NO_WINDOW | NEW_GROUP).map_err(|e| {
            self.log
                .line(&format!("update: the installer did not start: {e}"));
            "err_update_run".to_string()
        })?;
        self.log.line(&format!(
            "update: installing Lumen {} ({}), this program closes",
            offer.version,
            setup.display()
        ));
        Ok(())
    }
}

/// Against a real server on loopback with a test installer: the version
/// known, the installer downloaded whole; what the start does not do again.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use desktop::log::Log;
    use lumen_server::update::UpdateFile;
    use lumen_server::{Config, Server, dev_cert, tls_config};

    use super::*;

    #[test]
    fn the_server_has_a_newer_version() {
        let dir = std::env::temp_dir().join(format!("lumen-client-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("offer")).unwrap();
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("offer").join("lumen-setup-99.0.0.exe"), &data).unwrap();
        let cert = dev_cert(&["localhost".into(), "127.0.0.1".into()]).unwrap();
        std::fs::write(dir.join("ca.pem"), &cert.ca_pem).unwrap();
        let server = Server::start(Config {
            update: UpdateFile::find(&dir.join("offer")).unwrap(),
            ..Config::new(
                "127.0.0.1:0".parse().unwrap(),
                dir.join("lumen.db"),
                tls_config(cert.cert_pem.as_bytes(), cert.key_pem.as_bytes()).unwrap(),
            )
        })
        .unwrap();
        let data_dir = dir.join("app");
        // Its own downloads folder (`LUMEN_DATA`), not the user's.
        unsafe { std::env::set_var("LUMEN_DATA", &data_dir) };
        let app = App::open(
            data_dir.clone(),
            Arc::new(Log::new(data_dir.join("client.log"))),
        );
        app.settings.lock().unwrap().server_ca = dir.join("ca.pem").display().to_string();

        // No server yet: nothing known.
        assert!(app.check_update(START_CHECK).is_none());
        app.set_identity("anna".into(), server.addr().to_string())
            .unwrap();
        let offer = app.check_update(START_CHECK).unwrap();
        let u = app.update.lock().unwrap().shown.clone();
        assert_eq!((u.server_version.as_str(), u.newer), ("99.0.0", true));

        // Not let to update at start: nothing done.
        app.settings.lock().unwrap().auto_update = false;
        assert!(!app.update_at_start());
        // Let, but this version was tried already: not again.
        {
            let mut s = app.settings.lock().unwrap();
            s.auto_update = true;
            s.update_tried = "99.0.0".into();
        }
        assert!(!app.update_at_start());
        assert_eq!(app.update.lock().unwrap().shown.phase, "");

        // The installer, whole, in the downloads folder.
        let setup = app.fetch(&offer).unwrap();
        assert_eq!(std::fs::read(&setup).unwrap(), data);
        assert!(setup.starts_with(data_dir.join("updates")));

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
