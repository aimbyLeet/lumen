import { useCallback, useEffect, useState } from "react";
import { lang, setLang, words, type Lang } from "../../client/src/i18n";
import { Icon, LangToggle, TitleBar, type IconName } from "../../client/src/ui";
import { api, type AppState } from "./api";
import { Machine, Welcome } from "./Machine";
import { Monitors, MonitorsGuard } from "./Monitors";
import { Preferences } from "./Preferences";
import { UpdateFooter } from "../../client/src/Update";
import { useRun } from "./useRun";

const POLL_MS = 1000;

type Tab = "machine" | "monitors" | "settings";

/** The welcome page (a clean machine's first start): "form" — the fields; "token" — the server took the machine;
 * "done" — the usual window, for the rest of this run (a removed registration stays on the Machine tab, its note
 * there). "unknown" until the service answers. */
type Greeting = "unknown" | "form" | "token" | "done";

/** Where the welcome page goes with what the service says now. Not while it is off or silent. */
function nextGreeting(g: Greeting, s: AppState): Greeting {
  if (s.service_error || s.host_off) return g;
  if (g === "unknown") return s.registered ? "done" : "form";
  if (g !== "form" || !s.registered) return g;
  const st = s.status.state;
  if (st === "online" || st === "session") return "token";
  // Refused, offline, failed: the Machine tab says why and has what fixes it.
  return st === "starting" || st === "stopped" ? g : "done";
}

const W = words(
  {
    machine: "Machine",
    monitors: "Monitors",
    settings: "Settings",
    closeToTray: "Close to the tray",
    noDriver: "The virtual monitor driver is not installed",
    open: "Open",
    quit: "Turn off the host and quit",
  },
  {
    machine: "Машина",
    monitors: "Мониторы",
    settings: "Настройки",
    closeToTray: "Закрыть в трей",
    noDriver: "Драйвер виртуальных мониторов не установлен",
    open: "Открыть",
    quit: "Выключить хост и выйти",
  },
);

export function App() {
  const [s, setS] = useState<AppState | null>(null);
  // `?tab=`: only the stand-in for the service (src/mock) opens with another tab.
  const [tab, setTab] = useState<Tab>(() => (new URLSearchParams(location.search).get("tab") === "monitors" ? "monitors" : "machine"));

  const refresh = useCallback(async () => {
    setS(await api.state());
  }, []);
  const { error: updateError, run } = useRun(refresh);
  const [greeting, setGreeting] = useState<Greeting>("unknown");
  useEffect(() => {
    if (s) setGreeting((g) => nextGreeting(g, s));
  }, [s]);
  // Before anything below renders: the whole window speaks the chosen language.
  if (s) setLang(s.lang);
  const w = W();
  const current = lang();
  useEffect(() => {
    // Without it a click on the icon still opens the window: only the menu's words come from here.
    api.setTray({ open: w.open, quit: w.quit }).catch(console.error);
    // Again when the language changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current]);
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  const frame = <TitleBar name="Lumen Host" closeTitle={w.closeToTray} win={api.window} />;
  if (!s) return <div className="window">{frame}</div>;
  const tabs: [Tab, IconName, string][] = [
    ["machine", "desktop", w.machine],
    ["monitors", "monitors", w.monitors],
    ["settings", "settings", w.settings],
  ];
  // Without the driver the tab cannot be opened; the driver gone while it is open: back to the machine.
  const noDriver = !s.monitors.state.installed;
  const shownTab = tab === "monitors" && noDriver ? "machine" : tab;
  const onLang = (l: Lang) => api.setLang(l).then(refresh, console.error);
  // A clean machine's first start: the welcome page fills the window, without the tabs. Not while the service is
  // off or silent: the Machine tab says so and turns it on. Decided as the effect above will decide it, in the same
  // frame: the usual window is not seen for a moment behind the welcome page.
  const shown = nextGreeting(greeting, s);
  const first = !s.service_error && !s.host_off && (shown === "form" || (shown === "token" && !!s.token));
  return (
    <div className="window">
      {frame}
      {first ? (
        <main className="bare">
          <Welcome s={s} refresh={refresh} onLang={onLang} token={shown === "token" ? s.token! : undefined} onDone={() => setGreeting("done")} />
        </main>
      ) : (
        <>
          <nav className="tabs" role="tablist">
            {tabs.map(([id, icon, title]) => {
              const closed = id === "monitors" && noDriver;
              return (
                <button
                  key={id}
                  role="tab"
                  aria-selected={shownTab === id}
                  aria-label={title}
                  title={closed ? w.noDriver : title}
                  disabled={closed}
                  className={shownTab === id ? "on" : ""}
                  onClick={() => setTab(id)}
                >
                  <Icon name={icon} size={20} />
                </button>
              );
            })}
            <LangToggle onLang={onLang} />
          </nav>
          {/* All the tabs stay alive: what is typed or moved but not applied survives a look at another tab. */}
          <main>
            <div className="tab-page" hidden={shownTab !== "machine"}>
              <Machine s={s} refresh={refresh} />
            </div>
            {!noDriver && (
              <div className="tab-page" hidden={shownTab !== "monitors"}>
                <MonitorsGuard>
                  <Monitors info={s.monitors} clientName={s.status.state === "session" ? s.status.client : ""} serviceError={s.service_error} refresh={refresh} />
                </MonitorsGuard>
              </div>
            )}
            <div className="tab-page" hidden={shownTab !== "settings"}>
              <Preferences s={s} refresh={refresh} />
            </div>
          </main>
        </>
      )}
      {updateError && <p className="banner bad selectable update-error">{updateError}</p>}
      <UpdateFooter
        program="Lumen Host"
        version={s.version}
        update={s.update}
        session={s.status.state === "session"}
        run={run}
        onUpdate={api.updateNow}
      />
    </div>
  );
}
