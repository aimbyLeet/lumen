import { useCallback, useEffect, useState } from "react";
import { api, type AppState } from "./api";
import { errorText } from "./errors";
import { Help } from "./Help";
import { setLang, words } from "./i18n";
import { Machines } from "./Machines";
import { MonitorDialog } from "./MonitorDialog";
import { Preferences } from "./Preferences";
import { Footer } from "./Update";
import { Welcome } from "./Welcome";
import { Icon, LangToggle, TitleBar, type IconName } from "./ui";

const POLL_MS = 1000;

type Tab = "machines" | "settings" | "help";

const W = words(
  { machines: "Machines", settings: "Settings", help: "Help", close: "Close", hide: "Hide" },
  { machines: "Машины", settings: "Настройки", help: "Справка", close: "Закрыть", hide: "Скрыть" },
);

export function App() {
  const [s, setS] = useState<AppState | null>(null);
  const [tab, setTab] = useState<Tab>("machines");
  const [error, setError] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  // The host's question already answered (its number): the dialog waits for the monitor no more.
  const [answered, setAnswered] = useState(0);
  const asking = s?.video?.status.event === "monitor_wanted" ? s.video.status : null;

  const refresh = useCallback(async () => setS(await api.state()), []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, POLL_MS);
    return () => clearInterval(t);
  }, [refresh]);

  // Before anything below renders: the whole window speaks the chosen language.
  if (s) setLang(s.settings.lang);
  const w = W();
  // The first start: no name or server yet — the welcome page fills the window, without the tabs.
  const first = !!s && (!s.settings.name || !s.settings.server);
  const tabs: [Tab, IconName, string][] = [
    ["machines", "devices", w.machines],
    ["settings", "settings", w.settings],
    ["help", "help", w.help],
  ];

  const run = async (f: () => Promise<unknown>) => {
    setError("");
    setNote("");
    setBusy(true);
    try {
      await f();
      return true;
    } catch (e) {
      setError(errorText(e));
      return false;
    } finally {
      setBusy(false);
      await refresh();
    }
  };

  return (
    <div className="window">
      <TitleBar name="Lumen Client" closeTitle={w.close} win={api.window} />
      {/* Nothing before the core answers: neither the tabs nor the welcome page is known yet. */}
      {s && !first && (
      <nav className="tabs" role="tablist">
        {tabs.map(([id, icon, title]) => (
          <button key={id} role="tab" aria-selected={tab === id} aria-label={title} title={title} className={tab === id ? "on" : ""} onClick={() => setTab(id)}>
            <Icon name={icon} size={20} />
          </button>
        ))}
        <LangToggle onLang={(l) => run(() => api.setLang(l))} />
      </nav>
      )}
      <main key={tab} className={first ? "bare" : undefined}>
        {error && (
          <p className="banner bad selectable">
            {error}
            <button className="ghost-icon" title={w.hide} onClick={() => setError("")}>
              <Icon name="close" />
            </button>
          </p>
        )}
        {note && (
          <p className="banner selectable">
            {note}
            <button className="ghost-icon" title={w.hide} onClick={() => setNote("")}>
              <Icon name="close" />
            </button>
          </p>
        )}
        {first ? (
          <Welcome onDone={() => run(async () => {})} onLang={(l) => run(() => api.setLang(l))} />
        ) : (
          s &&
          (tab === "machines" ? (
            <Machines s={s} busy={busy} run={run} onNote={(n) => setNote(errorText(n))} />
          ) : tab === "settings" ? (
            <Preferences s={s} run={run} />
          ) : (
            <Help />
          ))
        )}
      </main>
      {s ? <Footer s={s} run={run} /> : <footer className="statusbar" />}
      {asking && asking.ask !== answered && (
        <MonitorDialog
          machine={s?.cards.find((c) => c.id === s.video?.card)?.machine ?? ""}
          question={asking}
          onCancel={() => run(() => api.disconnect())}
          onChoose={(m) => {
            setAnswered(asking.ask);
            run(() => api.chooseMonitor(m));
          }}
        />
      )}
    </div>
  );
}
