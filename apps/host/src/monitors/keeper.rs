//! The service's keeper of the virtual monitors.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use desktop::hostsvc::{ClientMonitors, MonitorUp, Monitors as MonitorsState};
use desktop::log::Log;
use vdd::layout::{self, ClientSet, Screen, Settings};

use super::VirtualMonitors;
use super::arrange::Plan;
use super::shape::Shape;

/// How long `displays arrange` may take.
const ARRANGE_WITHIN: Duration = Duration::from_secs(30);
/// Retries: opening the driver, arranging after a failure, looking for a
/// newly signed-in user.
const OPEN_AGAIN: Duration = Duration::from_secs(5);
const ARRANGE_AGAIN: Duration = Duration::from_secs(10);
const USER_LOOK: Duration = Duration::from_secs(2);

/// A client's monitors stay this long after its session was cut off.
const LOST_KEEP: Duration = Duration::from_secs(10);

/// `%ProgramData%\Lumen\monitors.json`.
fn settings_path() -> PathBuf {
    crate::registry::program_data().join("monitors.json")
}

/// `%ProgramData%\Lumen\client-monitors.json`: the clients' own sets.
fn sets_path() -> PathBuf {
    crate::registry::program_data().join("client-monitors.json")
}

fn read_json<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Which monitors are up.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    /// The machine's (Lumen Host's «Мониторы»), next to the physical ones.
    Machine,
    /// A client's own set, alone.
    Client(String),
    /// None: no physical monitor, the client in session has no set yet (it
    /// is asked).
    Ask(String),
    /// No physical monitor, nobody in session: the last client's set kept in
    /// the driver but off (another driver's display on: Parsec's fallback);
    /// on again at once when a client comes (the owner's decision, 2026-11-12).
    Parked(String),
    /// None: no physical monitor, nobody in session, no client's set yet.
    Nothing,
}

/// What to show: the machine's monitors with a physical one; without it, the
/// set of the client in session, if it has one; nobody in session — the last
/// client's set parked.
fn decide(
    physical: bool,
    client: Option<&str>,
    last: Option<&str>,
    sets: &BTreeMap<String, ClientSet>,
) -> Shown {
    match (physical, client, last) {
        (true, _, _) => Shown::Machine,
        (false, Some(k), _) if sets.contains_key(k) => Shown::Client(k.to_string()),
        (false, Some(k), _) => Shown::Ask(k.to_string()),
        (false, None, Some(k)) if sets.contains_key(k) => Shown::Parked(k.to_string()),
        (false, None, _) => Shown::Nothing,
    }
}

/// The client the host last told of; `lost_at`: its session was cut off then.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Visit {
    key: String,
    lost_at: Option<Instant>,
}

/// The visit after a host's report.
fn visit_after(
    old: Option<Visit>,
    client: Option<String>,
    lost: bool,
    now: Instant,
) -> Option<Visit> {
    match (client, old) {
        (Some(key), _) => Some(Visit { key, lost_at: None }),
        // Cut off: kept a while (the same client may come back).
        (None, Some(v)) if lost => Some(Visit {
            lost_at: v.lost_at.or(Some(now)),
            ..v
        }),
        (None, _) => None,
    }
}

/// The service's virtual monitors: kept as the settings say, arranged on the
/// console's desktop.
pub struct Keeper {
    inner: Mutex<Inner>,
    log: Arc<Log>,
}

struct Inner {
    monitors: Option<VirtualMonitors>,
    settings: Settings,
    /// The clients' own sets, by key.
    sets: BTreeMap<String, ClientSet>,
    /// As the host last told (true until it does: the machine's monitors).
    physical: bool,
    client: Option<Visit>,
    /// What is up now (`None`: not decided yet).
    shown: Option<Shown>,
    /// The client whose set was shown last: parked when nobody is in session.
    last: Option<String>,
    /// The last change added a monitor (Windows turns it on itself).
    fresh: bool,
    error: String,
    /// When to try opening the driver again.
    open_at: Instant,
    /// The place is still to be set (as SYSTEM in the console session).
    place: bool,
    place_at: Instant,
    /// Users who got the scale of what is up; when to look for a new one.
    scaled: Vec<String>,
    user_at: Instant,
}

impl Inner {
    fn decide(&self) -> Shown {
        decide(
            self.physical,
            self.client.as_ref().map(|v| v.key.as_str()),
            self.last.as_deref(),
            &self.sets,
        )
    }

