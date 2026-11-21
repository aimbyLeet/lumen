//! Lumen Client: cards of the hosts this user connects to, and the client's
//! settings.
//!
//! The client has one server, set with its name in
//! the welcome window; a card is a host's token there. The app keeps a
//! signaling link to the server (registering this client there the first
//! time) and polls the cards' state: access (asked, approved, rejected) and
//! the host's presence.
//! «Connect» opens the video window: `lumen-client.exe` next to this program,
//! one at a time; it keeps running if this window is closed (no tray). While
//! the video window is on the screen, this one is hidden (see [`watch_video`]).
//!
//! Its parts: `card` (a card, its state and button), `server` (the link to the
//! server: registration, requests, polling), `update` (this program updated from
//! the server), `video` (the video window's process).

#![windows_subsystem = "windows"]

mod card;
mod server;
mod state;
mod update;
mod video;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use desktop::log::Log;
use desktop::log_path;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use signal::Quality;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use transport::StaticKey;
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

use crate::card::{Card, CardLive};
use crate::server::ServerLive;
use crate::update::UpdateLive;
use crate::video::Video;

const POLL: Duration = Duration::from_secs(2);
/// The app's settings and cards, in the data folder.
const SETTINGS: &str = "client-app.json";
/// The overlay page's window.
const OVERLAY: &str = "overlay";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// This client's name on the server. Empty until the welcome window;
    /// stays while there are machines.
    name: String,
    /// The server, `host:port`. Empty until the welcome window; stays while
    /// there are machines.
    server: String,
    /// Development: the CA of the server's certificate (PEM file; no field
    /// for it in the windows, written into the file by checks).
    server_ca: String,
    fps: u16,
    /// 0: auto.
    mbps: f32,
    /// The video window without a frame, over the whole monitor: always 1:1
    /// (a framed window that does not fit is scaled down).
    borderless: bool,
    /// System shortcuts (Win, Alt+Tab, ...) go to the host while the video
    /// window is in front (`lumen-client --shortcuts host`).
    shortcuts_to_host: bool,
    /// The host does not lock Windows when the session ends
    /// (`lumen-client --keep-unlocked`).
    keep_unlocked: bool,
    /// The language of the windows (`desktop::lang`).
    lang: String,
    /// A newer version on the server is installed at start.
    auto_update: bool,
    /// The version last installed at start: not again (an installer that
    /// did not make this program newer would run at every start).
    update_tried: String,
    cards: Vec<Card>,
    next_id: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: String::new(),
            server: String::new(),
            server_ca: String::new(),
            fps: 60,
            mbps: 0.0,
            borderless: false,
            shortcuts_to_host: true,
            keep_unlocked: false,
            lang: desktop::lang::default(),
            auto_update: true,
            update_tried: String::new(),
            cards: Vec::new(),
            next_id: 1,
        }
    }
}

struct App {
    data: PathBuf,
    log: Arc<Log>,
    key: StaticKey,
    settings: Mutex<Settings>,
    cards: Mutex<HashMap<u64, CardLive>>,
    /// What the server last said about this client.
    live_server: Mutex<ServerLive>,
    /// The server's version and this program's update.
    update: Mutex<UpdateLive>,
    video: Mutex<Option<Video>>,
    /// The last video window that ended badly: (card, why).
    last_video: Mutex<Option<(u64, String)>>,
}

impl App {
    /// Writes the settings as changed under this lock, and lets go of it
    /// (the file is written without holding it).
    fn keep_settings(&self, s: std::sync::MutexGuard<'_, Settings>) {
        let snapshot = s.clone();
        drop(s);
        self.save_settings(&snapshot);
    }

    fn save_settings(&self, s: &Settings) {
        let _ = std::fs::create_dir_all(&self.data);
        // Tokens are sealed on the way (`Card::token`): if that fails, the file stays as it was.
        match serde_json::to_string_pretty(s) {
            Ok(text) => {
                if let Err(e) = std::fs::write(self.data.join(SETTINGS), text) {
                    self.log.line(&format!("{SETTINGS}: {e}"));
                }
            }
            Err(e) => self.log.line(&format!("{SETTINGS}: {e}")),
        }
    }

