// Development only (`vite --mode mock`, vite.config.ts): stands in for @tauri-apps/api/core, so the window can be
// looked at in a browser without the service. `?scene=` picks what the service says (Panel.tsx); the buttons work on
// this state in memory. It answers as apps/host-app does: without the service, no settings and no token.
import type { AppState, ClientSet, MonitorSettings, Status, Virtual } from "../api";
import { lang } from "../../../client/src/i18n";

const G7 = { x: 0, y: 0, width: 3840, height: 2160, name: "Odyssey G7" };
const SECOND = { x: 3840, y: 0, width: 2560, height: 1440, name: "DELL U2723QE" };
const THIRD = { x: -1920, y: 0, width: 1920, height: 1080, name: "LG 24GN650" };

const two: Virtual[] = [
  { width: 2560, height: 1440, scale: 150, attach: { to: 1, side: "right", align: "center" } },
  { width: 1920, height: 1080, scale: 100, attach: { to: 2, side: "above", align: "start" } },
];

const stopped: Status = { state: "stopped", detail: "", client: "", fps: 0, mbps: 0, since: 0, addrs: [], server_version: "", updating: "", update_error: "" };
/** The key of the mock's server in a local network. */
const LAN_KEY = "9500 EF55 438E 47F8 F472 91B2 CE45 DCD6 998A 8F06 EDBC 34BF 914B 13E5 99DF E36A";
const TOKEN = "WSP0-9942-TK91-7718-AB3D-QX72";

type M = AppState["monitors"];
const none: MonitorSettings = { enabled: false, virtuals: [] };

function online(): AppState {
  return {
    settings: { server: "lumen.example.com:7443", machine: "WPC-10", manual_addrs: "", server_ca: "", server_fingerprint: "" },
    defaults: { machine: "WPC-10" },
    token: TOKEN,
    registered: true,
    status: { ...stopped, state: "online" },
    service_error: null,
    host_off: false,
    autostart: true,
    keep_awake: false,
    auto_update: true,
    update: { server_version: "0.0.0-mock", newer: false, phase: "", error: "" },
    version: "0.0.0-mock",
    lang: lang(),
    monitors: {
      state: { installed: true, driver: "0.2.2", settings: none, up: [], error: "", physical: true, client: null },
      real: [G7],
      shown: 1,
      allowed: 2,
      lumen_on: false,
      catalog: {
        resolutions: (
          [
            [1920, 1080, "16:9"],
            [2560, 1440, "16:9"],
            [3200, 1800, "16:9"],
            [3840, 2160, "16:9"],
            [1920, 1200, "16:10"],
            [2560, 1600, "16:10"],
            [3840, 2400, "16:10"],
            [2560, 1080, "21:9"],
            [3440, 1440, "21:9"],
            [3840, 1600, "21:9"],
            [5120, 2160, "21:9"],
          ] as const
        ).map(([width, height, aspect]) => ({ width, height, aspect })),
        scales: [100, 125, 150, 175, 200],
      },
    },
  };
}

const KEY = "4f1c9a0e7b2d";
const clientOne: ClientSet = { primary: { width: 2560, height: 1440, scale: 125 }, virtuals: [] };
const clientTwo: ClientSet = { primary: { width: 3440, height: 1440, scale: 100 }, virtuals: [{ width: 1920, height: 1080, scale: 100, attach: { to: 1, side: "right", align: "center" } }] };
const setUp = (c: ClientSet) => [c.primary, ...c.virtuals].map((m, i) => ({ monitor: i + 1, width: m.width, height: m.height }));
/** A host without a physical monitor, a client in session. */
const headless = (s: AppState, set: ClientSet | null): Partial<AppState> => ({
  ...status(s, { state: "session", client: "Иван", fps: 60, mbps: 0, since: Date.now() / 1000 - 5 * 60 }),
  monitors: {
    ...s.monitors,
    real: [],
    shown: 0,
    allowed: 2,
    lumen_on: !!set,
    state: { ...s.monitors.state, physical: false, client: { key: KEY, set, parked: false }, up: set ? setUp(set) : [] },
  },
});
/** Nobody in session: the last client's set parked — kept on (the only display, seen through Parsec) or off (Parsec's display on). */
const parked = (s: AppState, on: boolean): Partial<AppState> => ({
  monitors: {
    ...s.monitors,
    real: [on ? { x: 0, y: 0, width: 2560, height: 1440, name: "LumenVDA" } : PARSEC],
    shown: 1,
    allowed: 2,
    lumen_on: on,
    state: { ...s.monitors.state, physical: false, client: { key: KEY, set: clientOne, parked: true }, up: setUp(clientOne) },
  },
});

const upOf = (s: Virtual[], real: number) => s.map((v, i) => ({ monitor: i + real + 1, width: v.width, height: v.height }));

