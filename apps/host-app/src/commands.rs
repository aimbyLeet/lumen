//! The Tauri commands of the window.

use std::path::Path;

use desktop::hostsvc::{self, Request};
use tauri::State;

use crate::App;
use crate::settings::{Local, Settings, default_machine, to_service};
use crate::state::{AppState, monitors_of, shown};

#[tauri::command(async)]
pub(crate) fn get_state(app: State<'_, App>) -> AppState {
    let local = app.local();
    match hostsvc::call(&Request::State) {
        Ok(st) => {
            let monitors = monitors_of(Some(&st));
            let clicked = app.update_clicked.lock().unwrap().clone();
            AppState::of_service(
                st,
                local.settings,
                default_machine(),
                monitors,
                local.lang,
                clicked,
            )
        }
        Err(e) => AppState::without_service(
            e,
            hostsvc::service() == hostsvc::Service::Stopped,
            default_machine(),
            monitors_of(None),
            local.lang,
        ),
    }
}

/// Registers the machine's host, or changes its settings; the service (re)starts it.
#[tauri::command(async)]
pub(crate) fn save(app: State<'_, App>, settings: Settings) -> Result<(), String> {
    let wanted = to_service(&settings)?;
    hostsvc::call(&Request::Set {
        settings: wanted.clone(),
    })?;
    app.save_local(&Local {
        settings: Settings {
            server: wanted.server.clone(),
            ..settings
        },
        ..app.local()
    });
    app.log.line(&format!(
        "--- settings saved: server {}, {}",
        wanted.server, wanted.machine
    ));
    Ok(())
}

/// A new token: clients with the old one cannot find this host any more.
#[tauri::command(async)]
pub(crate) fn new_token(app: State<'_, App>) -> Result<String, String> {
    let st = hostsvc::call(&Request::NewToken)?;
    app.log.line("--- a new token was made");
    Ok(st.token)
}

/// Whether the server in the form answers (before registering: the button waits for it).
/// A server with its own certificate not trusted yet: `err_cert_untrusted|<fingerprint>`.
#[tauri::command(async)]
pub(crate) fn probe(
    server: String,
    server_ca: String,
    server_fingerprint: String,
) -> Result<(), String> {
    let server = signal::server_address(&server)?;
    let (ca, fp) = (server_ca.trim(), server_fingerprint.trim());
    let trust = signal::Trust::choose(
        (!ca.is_empty()).then_some(Path::new(ca)),
        (!fp.is_empty()).then_some(fp),
    )
    .map_err(|e| e.to_string())?;
    // A certificate's question and a bad fingerprint come as their codes (the last arm).
    signal::probe(&signal::ServerConfig { server, trust }).map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "err_probe_timeout".into(),
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset => {
            "err_probe_closed".into()
        }
        _ => e.to_string(),
    })
}

/// "Remove registration": the registration goes here and, if it answers, on the server; the token stays.
#[tauri::command(async)]
pub(crate) fn unregister(app: State<'_, App>) -> Result<String, String> {
    let st = hostsvc::call(&Request::Unregister)?;
    app.log
        .line(&format!("--- registration removed: {}", st.note));
    Ok(st.note)
}

/// Forgets the registration on the server (e.g. the admin removed it): the host registers anew.
#[tauri::command(async)]
pub(crate) fn reregister(app: State<'_, App>) -> Result<(), String> {
    hostsvc::call(&Request::Reregister)?;
    app.log.line("--- registration dropped: registering anew");
    Ok(())
}

/// The window's language (and the tray menu's: the page sets it anew).
#[tauri::command]
pub(crate) fn set_lang(app: State<'_, App>, lang: String) -> Result<(), String> {
    desktop::lang::check(&lang)?;
    app.save_local(&Local {
        lang,
        ..app.local()
    });
    Ok(())
}