    /// The name and the server (the welcome window, the settings). The server
    /// knows this computer by its name: both change only with no machines
    /// (the last machine takes this client off the server). Another server is
    /// kept only if it answers (`err_cert_untrusted|…`: the user decides
    /// about its key first).
    fn set_identity(&self, name: String, server: String) -> Result<(), String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("err_name_required".into());
        }
        // Latin only (owner's decision 2026-11-09), the server's own rule.
        if !signal::name_allowed(&name) {
            return Err("err_name_invalid".into());
        }
        let server = signal::server_address(&server)?;
        let (same_name, same_server, ca, has_cards) = {
            let s = self.settings.lock().unwrap();
            (
                s.name == name,
                s.server == server,
                s.server_ca.clone(),
                !s.cards.is_empty(),
            )
        };
        if same_name && same_server {
            return Ok(());
        }
        if has_cards {
            return Err(if same_name {
                "err_server_has_machines"
            } else {
                "err_name_locked"
            }
            .into());
        }
        if !same_server {
            self.probe(&server, &ca)?;
        }
        let mut s = self.settings.lock().unwrap();
        if !same_server {
            self.log.line(&format!("--- the server: {server}"));
            *self.live_server.lock().unwrap() = ServerLive::default();
        }
        s.name = name;
        s.server = server;
        self.keep_settings(s);
        Ok(())
    }

    /// Lets go of the video window once it has closed (remembering why, if
    /// it ended badly); the one still open: its id, whether it is on the
    /// screen, and whether the host asks for our monitor.
    fn watch_video(&self) -> Option<(u64, bool, bool)> {
        let mut video = self.video.lock().unwrap();
        let v = video.as_mut()?;
        if let Some(why) = v.finished() {
            if !why.is_empty() {
                *self.last_video.lock().unwrap() = Some((v.card, why));
            }
            *video = None;
            return None;
        }
        Some((v.id, v.shown(), v.asking()))
    }
}

/// How often the hub looks at the video window.
const WATCH: Duration = Duration::from_millis(100);

/// The hub steps aside while the video window is on the screen (off the
/// screen and the taskbar, as Parsec does) and comes back in front when it
/// closes. While it connects, the hub stays: the card shows how it goes.
/// Shown again meanwhile (Lumen Client started once more), it stays. A
/// host that has lost its monitors mid-session asks for ours: the hub comes
/// over the video for the answer and steps aside again after it.
fn watch_video(handle: AppHandle) {
    let mut hub = Hub::Front;
    // The video window the overlay page's window is for.
    let mut overlaid: Option<u64> = None;
    loop {
        let window = handle.get_webview_window("main");
        let video = handle.state::<App>().watch_video();
        match video {
            Some((id, true, _)) if overlaid != Some(id) => {
                close_overlay(&handle);
                if open_overlay(&handle) {
                    overlaid = Some(id);
                }
            }
            None if overlaid.take().is_some() => close_overlay(&handle),
            _ => {}
        }
        let (next, act) = hub.after(video);
        hub = next;
        if let Some(w) = window {
            match act {
                HubAct::Hide => {
                    let _ = w.hide();
                }
                HubAct::Show => to_front(&w),
                HubAct::Stay => {}
            }
        }
        thread::sleep(WATCH);
    }
}

/// The hub: Lumen Client's main window.
fn open_hub(handle: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    WebviewWindowBuilder::new(handle, "main", WebviewUrl::default())
        .title("Lumen Client")
        .inner_size(960.0, 640.0)
        // Counted with the invisible frame (18 px at 125 %): at least 900 inside.
        .min_inner_size(920.0, 500.0)
        // Its own frame (web/client): the shadow and rounded corners stay Windows'.
        .decorations(false)
        .shadow(true)
        .build()
}

/// The hub shown and in front, over the video window too. Without the
/// foreground `set_focus` only flashes the taskbar and the hub stays under the
/// video; topmost on and off raises it anyway (`HWND_NOTOPMOST` puts it at the
/// top of the ordinary windows) and needs no foreground.
fn to_front(w: &tauri::WebviewWindow) {
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_always_on_top(true);
    let _ = w.set_always_on_top(false);
    let _ = w.set_focus();
}

