//! What the hub's interface gets from `get_state`: web/client/src/api.ts has
//! the same, and the test below keeps them equal (signal::contract).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use signal::{APP_VERSION, FPS_CHOICES, MBPS_PRESETS};

use crate::Settings;
use crate::card::{Button, Card, CardLive, button, host_update};
use crate::server::{LimitsJson, ServerLive};
use desktop::update::UpdateView;

#[derive(Debug, Serialize)]
pub(crate) struct AppState {
    settings: SettingsView,
    fps_choices: [u16; 2],
    mbps_presets: [f32; 7],
    cards: Vec<CardView>,
    /// What the server sets for this client, each on its own (null: the client chooses).
    forced: LimitsJson,
    /// The server's version, this program's update.
    update: UpdateView,
    video: Option<VideoView>,
    last_video: Option<LastVideo>,
    log_path: PathBuf,
    version: &'static str,
}

/// What the interface may change (not the cards, not the next id).
#[derive(Debug, Serialize)]
struct SettingsView {
    name: String,
    /// `host:port`; empty until the welcome window.
    server: String,
    fps: u16,
    mbps: f32,
    borderless: bool,
    shortcuts_to_host: bool,
    keep_unlocked: bool,
    lang: String,
    auto_update: bool,
}

#[derive(Debug, Serialize)]
struct CardView {
    id: u64,
    token: String,
    machine: String,
    account: String,
    found: Option<bool>,
    access: String,
    presence: String,
    host_version: String,
    /// The host's last update failed: its code ("": no failure).
    update_failed: String,
    /// Another version on the host: "", `on_connect`, `off`, `newer` (`card::host_update`).
    host_update: &'static str,
    server_error: String,
    server_unknown: bool,
    button: Button,
}

#[derive(Debug, Serialize)]
pub(crate) struct VideoView {
    pub(crate) card: u64,
    pub(crate) since: u64,
    /// As the video window said (apps/client/src/status.rs: its own contract).
    pub(crate) status: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct LastVideo {
    pub(crate) card: u64,
    pub(crate) error: String,
}

fn card_view(c: &Card, l: &CardLive, srv: &ServerLive) -> CardView {
    CardView {
        id: c.id,
        token: c.token.clone(),
        machine: c.machine.clone(),
        account: c.account.clone(),
        found: l.found,
        access: l.access.clone(),
        presence: l.presence.clone(),
        host_version: l.host_version.clone(),
        update_failed: l.update_failed.clone(),
        host_update: host_update(l),
        server_error: srv.error.clone(),
        server_unknown: srv.unknown,
        button: button(l, srv),
    }
}

impl AppState {
    pub(crate) fn of(
        s: &Settings,
        live: &HashMap<u64, CardLive>,
        server: &ServerLive,
        update: &UpdateView,
        video: Option<VideoView>,
        last_video: Option<LastVideo>,
        log_path: PathBuf,
    ) -> AppState {
        let cards = s
            .cards
            .iter()
            .map(|c| {
                let l = live.get(&c.id).cloned().unwrap_or_default();
                card_view(c, &l, server)
            })
            .collect();
        AppState {
            settings: SettingsView {
                name: s.name.clone(),
                server: s.server.clone(),
                fps: s.fps,
                mbps: s.mbps,
                borderless: s.borderless,
                shortcuts_to_host: s.shortcuts_to_host,
                keep_unlocked: s.keep_unlocked,
                lang: s.lang.clone(),
                auto_update: s.auto_update,
            },
            fps_choices: FPS_CHOICES,
            mbps_presets: MBPS_PRESETS,
            cards,
            forced: server.forced,
            update: update.clone(),
            video,
            last_video,
            log_path,
            version: APP_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use signal::contract::{check_example, steady};

    use super::*;

    fn card(id: u64) -> Card {
        Card {
            id,
            token: "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF".into(),
            machine: "WPC-10".into(),
            account: String::new(),
        }
    }

    fn live(access: &str, presence: &str) -> CardLive {
        CardLive {
            found: Some(true),
            access: access.into(),
            presence: presence.into(),
            host_version: "0.0.0".into(),
            update_failed: String::new(),
            auto_update: true,
        }
    }

    #[test]
    fn the_interface_has_the_same() {
        let settings = Settings {
            name: "gate".into(),
            server: "a:7443".into(),
            cards: vec![card(1), card(2), card(3)],
            // The default is the installer's language: not "en" on every machine.
            lang: "en".into(),
            ..Settings::default()
        };
        let cards = HashMap::from([
            (1, live("approved", "online")),
            (
                2,
                CardLive {
                    found: None,
                    ..live("", "")
                },
            ),
            (3, live("pending", "offline")),
        ]);
        let server = ServerLive {
            forced: LimitsJson {
                fps: Some(30),
                mbps: None,
            },
            ..ServerLive::default()
        };
        let none = UpdateView::default();
        let plain = AppState::of(
            &settings,
            &cards,
            &server,
            &none,
            None,
            None,
            "client.log".into(),
        );
        // The server out of reach, and one that does not know this client.
        let unreachable = AppState::of(
            &settings,
            &cards,
            &ServerLive {
                error: "no link".into(),
                ..ServerLive::default()
            },
            &UpdateView {
                server_version: "9.9.9".into(),
                newer: true,
                phase: "downloading",
                ..UpdateView::default()
            },
            None,
            None,
            "client.log".into(),
        );
        let unknown = AppState::of(
            &settings,
            &cards,
            &ServerLive {
                error: "err_unknown_device".into(),
                unknown: true,
                ..ServerLive::default()
            },
            &UpdateView {
                server_version: "9.9.9".into(),
                newer: true,
                error: "err_update_sha".into(),
                ..UpdateView::default()
            },
            None,
            None,
            "client.log".into(),
        );
        let session = AppState::of(
            &Settings {
                lang: "en".into(),
                ..Settings::default()
            },
            &cards,
            &ServerLive::default(),
            &none,
            Some(VideoView {
                card: 1,
                since: 5,
                status: json!({ "event": "connecting" }),
            }),
            Some(LastVideo {
                card: 2,
                error: "err_video_exit|1".into(),
            }),
            "client.log".into(),
        );
        let mut examples = serde_json::to_value([plain, unreachable, unknown, session]).unwrap();
        steady(&mut examples, &["version"]);
        let Value::Array(examples) = examples else {
            unreachable!()
        };
        check_example(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../web/client/src/contract/app.example.ts"
            ),
            "AppState",
            "../api",
            &examples,
        );
    }
}
