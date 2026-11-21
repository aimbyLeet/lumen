//! What the window gets from `get_state` (the interface's `AppState`) and
//! the contract test that keeps it equal to `web/host/src/api.ts`.

use desktop::hostsvc::{self, HostStatus};
use desktop::update::UpdateView;
use serde::Serialize;

use crate::settings::Settings;

/// What Lumen Host calls a monitor of Lumen VDD.
const LUMEN_MONITOR_NAME: &str = "LumenVDA";

/// How many of the `real` monitors are shown: those on NVIDIA's GPU, the
/// primary one always (`pick_monitors` of the host).
pub(crate) fn shown(real: &[vdd::display::Active]) -> usize {
    let outputs = capture::list_outputs().unwrap_or_default();
    real.iter()
        .filter(|a| {
            (a.rect.x == 0 && a.rect.y == 0)
                || outputs
                    .iter()
                    .find(|o| o.device_name == a.gdi_name)
                    .is_none_or(|o| o.is_nvidia)
        })
        .count()
}

/// What the window gets from `get_state`: web/host/src/api.ts has the same,
/// and the test at the end keeps them equal (signal::contract).
#[derive(Debug, Serialize)]
pub(crate) struct AppState {
    settings: Settings,
    defaults: Defaults,
    token: Option<String>,
    registered: bool,
    status: HostStatus,
    service_error: Option<String>,
    host_off: bool,
    autostart: bool,
    keep_awake: bool,
    /// The host updates itself when a newer client connects.
    auto_update: bool,
    /// The server's version and the host's update (web/client/src/Update.tsx).
    update: UpdateView,
    version: &'static str,
    monitors: MonitorsInfo,
    lang: String,
}

/// The footer as Lumen Client's: its yellow, the update under way, why it failed.
/// The phase is "" or "downloading" (Lumen Host is closed by the installer).
fn update_view(st: &HostStatus, clicked: Option<(std::time::Instant, String)>) -> UpdateView {
    // After the click the host restarts (knowing nothing for a while):
    // «updating» to the version clicked for, until it says otherwise.
    let clicked = clicked.filter(|(t, _)| t.elapsed() < UPDATE_CLICK_SHOWN);
    let server_version = [&st.updating, &st.server_version]
        .into_iter()
        .find(|v| !v.is_empty())
        .cloned()
        .or_else(|| clicked.as_ref().map(|(_, v)| v.clone()))
        .unwrap_or_default();
    let clicked = clicked.is_some();
    UpdateView {
        newer: signal::update::newer(&server_version, signal::APP_VERSION),
        server_version,
        phase: if (!st.updating.is_empty() || clicked) && st.update_error.is_empty() {
            "downloading"
        } else {
            ""
        },
        error: st.update_error.clone(),
    }
}

/// How long the footer's click shows «updating» without a word from the host.
const UPDATE_CLICK_SHOWN: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Serialize)]
struct Defaults {
    machine: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct MonitorsInfo {
    state: hostsvc::Monitors,
    real: Vec<RealMonitor>,
    shown: usize,
    allowed: usize,
    /// A monitor of Lumen VDD is on in Windows (a parked set: kept on, or off
    /// with another display on).
    lumen_on: bool,
    catalog: Catalog,
}

#[derive(Debug, Serialize)]
struct RealMonitor {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    name: String,
}

/// What the "Monitors" section offers.
#[derive(Debug, Serialize)]
struct Catalog {
    resolutions: &'static [vdd::layout::Resolution],
    scales: &'static [u16],
}

const CATALOG: Catalog = Catalog {
    resolutions: vdd::layout::RESOLUTIONS,
    scales: vdd::layout::SCALES,
};

impl AppState {
    /// The service answered; `lang`: the window's (`host-app.json`).
    pub(crate) fn of_service(
        st: hostsvc::State,
        local: Settings,
        machine: String,
        monitors: MonitorsInfo,
        lang: String,
        clicked: Option<(std::time::Instant, String)>,
    ) -> AppState {
        let update = update_view(&st.status, clicked);
        let settings = if st.registered {
            let s = st.settings;
            Settings {
                server: s.server,
                machine: s.machine,
                manual_addrs: s.manual_addrs,
                server_ca: if s.server_ca_pem.is_empty() {
                    String::new()
                } else {
                    local.server_ca
                },
                server_fingerprint: s.server_fingerprint,
            }
        } else {
            Settings {
                machine: machine.clone(),
                ..Settings::default()
            }
        };
        AppState {
            settings,
            defaults: Defaults { machine },
            token: (!st.token.is_empty()).then_some(st.token),
            registered: st.registered,
            status: if st.registered {
                st.status
            } else {
                HostStatus::stopped()
            },
            service_error: None,
            host_off: false,
            autostart: st.autostart,
            keep_awake: st.keep_awake,
            auto_update: st.auto_update,
            update,
            version: env!("CARGO_PKG_VERSION"),
            monitors,
            lang,
        }
    }