/// Where the hub is while a video window is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hub {
    /// As the user left it (no video window on the screen yet, or none).
    Front,
    /// Stepped aside for video window `id`.
    Aside(u64),
    /// Over video window `id`: its host asks for our monitor.
    Over(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HubAct {
    Stay,
    Hide,
    /// In front again.
    Show,
}

impl Hub {
    /// After a look at the video window: its id, whether it is on the
    /// screen, whether its host asks for our monitor (`App::watch_video`).
    fn after(self, video: Option<(u64, bool, bool)>) -> (Hub, HubAct) {
        match (self, video) {
            // The question: the hub in front, over the video.
            (Hub::Aside(a), Some((id, true, true))) if a == id => (Hub::Over(id), HubAct::Show),
            // On the screen (again: answered): aside.
            (hub, Some((id, true, false))) if hub != Hub::Aside(id) => {
                (Hub::Aside(id), HubAct::Hide)
            }
            // Closed: back.
            (Hub::Aside(_) | Hub::Over(_), None) => (Hub::Front, HubAct::Show),
            (hub, _) => (hub, HubAct::Stay),
        }
    }
}

/// What is drawn over the video: a transparent window of its own, which the
/// video window itself keeps over the picture (`apps/client/src/ui_api.rs`).
fn open_overlay(handle: &AppHandle) -> bool {
    let app = handle.state::<App>();
    // The page takes its language from the address (web/client/src/i18n.ts).
    let lang = app.settings.lock().unwrap().lang.clone();
    let page = format!("overlay.html?lang={lang}");
    let built = WebviewWindowBuilder::new(handle, OVERLAY, WebviewUrl::App(page.into()))
        .title("Lumen Overlay")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            app.log.line(&format!("overlay window: {e}"));
            return false;
        }
    };
    match window.hwnd() {
        Ok(hwnd) => app.to_video(&json!({ "cmd": "overlay_window", "hwnd": hwnd.0 as isize })),
        Err(e) => app.log.line(&format!("overlay window: {e}")),
    }
    true
}

fn close_overlay(handle: &AppHandle) {
    if let Some(w) = handle.get_webview_window(OVERLAY) {
        let _ = w.destroy();
    }
}

impl App {
    fn to_video(&self, command: &Value) {
        if let Some(v) = self.video.lock().unwrap().as_ref() {
            v.command(command);
        }
    }
}

/// The overlay page: whether it has anything to show, where it takes clicks.
#[tauri::command]
fn overlay_ui(app: State<'_, App>, visible: bool, interactive: Vec<[i32; 4]>) {
    app.to_video(&json!({ "cmd": "overlay_ui", "visible": visible, "interactive": interactive }));
}

#[tauri::command]
fn cancel_transfer(app: State<'_, App>, outgoing: bool, transfer: u32) {
    app.to_video(&json!({ "cmd": "cancel_transfer", "outgoing": outgoing, "transfer": transfer }));
}

#[tauri::command]
fn show_monitor(app: State<'_, App>, index: usize) {
    app.to_video(&json!({ "cmd": "show_monitor", "index": index }));
}

/// The monitor for a host without one: the user's answer to its question.
#[tauri::command]
fn choose_monitor(app: State<'_, App>, width: u32, height: u32, scale: u16) {
    app.log.line(&format!(
        "the host's monitor: {width}x{height} at {scale} %"
    ));
    app.to_video(
        &json!({ "cmd": "monitor_choice", "width": width, "height": height, "scale": scale }),
    );
}

#[tauri::command]
fn get_state(app: State<'_, App>) -> state::AppState {
    let s = app.settings.lock().unwrap().clone();
    let live = app.cards.lock().unwrap().clone();
    let server = app.live_server.lock().unwrap().clone();
    let update = app.update.lock().unwrap().shown.clone();
    let video = app
        .video
        .lock()
        .unwrap()
        .as_ref()
        .map(|v| state::VideoView {
            card: v.card,
            since: v.since,
            status: v.status(),
        });
    let last = app
        .last_video
        .lock()
        .unwrap()
        .clone()
        .map(|(card, error)| state::LastVideo { card, error });
    state::AppState::of(&s, &live, &server, &update, video, last, app.log.path())
}

/// A new card: the host's token. Asks the admin for access.
#[tauri::command(async)]
fn add_card(app: State<'_, App>, token: String) -> Result<(), String> {
    app.add_card(token)
}

/// The name and the server: the welcome window, the settings (see [`App::set_identity`]).
#[tauri::command(async)]
fn set_identity(app: State<'_, App>, name: String, server: String) -> Result<(), String> {
    app.set_identity(name, server)?;
    // The new server's version: the footer knows at once.
    app.check_update(update::START_CHECK);
    Ok(())
}