    /// What is shown, as monitors (`None`: none).
    fn shape(&self) -> Option<Shape> {
        match &self.shown {
            Some(Shown::Machine) => Some(Shape::machine(&self.settings)),
            Some(Shown::Client(k) | Shown::Parked(k)) => {
                self.sets.get(k).cloned().map(Shape::Client)
            }
            _ => None,
        }
    }

    /// The clients' sets to `client-monitors.json`.
    fn save_sets(&self) -> Result<()> {
        std::fs::write(sets_path(), serde_json::to_string_pretty(&self.sets)?)
            .context("client-monitors.json")
    }
}

impl Keeper {
    pub fn new(log: Arc<Log>) -> Arc<Keeper> {
        let now = Instant::now();
        Arc::new(Keeper {
            inner: Mutex::new(Inner {
                monitors: None,
                settings: read_json(&settings_path()),
                sets: read_json(&sets_path()),
                physical: true,
                client: None,
                shown: None,
                last: None,
                fresh: false,
                error: String::new(),
                open_at: now,
                place: false,
                place_at: now,
                scaled: Vec::new(),
                user_at: now,
            }),
            log,
        })
    }

    /// What Lumen Host shows.
    pub fn state(&self) -> MonitorsState {
        let inner = self.inner.lock().unwrap();
        let client = match &inner.shown {
            Some(Shown::Client(k) | Shown::Ask(k)) => Some(ClientMonitors {
                key: k.clone(),
                set: inner.sets.get(k).cloned(),
                parked: false,
            }),
            Some(Shown::Parked(k)) => Some(ClientMonitors {
                key: k.clone(),
                set: inner.sets.get(k).cloned(),
                parked: true,
            }),
            _ => None,
        };
        MonitorsState {
            installed: vdd::device::installed(),
            driver: inner
                .monitors
                .as_ref()
                .map(|m| m.device.info().driver_version())
                .unwrap_or_default(),
            settings: inner.settings.clone(),
            up: inner
                .monitors
                .as_ref()
                .map(|m| {
                    m.up.iter()
                        .map(|u| MonitorUp {
                            monitor: u.monitor as u8,
                            width: u.width,
                            height: u.height,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            error: inner.error.clone(),
            physical: inner.physical,
            client,
        }
    }

    /// «Создать» / «Применить» / «Отключить» / «Включить»: saved, made so and
    /// arranged before the answer (while they are up: a client's own may be
    /// instead).
    pub fn set(&self, s: vdd::layout::Settings) -> Result<()> {
        layout::check(&s)?;
        let mut inner = self.inner.lock().unwrap();
        if !vdd::device::installed() {
            bail!("err_vdd_missing");
        }
        // They are shown only next to a physical monitor: made without one
        // (through another remote access, on its virtual display) they would
        // never be seen, and would fight that display's own rules.
        if !inner.physical {
            bail!("err_monitors_no_physical");
        }
        std::fs::write(settings_path(), serde_json::to_string_pretty(&s)?)
            .context("monitors.json")?;
        self.log.line(&format!(
            "monitors: set {}",
            serde_json::to_string(&s).unwrap_or_default()
        ));
        inner.settings = s;
        self.changed(&mut inner)
    }

    /// «Сбросить»: no virtual monitors, no settings.
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let _ = std::fs::remove_file(settings_path());
        self.log.line("monitors: reset");
        inner.settings = Settings::default();
        self.changed(&mut inner)
    }

    /// The host's report: a physical monitor or not, the client in session.
    pub fn host_report(&self, physical: bool, client: Option<String>, lost: bool) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let before = (inner.physical, inner.client.clone());
        inner.physical = physical;
        inner.client = visit_after(inner.client.take(), client, lost, Instant::now());
        if before != (inner.physical, inner.client.clone()) {
            self.log.line(&format!(
                "monitors: the host says physical {physical}, client {:?}",
                inner.client
            ));
        }
        self.reconcile(&mut inner)
    }

    /// The monitor a client chose when asked: its set from now on. One it
    /// has already is kept (the answer to a repeated question).
    pub fn choose(&self, key: String, screen: Screen) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.sets.contains_key(&key) {
            let set = ClientSet::of(screen);
            set.rects()?;
            inner.sets.insert(key.clone(), set);
            inner.save_sets()?;
            self.log.line(&format!(
                "monitors: client {key} chose {}x{} at {} %",
                screen.width, screen.height, screen.scale
            ));
        }
        self.reconcile(&mut inner)
    }

    /// Lumen Host changed the set of the client whose set is up (in session,
    /// or parked after it): saved, made so in place and arranged before the
    /// answer.
    pub fn set_client(&self, key: String, set: ClientSet) -> Result<()> {
        set.rects()?;
        let mut inner = self.inner.lock().unwrap();
        if inner.shown != Some(Shown::Client(key.clone()))
            && inner.shown != Some(Shown::Parked(key.clone()))
        {
            bail!("err_client_monitors_gone");
        }
        inner.sets.insert(key.clone(), set.clone());
        inner.save_sets()?;
        self.log.line(&format!(
            "monitors: client {key} set {}",
            serde_json::to_string(&set).unwrap_or_default()
        ));
        self.changed(&mut inner)
    }

    /// Makes what is up match [`Inner::decide`], if that changed.
    fn reconcile(&self, inner: &mut Inner) -> Result<()> {
        let want = inner.decide();
        if inner.shown.as_ref() == Some(&want) {
            return Ok(());
        }
        self.log.line(&format!("monitors: now {want:?}"));
        if let Shown::Client(k) = &want {
            inner.last = Some(k.clone());
        }
        inner.shown = Some(want);
        self.changed(inner)
    }

    /// What is up changed (or its settings did): made so and arranged now.
    fn changed(&self, inner: &mut Inner) -> Result<()> {
        if inner.shown.is_none() {
            inner.shown = Some(inner.decide());
        }
        inner.scaled.clear();
        self.update(inner);
        self.arrange_now(inner);
        if inner.error.is_empty() {
            Ok(())
        } else {
            bail!("{}", inner.error)
        }
    }

    /// One look of the service (every 0.5 s): the driver opened when it
    /// appears, a cut-off client's monitors gone after a while, the place
    /// set once there is a console session, the scale for each user who
    /// signs in.
    pub fn tick(&self) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        if inner.monitors.is_none() && now >= inner.open_at && vdd::device::installed() {
            if inner.shown.is_none() {
                inner.shown = Some(inner.decide());
            }
            self.update(&mut inner);
            if inner.monitors.is_none() {
                inner.open_at = now + OPEN_AGAIN;
            }
        }
        let gone = inner
            .client
            .as_ref()
            .and_then(|v| v.lost_at)
            .is_some_and(|at| now.duration_since(at) >= LOST_KEEP);
        if gone {
            inner.client = None;
            self.log
                .line("monitors: the cut-off client did not come back");
            let _ = self.reconcile(&mut inner);
        }
        let up = inner.monitors.as_ref().is_some_and(|m| !m.up.is_empty());
        if !up {
            return;
        }
        // The place first (it sets the resolution), then the scale.
        if inner.place {
            if Instant::now() >= inner.place_at {
                self.place(&mut inner);
            }
            if inner.place {
                return;
            }
        }
        if now >= inner.user_at {
            inner.user_at = now + USER_LOOK;
            self.scale_for_user(&mut inner);
        }
    }