const notRegistered = (s: AppState): Partial<AppState> => ({
  registered: false,
  token: null,
  status: stopped,
  settings: { ...s.settings, server: "", machine: s.defaults.machine },
});
/** apps/host-app, get_state without an answer from the service. */
const noAnswer = (s: AppState, error: string, hostOff: boolean): Partial<AppState> => ({
  ...notRegistered(s),
  autostart: false,
  keep_awake: false,
  service_error: error,
  host_off: hostOff,
  monitors: { ...s.monitors, state: { ...s.monitors.state, settings: none, up: [] } },
});
// As the core says it (web/client/src/errors.ts has the words).
const NO_SERVICE = "err_service_down|The system cannot find the file specified. (os error 2)";
const status = (s: AppState, st: Partial<Status>): Partial<AppState> => ({ status: { ...s.status, ...st } });
const refused = (detail: string) => (s: AppState) => status(s, { state: "refused", detail });
const mon = (s: AppState, m: Partial<M> & { settings?: MonitorSettings; error?: string; installed?: boolean; physical?: boolean; up?: M["state"]["up"] }): Partial<AppState> => {
  const { settings, error, installed, physical, up, ...rest } = m;
  const st = s.monitors.state;
  return {
    monitors: {
      ...s.monitors,
      ...rest,
      state: {
        ...st,
        settings: settings ?? st.settings,
        error: error ?? st.error,
        installed: installed ?? st.installed,
        physical: physical ?? st.physical,
        up: up ?? st.up,
      },
    },
  };
};
/** The machine's virtual monitors saved; no physical monitor, nobody in session (Parsec on its own virtual display). */
const PARSEC = { x: 0, y: 0, width: 1920, height: 1080, name: "ParsecVDA" };
const wide: Virtual[] = [{ width: 3440, height: 1440, scale: 100, attach: { to: 1, side: "right", align: "start" } }];

type Scene = { title: string; tab?: "monitors"; make: (s: AppState) => Partial<AppState> };

export const SCENES: Record<string, Scene> = {
  online: { title: "В сети", make: () => ({}) },
  first: { title: "Первый запуск: сервера нет", make: notRegistered },
  session: { title: "Идёт сессия", make: (s) => status(s, { state: "session", client: "Иван", fps: 60, mbps: 0, since: Date.now() / 1000 - 72 * 60 }) },
  "session-cap": { title: "Идёт сессия: потолок 40 Мбит/с", make: (s) => status(s, { state: "session", client: "Мария Колористова", fps: 30, mbps: 40, since: Date.now() / 1000 - 8 * 60 }) },
  starting: { title: "Запуск", make: (s) => status(s, { state: "starting" }) },
  off: { title: "Хост выключен", make: (s) => noAnswer(s, NO_SERVICE, true) },
  noservice: { title: "Служба не отвечает", make: (s) => noAnswer(s, NO_SERVICE, false) },
  offline: { title: "Нет связи с сервером", make: (s) => status(s, { state: "offline", detail: "timed out" }) },
  unknown: { title: "Сервер не знает машину", make: refused("BadCredentials") },
  taken: { title: "Токен занят", make: refused("TokenTaken") },
  badname: { title: "Имя не принято", make: refused("BadName") },
  version: { title: "Другая версия сервера", make: refused("Version") },
  outdated: { title: "Версия устарела: жёлтый футер", make: () => ({ update: { server_version: "0.7.1", newer: true, phase: "", error: "" } }) },
  updating: { title: "Хост обновляется", make: () => ({ update: { server_version: "0.7.1", newer: true, phase: "downloading", error: "" } }) },
  updateFailed: { title: "Обновление не удалось", make: () => ({ update: { server_version: "0.7.1", newer: true, phase: "", error: "err_update_install" } }) },
  error: { title: "Ошибка хоста", make: (s) => status(s, { state: "error", detail: "NVENC is not available" }) },
  nodriver: { title: "Драйвер не установлен: «Мониторы» закрыты", make: (s) => mon(s, { installed: false }) },
  "mon-none": { title: "Мониторы: виртуальных нет", tab: "monitors", make: () => ({}) },
  "mon-on": { title: "Мониторы: включены", tab: "monitors", make: (s) => mon(s, { settings: { enabled: true, virtuals: two }, up: upOf(two, 1) }) },
  "mon-off": { title: "Мониторы: выключены", tab: "monitors", make: (s) => mon(s, { settings: { enabled: false, virtuals: two } }) },
  "mon-error": {
    title: "Мониторы: не созданы",
    tab: "monitors",
    make: (s) => mon(s, { settings: { enabled: true, virtuals: two }, error: "err_monitor_place|Windows did not turn the virtual monitors on" }),
  },
  "mon-two": { title: "Мониторы: два настоящих", tab: "monitors", make: (s) => mon(s, { real: [G7, SECOND], shown: 2, allowed: 1 }) },
  "mon-three": { title: "Мониторы: три настоящих", tab: "monitors", make: (s) => mon(s, { real: [G7, SECOND, THIRD], shown: 3, allowed: 0 }) },
  "mon-many": {
    title: "Мониторы: виртуальных больше, чем можно",
    tab: "monitors",
    make: (s) => {
      const list: Virtual[] = [
        { width: 2560, height: 1440, scale: 150, attach: { to: 2, side: "right", align: "start" } },
        { width: 1920, height: 1080, scale: 100, attach: { to: 3, side: "above", align: "start" } },
      ];
      return mon(s, { real: [G7, SECOND], shown: 2, allowed: 1, settings: { enabled: true, virtuals: list }, up: upOf(list, 2) });
    },
  },
  "mon-client": { title: "Мониторы клиента: один", tab: "monitors", make: (s) => headless(s, clientOne) },
  "mon-client-two": { title: "Мониторы клиента: два", tab: "monitors", make: (s) => headless(s, clientTwo) },
  "mon-client-ask": { title: "Мониторы клиента: выбирает", tab: "monitors", make: (s) => headless(s, null) },
  "mon-foreign": {
    title: "Мониторы: нет физического, вход через Parsec",
    tab: "monitors",
    make: (s) => mon(s, { real: [PARSEC], shown: 1, allowed: 2, physical: false, settings: { enabled: true, virtuals: wide } }),
  },
  "mon-parked": { title: "Мониторы: нет физического, мониторы клиента припаркованы включёнными (вход через Parsec)", tab: "monitors", make: (s) => parked(s, true) },
  "mon-parked-off": { title: "Мониторы: нет физического, мониторы клиента припаркованы выключенными (включён ParsecVDA)", tab: "monitors", make: (s) => parked(s, false) },
  "mon-host-off": { title: "Мониторы: хост выключен", tab: "monitors", make: (s) => noAnswer(s, NO_SERVICE, true) },
};