/// The footer's click, confirmed: the newer version is downloaded and
/// installed; this program closes and starts again after it.
#[tauri::command(async)]
fn update_now(handle: AppHandle, app: State<'_, App>) -> Result<(), String> {
    app.install_update()?;
    close_for_update(handle);
    Ok(())
}

/// The installer runs: this program goes (a moment later: the answer first).
fn close_for_update(handle: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        handle.exit(0);
    });
}

/// The user trusts the server's key they were shown (`err_cert_untrusted` /
/// `err_cert_changed`): the next connections accept only it.
#[tauri::command(async)]
fn trust_server(app: State<'_, App>, server: String, fingerprint: String) -> Result<(), String> {
    app.trust_server(&server, &fingerprint)
}

/// Asks again (the admin took the access back).
#[tauri::command(async)]
fn request_access(app: State<'_, App>, id: u64) -> Result<(), String> {
    let card = app.card(id)?;
    app.ask(&card).map(|_| ())
}

/// Restarts the card's machine (see [`App::reboot_card`]).
#[tauri::command(async)]
fn reboot_card(app: State<'_, App>, id: u64) -> Result<String, String> {
    app.reboot_card(id)
}

/// Deletes the card (see [`App::remove_card`]).
#[tauri::command(async)]
fn remove_card(app: State<'_, App>, id: u64) -> Result<String, String> {
    app.remove_card(id)
}

#[tauri::command(async)]
fn save_settings(
    app: State<'_, App>,
    fps: u16,
    mbps: f32,
    borderless: bool,
    shortcuts_to_host: bool,
    keep_unlocked: bool,
    auto_update: bool,
) -> Result<(), String> {
    if !(Quality { fps, mbps }).is_valid() {
        return Err("err_quality".into());
    }
    let mut s = app.settings.lock().unwrap();
    s.fps = fps;
    s.mbps = mbps;
    s.borderless = borderless;
    s.shortcuts_to_host = shortcuts_to_host;
    s.keep_unlocked = keep_unlocked;
    s.auto_update = auto_update;
    app.keep_settings(s);
    Ok(())
}

/// The windows' language: the hub at once, the overlay from the next session.
#[tauri::command]
fn set_lang(app: State<'_, App>, lang: String) -> Result<(), String> {
    desktop::lang::check(&lang)?;
    let mut s = app.settings.lock().unwrap();
    s.lang = lang;
    app.keep_settings(s);
    Ok(())
}

/// Opens the video window for the card (closing another one first), `title` on it.
#[tauri::command]
fn connect(handle: AppHandle, app: State<'_, App>, id: u64, title: String) -> Result<(), String> {
    let card = app.card(id)?;
    let s = app.settings.lock().unwrap().clone();
    let server = s.server.clone();
    let mut video = app.video.lock().unwrap();
    if let Some(v) = video.take() {
        v.stop();
    }
    app.last_video.lock().unwrap().take();
    let mut args = vec![
        "--server".to_string(),
        server.clone(),
        "--token-stdin".into(),
        "--creds".into(),
        app.creds_path(&server).display().to_string(),
        "--name".into(),
        s.name.clone(),
        "--fps".into(),
        s.fps.to_string(),
        "--max-mbps".into(),
        s.mbps.to_string(),
        "--title".into(),
        title,
        "--status-json".into(),
    ];
    let view = app.view_path(id);
    if let Some(dir) = view.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    args.extend(["--view-memory".into(), view.display().to_string()]);
    if !s.server_ca.is_empty() {
        args.extend(["--server-ca".into(), s.server_ca.clone()]);
    } else if let Some(fp) = app.pin(&server) {
        args.extend(["--server-pin".into(), fp]);
    }
    if s.borderless {
        args.push("--borderless".into());
    }
    if !s.shortcuts_to_host {
        args.extend(["--shortcuts".into(), "local".into()]);
    }
    if s.keep_unlocked {
        args.push("--keep-unlocked".into());
    }
    *video = Some(Video::start(
        id,
        args,
        &card.token,
        &app.data,
        app.log.clone(),
        Box::new(move |state| {
            let _ = handle.emit_to(OVERLAY, OVERLAY, state);
        }),
    )?);
    Ok(())
}

#[tauri::command]
fn disconnect(app: State<'_, App>) {
    if let Some(v) = app.video.lock().unwrap().take() {
        v.stop();
    }
}