    /// Opens the driver if need be and makes its monitors match what is shown.
    fn update(&self, inner: &mut Inner) {
        inner.error.clear();
        if inner.monitors.is_none() {
            match VirtualMonitors::open() {
                Ok(m) => {
                    self.log.line(&format!(
                        "monitors: Lumen VDD {} opened",
                        m.device.info().driver_version()
                    ));
                    inner.monitors = Some(m);
                }
                Err(e) => {
                    inner.error = format!("{e:#}");
                    self.log.line(&format!("monitors: {e:#}"));
                    return;
                }
            }
        }
        let wanted = inner.shape().map(|s| s.wanted()).unwrap_or_default();
        let m = inner.monitors.as_mut().unwrap();
        let t = Instant::now();
        let r = m.apply(&wanted);
        let n = m.up.len();
        match r {
            Ok(added) => {
                inner.fresh = added;
                self.log.line(&format!(
                    "monitors: {n} virtual up{} ({} ms)",
                    if added { ", added" } else { "" },
                    t.elapsed().as_millis()
                ))
            }
            Err(e) => {
                inner.error = format!("{e:#}");
                self.log.line(&format!("monitors: {e:#}"));
            }
        }
        inner.place = n > 0;
        inner.place_at = Instant::now();
    }

    /// Place and scale now (a user asked).
    fn arrange_now(&self, inner: &mut Inner) {
        if inner.place {
            self.place(inner);
        }
        self.scale_for_user(inner);
    }