export const sceneName = new URLSearchParams(location.search).get("scene") ?? "online";
const scene = SCENES[sceneName] ?? SCENES.online;
if (scene.tab && !new URLSearchParams(location.search).has("tab")) history.replaceState(null, "", `${location.search || "?"}&tab=${scene.tab}`);
const base = online();
let state: AppState = { ...base, ...scene.make(base) };

const later = () => new Promise((r) => setTimeout(r, 300));

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const a = (args ?? {}) as any;
  switch (cmd) {
    case "get_state":
      return structuredClone(state) as T;
    case "probe":
      await later();
      // An address with a dot answers; 192.168.x.x has its own certificate (a local network).
      if (!String(a.server).includes(".")) throw "err_probe_timeout";
      if (String(a.server).startsWith("192.168.") && a.serverFingerprint !== LAN_KEY) throw `err_cert_untrusted|${LAN_KEY}`;
      return undefined as T;
    case "save": {
      const server = String(a.settings.server).includes(":") ? a.settings.server : `${a.settings.server}:7443`;
      state = { ...state, settings: { ...a.settings, server }, registered: true, token: state.token ?? TOKEN, status: { ...stopped, state: "starting" } };
      // The service reaches the server a moment later.
      setTimeout(() => (state = { ...state, status: { ...state.status, state: "online" } }), 1500);
      return undefined as T;
    }
    case "reregister":
      state = { ...state, status: { ...stopped, state: "online" } };
      return undefined as T;
    case "new_token": {
      const taken = state.status.state === "refused" && state.status.detail === "TokenTaken";
      state = { ...state, token: "NEW7-T0KN-A1B2-C3D4-E5F6-G7H8", status: taken ? { ...stopped, state: "online" } : state.status };
      return state.token as T;
    }
    case "set_tray":
      ((window as any).__tray = a);
      return undefined as T;
    case "set_lang":
      if (a.lang !== "en" && a.lang !== "ru") throw "err_lang";
      state = { ...state, lang: a.lang };
      return undefined as T;
    case "unregister":
      state = { ...state, ...notRegistered(state) };
      return "note_removed" as T;
    case "set_autostart":
      state = { ...state, autostart: a.enabled };
      return undefined as T;
    case "set_auto_update":
      state = { ...state, auto_update: a.enabled };
      return undefined as T;
    case "update_now":
      state = { ...state, update: { ...state.update, phase: "downloading" } };
      return undefined as T;
    case "set_keep_awake":
      state = { ...state, keep_awake: a.enabled };
      return undefined as T;
    case "start_host":
      state = { ...online(), lang: state.lang };
      return undefined as T;
    case "open_log_folder":
      return undefined as T;
    case "set_monitors":
      await later();
      state = { ...state, ...mon(state, { settings: a.settings, error: "", up: a.settings.enabled ? upOf(a.settings.virtuals, state.monitors.real.length) : [] }) };
      return undefined as T;
    case "set_client_monitors": {
      await later();
      const st = state.monitors.state;
      if (st.client?.key !== a.client) throw "err_client_monitors_gone";
      state = { ...state, monitors: { ...state.monitors, state: { ...st, client: { key: a.client, set: a.set, parked: st.client?.parked ?? false }, up: setUp(a.set), error: "" } } };
      return undefined as T;
    }
    case "reset_monitors":
      await later();
      state = { ...state, ...mon(state, { settings: none, error: "", up: [] }) };
      return undefined as T;
    default:
      throw `mock: ${cmd}`;
  }
}
