//! A card: a host's token (on this client's one server), what the server last
//! said about it, and what its one button besides «Удалить» does (words are the UI's).

use serde::{Deserialize, Serialize};
use signal::{APP_VERSION, Access, Presence};

use crate::server::ServerLive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Card {
    pub(crate) id: u64,
    /// The host's token, as the user typed it (normalised). In the file —
    /// sealed for this Windows account (`desktop::secret`); plain text from
    /// before 0.6.0 is read and written again sealed.
    #[serde(with = "desktop::secret::field")]
    pub(crate) token: String,
    /// As the server last said.
    #[serde(default)]
    pub(crate) machine: String,
    #[serde(default)]
    pub(crate) account: String,
}

/// What the server last said about a card (not saved).
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct CardLive {
    /// None: not asked yet; false: no host has this token.
    pub(crate) found: Option<bool>,
    pub(crate) access: String,
    pub(crate) presence: String,
    /// The Lumen version the host runs (empty: unknown).
    pub(crate) host_version: String,
    /// The host's last update failed: its code (empty: no failure).
    pub(crate) update_failed: String,
    /// The host updates itself when a newer client connects (its switch).
    pub(crate) auto_update: bool,
}

/// The card's one button besides «Удалить»: what it does, whether it can be
/// pressed now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Button {
    /// `connect`, `request`; `pending`, `rejected`: nothing to do but wait.
    pub(crate) op: &'static str,
    pub(crate) enabled: bool,
}

/// Another version on the host: "" — the same (or
/// not known yet); `on_connect` — older, it updates itself when this client
/// connects (the server tells it; the video window waits); `off` — older, its
/// switch is off; `newer` — this client updates first (its footer).
pub(crate) fn host_update(live: &CardLive) -> &'static str {
    let v = live.host_version.as_str();
    if v.is_empty() || v == APP_VERSION {
        ""
    } else if signal::update::newer(APP_VERSION, v) {
        if live.auto_update {
            "on_connect"
        } else {
            "off"
        }
    } else {
        "newer"
    }
}

pub(crate) fn button(live: &CardLive, srv: &ServerLive) -> Button {
    let request = |enabled| Button {
        op: "request",
        enabled,
    };
    let connect = |enabled| Button {
        op: "connect",
        enabled,
    };
    let wait = |op| Button { op, enabled: false };
    if srv.unknown {
        return request(true);
    }
    let reachable = srv.error.is_empty();
    match (live.found, live.access.as_str()) {
        (None, _) | (Some(false), _) => connect(false),
        (_, "none") => request(reachable),
        (_, "pending") => wait("pending"),
        (_, "rejected") => wait("rejected"),
        _ => match host_update(live) {
            "" => connect(reachable && live.presence == "online"),
            "on_connect" => Button {
                op: "update",
                enabled: reachable && live.presence == "online",
            },
            "off" => wait("update_off"),
            _ => wait("update_client"),
        },
    }
}

pub(crate) fn access_text(a: Access) -> &'static str {
    match a {
        Access::None => "none",
        Access::Pending => "pending",
        Access::Approved => "approved",
        Access::Rejected => "rejected",
    }
}

pub(crate) fn presence_text(p: Presence) -> &'static str {
    match p {
        Presence::Offline => "offline",
        Presence::Online => "online",
        Presence::Busy => "busy",
        Presence::Inactive => "inactive",
        Presence::Updating => "updating",
        Presence::Restarting => "restarting",
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    fn live(access: &str, presence: &str) -> CardLive {
        CardLive {
            found: Some(true),
            access: access.into(),
            presence: presence.into(),
            host_version: APP_VERSION.into(),
            update_failed: String::new(),
            auto_update: true,
        }
    }

    #[test]
    fn one_button_by_the_state() {
        let ok = ServerLive::default();
        let at = |l: &CardLive, s: &ServerLive| {
            let b = button(l, s);
            (b.op, b.enabled)
        };
        assert_eq!(at(&live("approved", "online"), &ok), ("connect", true));
        assert_eq!(at(&live("approved", "offline"), &ok), ("connect", false));
        assert_eq!(at(&live("approved", "busy"), &ok), ("connect", false));
        let host = |version: &str, presence: &str, auto_update: bool| CardLive {
            host_version: version.into(),
            auto_update,
            ..live("approved", presence)
        };
        // An older host that updates itself: «Update and connect»; one whose
        // switch is off: nothing to do; a newer one: this client updates first.
        let older = host("0.0.1", "online", true);
        assert_eq!(host_update(&older), "on_connect");
        assert_eq!(at(&older, &ok), ("update", true));
        assert_eq!(at(&host("0.0.1", "offline", true), &ok), ("update", false));
        assert_eq!(at(&host("0.0.1", "updating", true), &ok), ("update", false));
        let off = host("0.0.1", "online", false);
        assert_eq!(host_update(&off), "off");
        assert_eq!(at(&off, &ok), ("update_off", false));
        let newer = host("99.0.0", "online", true);
        assert_eq!(host_update(&newer), "newer");
        assert_eq!(at(&newer, &ok), ("update_client", false));
        assert_eq!(host_update(&host("", "online", false)), "");
        assert_eq!(host_update(&live("approved", "online")), "");
        assert_eq!(at(&live("none", "online"), &ok), ("request", true));
        assert_eq!(at(&live("pending", "online"), &ok), ("pending", false));
        assert_eq!(at(&live("rejected", "online"), &ok), ("rejected", false));
        assert_eq!(
            at(&CardLive::default(), &ok),
            ("connect", false),
            "not asked yet"
        );

        let down = ServerLive {
            error: "no link".into(),
            ..ServerLive::default()
        };
        assert_eq!(at(&live("approved", "online"), &down), ("connect", false));
        assert_eq!(at(&live("none", "online"), &down), ("request", false));
        let unknown = ServerLive {
            error: "err_unknown_device".into(),
            unknown: true,
            ..ServerLive::default()
        };
        assert_eq!(at(&live("approved", "online"), &unknown), ("request", true));
        assert_eq!(at(&CardLive::default(), &unknown), ("request", true));
    }
}
