import { useState, type ReactNode } from "react";
import { api, type AppState, type Run, type Settings } from "./api";
import { words } from "./i18n";
import { Icon, Keys, LogSetting, nameProblem, Setting, Toggle } from "./ui";
import { fps, mbps, serverField, serverName, U } from "./units";
import { useIdentity } from "./Welcome";

const W = words(
  {
    byServer: (what: string, server: string) => `${what} is set by the server ${server}. It cannot be changed.`,
    settings: "Settings",
    yourName: "Your name",
    enterName: "Enter your name",
    nameSeen: "The server's administrator sees it when approving a connection.",
    nameLocked: "To change the name, remove all machines from the list.",
    server: "Server",
    serverText: "Your organisation's Lumen server: the machines are there.",
    serverLocked: "To change the server, remove all machines from the list.",
    fps: "Frame rate",
    fpsForced: "The frame rate",
    fpsText: "How many frames per second the host sends.",
    cap: "Bitrate cap",
    capForced: "The cap",
    capText: "The host sends no more than this. Auto: the server sets it by the resolution and frame rate.",
    borderless: "Video window without a frame",
    borderlessText: "Open over the whole monitor. The picture fits with its proportions kept. In a session:",
    shortcuts: "System shortcuts to the remote machine",
    shortcutsText: (keys: (c: string) => ReactNode) => (
      <>
        {keys("Win")}, {keys("Alt+Tab")} and others go to the host while the video window is in front. {keys("Ctrl+Alt+Del")} and{" "}
        {keys("Win+L")} always stay on this computer.
      </>
    ),
    keepUnlocked: "Do not lock Windows after the session",
    keepUnlockedText: "When the session ends, the machine stays as it is: programs open, no password to enter.",
    autoUpdate: "Update automatically",
    autoUpdateText: "At start, Lumen Client installs the newer version if the server has one. Without it, the yellow footer offers the update.",
  },
  {
    byServer: (what: string, server: string) => `${what} задаёт сервер ${server}. Изменить нельзя.`,
    settings: "Настройки",
    yourName: "Ваше имя",
    enterName: "Введите имя",
    nameSeen: "Его видит администратор сервера, когда одобряет подключение.",
    nameLocked: "Чтобы изменить имя, удалите все машины из списка.",
    server: "Сервер",
    serverText: "Сервер Lumen вашей организации: машины — на нём.",
    serverLocked: "Чтобы сменить сервер, удалите все машины из списка.",
    fps: "Частота кадров",
    fpsForced: "Частоту",
    fpsText: "Сколько кадров в секунду передаёт хост.",
    cap: "Потолок битрейта",
    capForced: "Потолок",
    capText: "Больше этого хост не передаёт. Авто: значение задаёт сервер по разрешению и частоте.",
    borderless: "Окно видео без рамки",
    borderlessText: "Открывать на весь монитор. Картинка вписывается с сохранением пропорций. В сессии:",
    shortcuts: "Системные сочетания на удалённую машину",
    shortcutsText: (keys: (c: string) => ReactNode) => (
      <>
        {keys("Win")}, {keys("Alt+Tab")} и другие уходят на хост, пока окно видео впереди. {keys("Ctrl+Alt+Del")} и{" "}
        {keys("Win+L")} всегда остаются на этом компьютере.
      </>
    ),
    keepUnlocked: "Не блокировать Windows после сессии",
    keepUnlockedText: "Когда сессия кончается, машина остаётся как есть: программы открыты, пароль вводить не нужно.",
    autoUpdate: "Обновлять автоматически",
    autoUpdateText: "При запуске Lumen Client ставит новую версию, если она есть на сервере. Без этого обновиться предложит жёлтый футер.",
  },
);

function ByServer({ server, what }: { server: string; what: string }) {
  return (
    <p className="by-server">
      <Icon name="lock" size={14} />
      {W().byServer(what, serverName(server))}
    </p>
  );
}

const keys = (combo: string) => <Keys combo={combo} />;