/// Opens the folder of the logs.
#[tauri::command]
fn open_log_folder(app: State<'_, App>) -> Result<(), String> {
    let path = app.log.path();
    desktop::open_folder(path.parent().ok_or("err_no_log_folder")?)
}

impl App {
    /// The app's state, kept in `data` (`%APPDATA%\Lumen`).
    fn open(data: PathBuf, log: Arc<Log>) -> App {
        let text = std::fs::read_to_string(data.join(SETTINGS)).ok();
        let mut settings: Settings = text
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default();
        // Cards of a Lumen before 0.7.0 (each with its server): no carrying over
        //, the welcome window comes first.
        let stale = settings.server.is_empty() && !settings.cards.is_empty();
        if stale {
            settings.cards.clear();
        }
        let plain = text.as_deref().is_some_and(has_plain_tokens);
        let key = match StaticKey::load_or_create(&data.join("client.key")) {
            Ok((k, _)) => k,
            Err(e) => {
                log.line(&format!("client key: {e}"));
                StaticKey::generate()
            }
        };
        let app = App {
            data,
            log,
            key,
            settings: Mutex::new(settings.clone()),
            cards: Mutex::new(HashMap::new()),
            live_server: Mutex::new(ServerLive::default()),
            update: Mutex::new(UpdateLive::default()),
            video: Mutex::new(None),
            last_video: Mutex::new(None),
        };
        if stale {
            app.log
                .line("cards of a Lumen before 0.7.0 (no server set): dropped");
        }
        if plain || stale {
            if plain {
                app.log.line("tokens kept in plain text: sealing them");
            }
            app.save_settings(&settings);
        }
        if settings.cards.iter().any(|c| c.token.is_empty()) {
            app.log
                .line("a token does not open (another account or computer?)");
        }
        app
    }
}

/// Whether the file keeps a card's token in plain text (before 0.6.0).
fn has_plain_tokens(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    v["cards"].as_array().is_some_and(|cards| {
        cards
            .iter()
            .any(|c| c["token"].as_str().is_some_and(desktop::secret::is_plain))
    })
}

