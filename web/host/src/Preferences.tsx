import { words } from "../../client/src/i18n";
import { Icon, LogSetting, Setting, Toggle } from "../../client/src/ui";
import { api, type AppState } from "./api";
import { useRun } from "./useRun";

const W = words(
  {
    settings: "Settings",
    autostart: "Start with Windows",
    autostartText: "The host is available right after the computer is turned on, even before anyone signs in.",
    keepAwake: "Keep the computer from sleeping",
    keepAwakeText: "While the host's service runs, the computer does not go to sleep, even if sleep is set up in Windows.",
    autoUpdate: "Update when a newer client connects",
    autoUpdateText: "A client of a newer version wants to connect: the host installs the server's version by itself, the client connects after. Also with Windows locked.",
  },
  {
    settings: "Настройки",
    autostart: "Запускать вместе с Windows",
    autostartText: "Хост доступен сразу после включения компьютера, ещё до входа в систему.",
    keepAwake: "Не давать компьютеру засыпать",
    keepAwakeText: "Пока работает служба хоста, компьютер не уходит в сон, даже если сон настроен в Windows.",
    autoUpdate: "Обновляться, когда подключается клиент новее",
    autoUpdateText: "Если подключается клиент более новой версии, хост сам ставит версию с сервера, клиент подключается после. Даже при заблокированной Windows.",
  },
);

export function Preferences({ s, refresh }: { s: AppState; refresh: () => Promise<void> }) {
  const w = W();
  const { error, run } = useRun(refresh);
  const off = !!s.service_error;
  return (
    <section className="view">
      <header className="view-head">
        <Icon name="settings" size={19} />
        <h2>{w.settings}</h2>
      </header>
      {error && <p className="banner bad selectable">{error}</p>}
      <ul className="rows">
        <Setting title={w.autostart} control={<Toggle label={w.autostart} checked={s.autostart} onChange={(v) => !off && run(() => api.setAutostart(v))} />}>
          <p>{w.autostartText}</p>
        </Setting>
        <Setting title={w.keepAwake} control={<Toggle label={w.keepAwake} checked={s.keep_awake} onChange={(v) => !off && run(() => api.setKeepAwake(v))} />}>
          <p>{w.keepAwakeText}</p>
        </Setting>
        <Setting title={w.autoUpdate} control={<Toggle label={w.autoUpdate} checked={s.auto_update} onChange={(v) => !off && run(() => api.setAutoUpdate(v))} />}>
          <p>{w.autoUpdateText}</p>
        </Setting>
        <LogSetting onOpen={() => run(api.openLogFolder)} />
      </ul>
    </section>
  );
}
