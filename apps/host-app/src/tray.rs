//! The tray: turning the host off from it, its menu, showing the window.

use desktop::hostsvc::{self, Request};
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager};

use crate::App;

/// Turns the host off and quits: nobody can connect until it is started again.
pub(crate) fn turn_off_and_quit(app: &AppHandle) {
    let log = app.state::<App>().log.clone();
    match hostsvc::call(&Request::Stop) {
        Ok(_) => log.line("--- host turned off, quitting"),
        Err(e) => log.line(&format!("--- quitting; the host: {e}")),
    }
    app.exit(0);
}

/// The tray's menu, in the interface's words; what its items do is here.
#[tauri::command]
pub(crate) fn set_tray(app: AppHandle, open: String, quit: String) -> Result<(), String> {
    let e = |e: tauri::Error| e.to_string();
    let open = MenuItem::with_id(&app, "open", open, true, None::<&str>).map_err(e)?;
    let quit = MenuItem::with_id(&app, "quit", quit, true, None::<&str>).map_err(e)?;
    let menu = Menu::with_items(&app, &[&open, &quit]).map_err(e)?;
    let tray = app.tray_by_id("main").ok_or("no tray icon")?;
    tray.set_menu(Some(menu)).map_err(e)?;
    app.state::<App>().log.line("--- the tray's menu is set");
    Ok(())
}

pub(crate) fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