/// Whether the host starts with Windows (the service's start type).
#[tauri::command(async)]
pub(crate) fn set_autostart(app: State<'_, App>, enabled: bool) -> Result<(), String> {
    hostsvc::call(&Request::SetAutostart { on: enabled })?;
    app.log.line(&format!(
        "--- start with Windows: {}",
        if enabled { "on" } else { "off" }
    ));
    Ok(())
}

/// Whether the service keeps the computer from sleeping.
#[tauri::command(async)]
pub(crate) fn set_keep_awake(app: State<'_, App>, enabled: bool) -> Result<(), String> {
    hostsvc::call(&Request::SetKeepAwake { on: enabled })?;
    app.log.line(&format!(
        "--- keep awake: {}",
        if enabled { "on" } else { "off" }
    ));
    Ok(())
}

/// "Update when a newer client connects".
#[tauri::command(async)]
pub(crate) fn set_auto_update(app: State<'_, App>, enabled: bool) -> Result<(), String> {
    hostsvc::call(&Request::SetAutoUpdate { on: enabled })?;
    app.log.line(&format!(
        "--- update by itself: {}",
        if enabled { "on" } else { "off" }
    ));
    Ok(())
}

/// The footer's click, confirmed: the host restarts and updates; the
/// installer closes this window, the service opens it again after.
#[tauri::command(async)]
pub(crate) fn update_now(app: State<'_, App>) -> Result<(), String> {
    let st = hostsvc::call(&Request::UpdateNow)?;
    *app.update_clicked.lock().unwrap() =
        Some((std::time::Instant::now(), st.status.server_version));
    app.log
        .line("--- update now: the host restarts and updates");
    Ok(())
}

/// Turns the host on (the service was stopped).
#[tauri::command(async)]
pub(crate) fn start_host(app: State<'_, App>) -> Result<(), String> {
    hostsvc::start_service()?;
    app.log.line("--- host turned on");
    Ok(())
}

/// "Create" / "Apply" / "Turn off" / "Turn on": the service makes the
/// virtual monitors so and arranges them before it answers.
#[tauri::command(async)]
pub(crate) fn set_monitors(
    app: State<'_, App>,
    settings: vdd::layout::Settings,
) -> Result<(), String> {
    vdd::layout::check(&settings).map_err(|e| e.to_string())?;
    // Turning them off is always allowed.
    if settings.enabled {
        let real = vdd::display::real().map_err(|e| e.to_string())?;
        let allowed = vdd::layout::virtuals_allowed(shown(&real));
        if settings.virtuals.len() > allowed {
            return Err(format!(
                "err_monitors_limit|{}|{allowed}",
                vdd::layout::MAX_MONITORS
            ));
        }
    }
    let line = serde_json::to_string(&settings).unwrap_or_default();
    hostsvc::call(&Request::SetMonitors { settings })?;
    app.log.line(&format!("--- monitors: {line}"));
    Ok(())
}

/// A host without a physical monitor, in session: "Apply" on the
/// client's own monitors. The service makes them so
/// in place and keeps the set for that client.
#[tauri::command(async)]
pub(crate) fn set_client_monitors(
    app: State<'_, App>,
    client: String,
    set: vdd::layout::ClientSet,
) -> Result<(), String> {
    set.rects().map_err(|e| e.to_string())?;
    let line = serde_json::to_string(&set).unwrap_or_default();
    hostsvc::call(&Request::SetClientMonitors { client, set })?;
    app.log.line(&format!("--- client monitors: {line}"));
    Ok(())
}

/// The trash button: no virtual monitors, no settings.
#[tauri::command(async)]
pub(crate) fn reset_monitors(app: State<'_, App>) -> Result<(), String> {
    hostsvc::call(&Request::ResetMonitors)?;
    app.log.line("--- monitors reset");
    Ok(())
}

/// Opens the folder of the host's logs (the service writes them; every account can read them).
#[tauri::command]
pub(crate) fn open_log_folder() -> Result<(), String> {
    let dir = hostsvc::registration_dir().join("logs");
    if !dir.is_dir() {
        return Err("err_no_log".into());
    }
    desktop::open_folder(&dir)
}