export function Preferences({ s, run }: { s: AppState; run: Run }) {
  const w = W();
  // The server as a field shows it: the default port left out.
  const [prefs, setPrefs] = useState<Settings>({ ...s.settings, server: serverField(s.settings.server) });
  const [nameError, setNameError] = useState("");
  const server = s.settings.server;
  const forcedFps = s.forced.fps;
  const forcedCap = s.forced.mbps;
  // The name and the server: what the server knows this computer by and where; they stay while there are machines.
  const locked = s.cards.length > 0;
  const id = useIdentity(() => run(async () => {}));

  /** Everything but the name and the server: those count once kept (`saveIdentity`). */
  const apply = (change: Partial<Settings>) => {
    const next = { ...prefs, ...change };
    setPrefs(next);
    run(() => api.saveSettings(next));
  };
  const saveIdentity = () => {
    const name = prefs.name.trim();
    const srv = prefs.server.trim();
    if (locked || (name === s.settings.name && srv === serverField(server))) return;
    const problem = nameProblem(name);
    setNameError(problem);
    if (problem) return;
    id.save(name, srv);
  };

  return (
    <section className="view">
      <header className="view-head">
        <Icon name="settings" size={19} />
        <h2>{w.settings}</h2>
      </header>
      <ul className="rows">
        <Setting
          title={w.yourName}
          control={
            <input
              type="text"
              aria-label={w.yourName}
              value={prefs.name}
              placeholder={w.enterName}
              disabled={locked}
              onChange={(e) => setPrefs({ ...prefs, name: e.target.value })}
              onBlur={saveIdentity}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
          }
        >
          <p>{w.nameSeen}</p>
          {locked && (
            <p className="by-server">
              <Icon name="lock" size={14} />
              {w.nameLocked}
            </p>
          )}
          {nameError && <p className="field-error">{nameError}</p>}
        </Setting>
        <Setting
          title={w.server}
          control={
            <input
              type="text"
              className="mono"
              aria-label={w.server}
              value={prefs.server}
              placeholder="lumen.example.com"
              disabled={locked || id.saving}
              onChange={(e) => setPrefs({ ...prefs, server: e.target.value })}
              onBlur={saveIdentity}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
          }
        >
          <p>{w.serverText}</p>
          {locked && (
            <p className="by-server">
              <Icon name="lock" size={14} />
              {w.serverLocked}
            </p>
          )}
          {id.error && <p className="field-error selectable">{id.error}</p>}
          {id.dialog}
        </Setting>
        <Setting
          title={w.fps}
          control={
            <select
              aria-label={w.fps}
              disabled={forcedFps !== null}
              value={forcedFps ?? prefs.fps}
              onChange={(e) => apply({ fps: Number(e.target.value) })}
            >
              {s.fps_choices.map((f) => (
                <option key={f} value={f}>
                  {fps(f)}
                </option>
              ))}
            </select>
          }
        >
          {forcedFps !== null ? <ByServer server={server} what={w.fpsForced} /> : <p>{w.fpsText}</p>}
        </Setting>
        <Setting
          title={w.cap}
          control={
            <select
              aria-label={w.cap}
              disabled={forcedCap !== null}
              value={forcedCap ?? prefs.mbps}
              onChange={(e) => apply({ mbps: Number(e.target.value) })}
            >
              <option value={0}>{U().auto}</option>
              {s.mbps_presets.map((m) => (
                <option key={m} value={m}>
                  {mbps(m)}
                </option>
              ))}
            </select>
          }
        >
          {forcedCap !== null ? <ByServer server={server} what={w.capForced} /> : <p>{w.capText}</p>}
        </Setting>
        <Setting
          title={w.borderless}
          control={<Toggle label={w.borderless} checked={prefs.borderless} onChange={(v) => apply({ borderless: v })} />}
        >
          <p>
            {w.borderlessText} <Keys combo="Ctrl+Alt+Shift+F" />
          </p>
        </Setting>
        <Setting
          title={w.shortcuts}
          control={<Toggle label={w.shortcuts} checked={prefs.shortcuts_to_host} onChange={(v) => apply({ shortcuts_to_host: v })} />}
        >
          <p>{w.shortcutsText(keys)}</p>
        </Setting>
        <Setting
          title={w.keepUnlocked}
          control={<Toggle label={w.keepUnlocked} checked={prefs.keep_unlocked} onChange={(v) => apply({ keep_unlocked: v })} />}
        >
          <p>{w.keepUnlockedText}</p>
        </Setting>
        <Setting
          title={w.autoUpdate}
          control={<Toggle label={w.autoUpdate} checked={prefs.auto_update} onChange={(v) => apply({ auto_update: v })} />}
        >
          <p>{w.autoUpdateText}</p>
        </Setting>
        <LogSetting onOpen={() => run(api.openLogFolder)} />
      </ul>
    </section>
  );
}