fn main() {
    let data = transport::default_dir();
    let log = Arc::new(Log::new(log_path(&data, "client.log")));
    log.line(&format!("--- Lumen Client {}", signal::APP_VERSION));
    let app = App::open(data, log);
    let mut builder = tauri::Builder::default();
    // One window on a computer; another LUMEN_DATA is another device (checks on one machine).
    if std::env::var_os("LUMEN_DATA").is_none() {
        // A second start (by hand) is in the foreground: it lets the first one
        // take it before the plugin hands over and this process exits.
        // SAFETY: a plain user32 call without pointers.
        let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // No hub (it cannot be: closing it ends the program): a new one.
            match app.get_webview_window("main") {
                Some(w) => to_front(&w),
                None => {
                    if let Ok(w) = open_hub(app) {
                        to_front(&w);
                    }
                }
            }
        }));
    }
    builder
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            get_state,
            add_card,
            set_identity,
            update_now,
            trust_server,
            request_access,
            remove_card,
            reboot_card,
            save_settings,
            set_lang,
            connect,
            disconnect,
            open_log_folder,
            overlay_ui,
            cancel_transfer,
            show_monitor,
            choose_monitor
        ])
        // The hub closed is the program closed, the video window with it: before,
        // the hidden overlay window kept the process alive with no window to
        // show, and a new start (single instance) showed nothing (2026-11-13).
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                let handle = window.app_handle();
                if let Some(v) = handle.state::<App>().video.lock().unwrap().take() {
                    v.stop();
                }
                handle.exit(0);
            }
        })
        .setup(|app| {
            open_hub(app.handle())?;
            // A newer version at start: installed now (the window shows it), if the user lets it.
            let handle = app.handle().clone();
            thread::Builder::new()
                .name("update".into())
                .spawn(move || {
                    if handle.state::<App>().update_at_start() {
                        close_for_update(handle);
                    }
                })?;
            let handle = app.handle().clone();
            thread::Builder::new().name("poll".into()).spawn(move || {
                let mut link = None;
                loop {
                    handle.state::<App>().poll(&mut link);
                    thread::sleep(POLL);
                }
            })?;
            let handle = app.handle().clone();
            thread::Builder::new()
                .name("video".into())
                .spawn(move || watch_video(handle))?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Lumen Client failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_steps_aside_and_comes_over_for_the_question() {
        fn step(hub: &mut Hub, video: Option<(u64, bool, bool)>) -> HubAct {
            let (next, act) = hub.after(video);
            *hub = next;
            act
        }
        let h = &mut Hub::Front;
        assert_eq!(step(h, Some((1, false, false))), HubAct::Stay, "connecting");
        assert_eq!(
            step(h, Some((1, false, true))),
            HubAct::Stay,
            "asked before the window"
        );
        assert_eq!(step(h, Some((1, true, false))), HubAct::Hide);
        assert_eq!(step(h, Some((1, true, false))), HubAct::Stay, "once");
        assert_eq!(
            step(h, Some((1, true, true))),
            HubAct::Show,
            "asked mid-session"
        );
        assert_eq!(*h, Hub::Over(1));
        assert_eq!(step(h, Some((1, true, true))), HubAct::Stay);
        assert_eq!(step(h, Some((1, true, false))), HubAct::Hide, "answered");
        assert_eq!(step(h, None), HubAct::Show, "closed");
        assert_eq!(step(h, None), HubAct::Stay);
        // Over the video when it closes: back the same.
        *h = Hub::Over(2);
        assert_eq!(step(h, None), HubAct::Show);
        // A new video window while aside for an old one.
        *h = Hub::Aside(2);
        assert_eq!(step(h, Some((3, true, false))), HubAct::Hide);
        assert_eq!(*h, Hub::Aside(3));
    }

    const TOKEN: &str = "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF";

    #[test]
    fn tokens_are_sealed_in_the_file() {
        let plain = format!(
            r#"{{"name":"a","server":"s:7443","cards":[{{"id":1,"token":"{TOKEN}"}}],"next_id":2}}"#
        );
        assert!(has_plain_tokens(&plain));
        let s: Settings = serde_json::from_str(&plain).unwrap();
        assert_eq!(s.cards[0].token, TOKEN, "a file from before 0.6.0 reads");

        let sealed = serde_json::to_string_pretty(&s).unwrap();
        assert!(!sealed.contains("AAAA"), "{sealed}");
        assert!(!has_plain_tokens(&sealed));
        let back: Settings = serde_json::from_str(&sealed).unwrap();
        assert_eq!(back.cards[0].token, TOKEN);
    }

    #[test]
    fn the_app_seals_an_old_file_on_start() {
        let dir =
            std::env::temp_dir().join(format!("lumen-client-app-seal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(SETTINGS);
        std::fs::write(
            &file,
            format!(r#"{{"name":"a","server":"s:7443","cards":[{{"id":1,"token":"{TOKEN}"}}],"next_id":2}}"#),
        )
        .unwrap();
        let log = Arc::new(Log::new(dir.join("client.log")));
        let app = App::open(dir.clone(), log);
        assert_eq!(app.settings.lock().unwrap().cards[0].token, TOKEN);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("AAAA") && !has_plain_tokens(&text), "{text}");
        drop(app);
        let again = App::open(dir.clone(), Arc::new(Log::new(dir.join("client.log"))));
        assert_eq!(again.settings.lock().unwrap().cards[0].token, TOKEN);
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cards of a Lumen before 0.7.0 (a server each, none set for the client):
    /// dropped, the welcome window comes first; the rest stays.
    #[test]
    fn cards_from_before_one_server_are_dropped() {
        let dir =
            std::env::temp_dir().join(format!("lumen-client-app-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(SETTINGS);
        std::fs::write(
            &file,
            r#"{"name":"a","fps":30,"cards":[{"id":1,"server":"s:7443","token":"dpapi:00ff","server_ca":""}],"next_id":2}"#,
        )
        .unwrap();
        let app = App::open(dir.clone(), Arc::new(Log::new(dir.join("client.log"))));
        let s = app.settings.lock().unwrap().clone();
        assert!(s.cards.is_empty());
        assert_eq!((s.name.as_str(), s.server.as_str(), s.fps), ("a", "", 30));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("s:7443"), "{text}");
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_that_does_not_open_keeps_the_rest() {
        let text = r#"{"name":"a","server":"s:7443","cards":[{"id":1,"token":"dpapi:00ff"},
            {"id":2,"token":""}],"next_id":3}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(s.name, "a");
        assert_eq!(s.cards.len(), 2);
        assert!(s.cards[0].token.is_empty());
        assert!(!has_plain_tokens(text));
    }
}