    /// The service did not answer: `error`; `host_off`: it is stopped.
    pub(crate) fn without_service(
        error: String,
        host_off: bool,
        machine: String,
        monitors: MonitorsInfo,
        lang: String,
    ) -> AppState {
        AppState {
            settings: Settings {
                machine: machine.clone(),
                ..Settings::default()
            },
            defaults: Defaults { machine },
            token: None,
            registered: false,
            status: HostStatus::stopped(),
            service_error: Some(error),
            host_off,
            autostart: false,
            keep_awake: false,
            auto_update: false,
            update: UpdateView::default(),
            version: env!("CARGO_PKG_VERSION"),
            monitors,
            lang,
        }
    }
}

/// The service's virtual monitors; without the service, only whether the driver is there.
pub(crate) fn monitors_of(st: Option<&hostsvc::State>) -> MonitorsInfo {
    let monitors = st.map_or_else(
        || hostsvc::Monitors {
            installed: vdd::device::installed(),
            ..hostsvc::Monitors::default()
        },
        |st| st.monitors.clone(),
    );
    let real = vdd::display::real().unwrap_or_default();
    let shown = shown(&real);
    // Nothing but Lumen's on (a client's monitors parked and kept on: the
    // only display, seen through Parsec too): those, not "unknown 1920×1080".
    let active = vdd::display::active().unwrap_or_default();
    let lumen_on = active.iter().any(|a| a.vdd_id.is_some());
    let listed = if real.is_empty() { active } else { real };
    MonitorsInfo {
        state: monitors,
        real: listed
            .iter()
            .map(|a| RealMonitor {
                x: a.rect.x,
                y: a.rect.y,
                width: a.rect.width,
                height: a.rect.height,
                // Lumen's own as the owner names them, like Parsec's
                // ParsecVDA: the driver's EDID says so since Lumen VDD
                // 0.2.3; an older driver (Lumen's update leaves it) says
                // "Lumen VDD".
                name: if a.vdd_id.is_some() {
                    LUMEN_MONITOR_NAME.into()
                } else {
                    a.name.clone()
                },
            })
            .collect(),
        shown,
        allowed: vdd::layout::virtuals_allowed(shown),
        lumen_on,
        catalog: CATALOG,
    }
}

#[cfg(test)]
mod contract {
    use serde_json::{Value, json};
    use signal::contract::{check_example, steady};

    use super::*;
    use crate::settings::Local;

    #[test]
    fn host_app_json_keeps_the_settings_and_the_language() {
        // Written before the language: the default one (the installer's, else English; not "en" on
        // every machine).
        let old: Local = serde_json::from_str(r#"{"server":"a:7443","machine":"PC"}"#).unwrap();
        assert_eq!(
            (old.settings.server.as_str(), old.lang.as_str()),
            ("a:7443", desktop::lang::default().as_str())
        );
        let text = serde_json::to_string(&Local {
            lang: "ru".into(),
            ..old
        })
        .unwrap();
        let back: Local = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (back.settings.machine.as_str(), back.lang.as_str()),
            ("PC", "ru")
        );
        assert!(
            text.contains(r#""server":"a:7443""#),
            "flat, as before: {text}"
        );
    }

    fn monitors() -> MonitorsInfo {
        let settings = serde_json::from_value(json!({
            "enabled": true,
            "virtuals": [{ "width": 2560, "height": 1440, "scale": 100, "attach": { "to": 1, "side": "right", "align": "start" } }],
        }))
        .unwrap();
        MonitorsInfo {
            state: hostsvc::Monitors {
                installed: true,
                driver: "0.0.0".into(),
                settings,
                up: vec![hostsvc::MonitorUp {
                    monitor: 2,
                    width: 2560,
                    height: 1440,
                }],
                error: String::new(),
                physical: true,
                client: None,
            },
            real: vec![RealMonitor {
                x: 0,
                y: 0,
                width: 3840,
                height: 2160,
                name: "DELL".into(),
            }],
            shown: 1,
            allowed: 2,
            lumen_on: true,
            catalog: CATALOG,
        }
    }

    /// What the service says, as it says it.
    fn from_service(event: Value) -> hostsvc::State {
        let mut status = HostStatus::starting();
        status.apply(&json!({ "event": "listening", "addrs": ["10.0.0.1:9000"] }));
        status.apply(&event);
        hostsvc::State {
            registered: true,
            token: "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF".into(),
            status,
            settings: hostsvc::Settings {
                server: "a:7443".into(),
                machine: "WPC-10".into(),
                ..hostsvc::Settings::default()
            },
            ..hostsvc::State::default()
        }
    }

    #[test]
    fn the_window_has_the_same() {
        let of = |st| {
            AppState::of_service(
                st,
                Settings::default(),
                "WPC-10".into(),
                monitors(),
                "en".into(),
                None,
            )
        };
        let session = json!({ "event": "session", "client": "anna", "fps": 60, "mbps": 40.0 });
        let examples = [
            of(from_service(json!({ "event": "online" }))),
            of(from_service(session)),
            of(hostsvc::State::default()),
            AppState::without_service(
                "err_service_down|x".into(),
                true,
                "WPC-10".into(),
                monitors(),
                "en".into(),
            ),
            // The server has a newer version; the host is updating to it.
            of(from_service(
                json!({ "event": "updating", "version": "99.0.0" }),
            )),
        ];
        let mut examples = serde_json::to_value(examples).unwrap();
        // Set when the session began, by the clock.
        steady(&mut examples, &["version", "since"]);
        let Value::Array(examples) = examples else {
            unreachable!()
        };
        check_example(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../web/host/src/contract/app.example.ts"
            ),
            "AppState",
            "../api",
            &examples,
        );
    }
}
