//! Lumen Host: the host's window and tray icon.
//!
//! The streaming itself is the "Lumen Host" Windows service: it runs the
//! machine's host, as SYSTEM, from boot. The registration is the machine's
//! (one token for all its Windows accounts): this app, in any account, shows
//! it (token, state, log) and changes it through the service's pipe
//! (`desktop::hostsvc`). Closing the window leaves it in the tray; "Turn off the host
//! and quit" in the tray stops the service: nobody can connect until Lumen
//! Host is started again (it starts the service) or Windows starts it with
//! itself ("Start with Windows"). At sign-in (`--hidden`, the
//! installer's entry for every user) the tray icon shows only while the host
//! is on.

#![windows_subsystem = "windows"]

mod commands;
mod settings;
mod state;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use desktop::hostsvc;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{WebviewUrl, WebviewWindowBuilder, WindowEvent};

use desktop::log::Log;
use desktop::log_path;

use crate::settings::Local;
use crate::tray::{show, turn_off_and_quit};

struct App {
    /// The user's own folder (`%APPDATA%\Lumen`): this app's settings and log.
    data: PathBuf,
    log: Arc<Log>,
    /// The footer was clicked (the host restarts and updates): when, and to which version.
    update_clicked: std::sync::Mutex<Option<(std::time::Instant, String)>>,
}

impl App {
    fn prefs_path(&self) -> PathBuf {
        self.data.join("host-app.json")
    }

    /// The window's own copy of the settings (what the service does not keep: the CA path), its language.
    fn local(&self) -> Local {
        std::fs::read_to_string(self.prefs_path())
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    fn save_local(&self, s: &Local) {
        let _ = std::fs::create_dir_all(&self.data);
        let _ = std::fs::write(
            self.prefs_path(),
            serde_json::to_string_pretty(s).expect("json"),
        );
    }
}

fn main() {
    let hidden = std::env::args().any(|a| a == "--hidden");
    let data = transport::default_dir();
    let log = Arc::new(Log::new(log_path(&data, "host.log")));
    if hidden {
        // At sign-in: the tray icon only while the host is on.
        if hostsvc::service() != hostsvc::Service::Running {
            return;
        }
    } else if hostsvc::service() == hostsvc::Service::Stopped {
        // Started by hand: the host goes on with it.
        match hostsvc::start_service() {
            Ok(()) => log.line("--- host turned on at start"),
            Err(e) => log.line(&format!("--- host not turned on: {e}")),
        }
    }
    let app_state = App {
        data,
        log,
        update_clicked: std::sync::Mutex::new(None),
    };
    let app = tauri::Builder::default()
        // A second start (the shortcut again) shows this one's window; the
        // sign-in one does not.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|a| a == "--hidden") {
                show(app)
            }
        }))
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::save,
            commands::new_token,
            commands::probe,
            commands::unregister,
            commands::reregister,
            commands::set_autostart,
            commands::set_keep_awake,
            commands::set_auto_update,
            commands::update_now,
            commands::start_host,
            commands::open_log_folder,
            commands::set_monitors,
            commands::set_client_monitors,
            commands::reset_monitors,
            tray::set_tray,
            commands::set_lang
        ])
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Lumen Host")
                .inner_size(960.0, 640.0)
                // Counted with the invisible frame (18 px at 125 %): at least 900 inside.
                .min_inner_size(920.0, 500.0)
                // Its own frame (web/host): the shadow and rounded corners stay Windows'.
                .decorations(false)
                .shadow(true)
                .visible(!hidden)
                .build()?;
            // The menu's words come from the page (set_tray); a click opens the window without them.
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().expect("icon").clone())
                .tooltip("Lumen Host")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show(app),
                    "quit" => {
                        let app = app.clone();
                        std::thread::spawn(move || turn_off_and_quit(&app));
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        show(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        // Closing the window keeps the app in the tray (the host stays on).
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("Lumen Host failed to start");
    app.run(|_, _| {});
}