    fn plan(inner: &Inner) -> Option<String> {
        let m = inner.monitors.as_ref()?;
        if m.up.is_empty() {
            return None;
        }
        serde_json::to_string(&Plan {
            up: m.up.clone(),
            shape: inner.shape()?,
            park: matches!(inner.shown, Some(Shown::Parked(_))),
            wake: !inner.fresh,
        })
        .ok()
    }

    fn run(&self, token: windows::Win32::Foundation::HANDLE, plan: &str, part: &str) -> Result<()> {
        let args = ["displays", "arrange", "--part", part, "--plan", plan].map(String::from);
        let t = Instant::now();
        let (code, out) = crate::worker::run_and_wait(token, &args, ARRANGE_WITHIN)?;
        let out = out.trim();
        self.log.line(&format!(
            "monitors: {part}: exit {code} in {} ms{}{out}",
            t.elapsed().as_millis(),
            if out.is_empty() { "" } else { ": " }
        ));
        if code != 0 {
            bail!(
                "{}",
                out.lines().last().unwrap_or("displays arrange failed")
            );
        }
        Ok(())
    }

    /// Where the monitors sit: as SYSTEM in the console session.
    fn place(&self, inner: &mut Inner) {
        let Some(plan) = Self::plan(inner) else {
            inner.place = false;
            return;
        };
        let Some(session) = crate::worker::console_session() else {
            return;
        };
        let r = crate::worker::session_token(session)
            .and_then(|token| self.run(token.get(), &plan, "place"));
        match r {
            Ok(()) => inner.place = false,
            Err(e) => {
                inner.error = format!("err_monitor_place|{e:#}");
                inner.place_at = Instant::now() + ARRANGE_AGAIN;
            }
        }
    }

    /// Their scale for the user signed in on the console, once per settings.
    fn scale_for_user(&self, inner: &mut Inner) {
        let Some(plan) = Self::plan(inner) else {
            return;
        };
        let Some(user) = crate::user_files::ConsoleUser::get() else {
            return;
        };
        if inner.scaled.contains(&user.sid) {
            return;
        }
        // Tried once per user and settings: a failure is in the log and the state.
        inner.scaled.push(user.sid.clone());
        if let Err(e) = self.run(user.token(), &plan, "scale") {
            inner.error = format!("err_monitor_scale|{e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: u32, height: u32) -> Screen {
        Screen {
            width,
            height,
            scale: 100,
        }
    }

    #[test]
    fn what_is_shown() {
        let mut sets = BTreeMap::new();
        sets.insert("a".to_string(), ClientSet::of(screen(3440, 1440)));
        // A physical monitor: the machine's, whoever is in session.
        let a = Some("a");
        assert_eq!(decide(true, a, None, &sets), Shown::Machine);
        assert_eq!(decide(true, None, a, &sets), Shown::Machine);
        // None: the client's own set, or it is asked.
        assert_eq!(decide(false, a, None, &sets), Shown::Client("a".into()));
        assert_eq!(decide(false, Some("b"), a, &sets), Shown::Ask("b".into()));
        // Nobody: the last client's set parked; no set — nothing.
        assert_eq!(decide(false, None, a, &sets), Shown::Parked("a".into()));
        assert_eq!(decide(false, None, Some("b"), &sets), Shown::Nothing);
        assert_eq!(decide(false, None, None, &sets), Shown::Nothing);
    }

    #[test]
    fn a_cut_off_client_is_kept_a_while() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(3);
        let a = || Some("a".to_string());
        let v = visit_after(None, a(), false, t0).unwrap();
        assert_eq!(v.lost_at, None);
        // Closed: gone at once.
        assert_eq!(visit_after(Some(v.clone()), None, false, t0), None);
        // Cut off: kept, since the first report of it.
        let lost = visit_after(Some(v.clone()), None, true, t0).unwrap();
        assert_eq!(lost.lost_at, Some(t0));
        let again = visit_after(Some(lost.clone()), None, true, t1).unwrap();
        assert_eq!(again.lost_at, Some(t0));
        // Back: kept for good; another client: that one.
        assert_eq!(
            visit_after(Some(lost.clone()), a(), false, t1)
                .unwrap()
                .lost_at,
            None
        );
        let b = visit_after(Some(lost), Some("b".into()), false, t1).unwrap();
        assert_eq!(b.key, "b");
        // Nobody was there: nothing to keep.
        assert_eq!(visit_after(None, None, true, t0), None);
    }
}
