// Development only (`vite --mode mock`, vite.config.ts): stands in for
// @tauri-apps/api/core, so the hub can be looked at in a browser without the
// app or a server. `?scene=` picks what the app says (SCENES). The buttons work
// on this in-memory state; the token "bad" fails to add; in the welcome window a
// server named "bad" does not answer, one at 192.168.x.x asks about its key.
import type { AppState, Card, ButtonOp, Presence, VideoStatus } from "../api";
import { lang } from "../i18n";

const VERSION = "0.5.5";
const SERVER = "lumen.example.com:7443";
const TOKEN = "WSP0-9942-TK91-7718-AB3D-QX72";
/** The key of the mock's server in a local network (192.168.x.x): asked about until trusted. */
const LAN_KEY = "9500 EF55 438E 47F8 F472 91B2 CE45 DCD6 998A 8F06 EDBC 34BF 914B 13E5 99DF E36A";
const trusted = new Set<string>();

function card(id: number, machine: string, over: Partial<Card> & { auto?: boolean } = {}): Card {
  const c: Card = {
    id,
    token: TOKEN,
    machine,
    account: "",
    found: true,
    access: "approved",
    presence: "online",
    host_version: VERSION,
    update_failed: "",
    host_update: "",
    server_error: "",
    server_unknown: false,
    button: { op: "connect", enabled: true },
    ...over,
  };
  // As the core works it out (apps/client-app/src/card.rs): `off` from the host's switch (`auto` here, the mock's only).
  const v = c.host_version;
  const update: Card["host_update"] =
    !v || v === VERSION ? "" : older(v, VERSION) ? ((over as { auto?: boolean }).auto === false ? "off" : "on_connect") : "newer";
  const done = { ...c, host_update: update };
  delete (done as { auto?: boolean }).auto;
  return { ...done, button: buttonOf(done) };
}

/** `a` is a lower version than `b` (protocol::update::newer, for the mock's versions). */
function older(a: string, b: string): boolean {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i];
  return false;
}

/** apps/client-app/src/card.rs: `button`. */
function buttonOf(c: Card): { op: ButtonOp; enabled: boolean } {
  const reachable = !c.server_error;
  if (c.server_unknown) return { op: "request", enabled: true };
  if (c.found !== true) return { op: "connect", enabled: false };
  if (c.access === "none") return { op: "request", enabled: reachable };
  if (c.access === "pending") return { op: "pending", enabled: false };
  if (c.access === "rejected") return { op: "rejected", enabled: false };
  return {
    op: c.host_update === "on_connect" ? "update" : c.host_update === "off" ? "update_off" : c.host_update === "newer" ? "update_client" : "connect",
    enabled: reachable && c.presence === "online" && (c.host_update === "" || c.host_update === "on_connect"),
  };
}

const UNREACHABLE: VideoStatus = {
  event: "unreachable",
  routes: [
    { host: "192.168.100.41:9000", adapter: "happ-xray", via: "172.19.0.1", default: true },
    { host: "10.8.0.2:9000", adapter: "wg0" },
  ],
  host_nets: ["192.168.100.0/24", "10.8.0.0/24"],
  client_nets: ["172.16.82.0/24"],
};
const STREAMING: VideoStatus = {
  event: "streaming",
  host: "192.168.100.41:9000",
  host_net: "192.168.100.0/24",
  via: "172.16.82.1",
  adapter: "wg0",
  client_net: "172.16.82.0/24",
};

// As the video window says it (apps/client/src/monitor_pick.rs): this computer is 3440x1440 at 125 %.
const MONITOR_WANTED: VideoStatus = {
  event: "monitor_wanted",
  ask: 1,
  suggest: { width: 3440, height: 1440, scale: 125 },
  resolutions: [
    [1920, 1080, "16:9"], [2560, 1440, "16:9"], [3200, 1800, "16:9"], [3840, 2160, "16:9"],
    [1920, 1200, "16:10"], [2560, 1600, "16:10"], [3840, 2400, "16:10"],
    [2560, 1080, "21:9"], [3440, 1440, "21:9"], [3840, 1600, "21:9"], [5120, 2160, "21:9"],
  ].map(([width, height, aspect]) => ({ width: width as number, height: height as number, aspect: aspect as string })),
  scales: [100, 125, 150, 175, 200],
};

type Scene = Partial<Pick<AppState, "cards" | "forced" | "video" | "last_video" | "update">> & { name?: string; server?: string };
const NEWER = { server_version: "0.7.1", newer: true, phase: "", error: "" } as const;

const CONCEPT = () => [card(1, "WPC-10"), card(2, "Render-Workstation"), card(3, "Dev-Server", { presence: "offline" })];

export const SCENES: Record<string, { title: string; make: () => Scene }> = {
  concept: { title: "Как в концепте", make: () => ({ cards: CONCEPT() }) },
  first: { title: "Первый запуск: приветствие", make: () => ({ cards: [], name: "", server: "" }) },
  empty: { title: "Имя и сервер есть, машин нет", make: () => ({ cards: [] }) },
  access: {
    title: "Доступ и заявки",
    make: () => ({
      cards: [
        card(1, "WPC-10", { access: "none" }),
        card(2, "Render-Workstation", { access: "pending" }),
        card(3, "Dev-Server", { access: "rejected" }),
      ],
    }),
  },
  problems: {
    title: "Занят, версия, сервер",
    make: () => ({
      cards: [
        card(1, "WPC-10", { presence: "busy" }),
        card(2, "Render-Workstation", { presence: "inactive" }),
        card(3, "Dev-Server", { host_version: "0.5.2" }),
        card(8, "Studio-06", { host_version: "0.5.2", presence: "updating" }),
        card(9, "Studio-07", { host_version: "0.5.2", update_failed: "err_update_install" }),
        card(4, "Studio-01", { server_error: "нет связи (timed out)" }),
        card(5, "Studio-02", { server_unknown: true }),
        card(6, "Studio-03", { found: false }),
        card(7, "Studio-04", { found: null, access: "", presence: "" }),
      ],
    }),
  },
  versions: {
    title: "Версии хоста",
    make: () => ({
      cards: [
        card(1, "WPC-10"),
        card(2, "Render-01", { host_version: "0.5.2" }),
        card(3, "Render-02", { host_version: "0.5.2", presence: "offline" }),
        card(4, "Render-03", { host_version: "0.5.2", presence: "updating" }),
        card(5, "Render-04", { host_version: "0.5.2", update_failed: "err_update_install" }),
        card(6, "Render-05", { host_version: "0.5.2", auto: false }),
        card(7, "Render-06", { host_version: "0.6.0" }),
      ],
      video: { card: 4, since: 0, status: { event: "waiting", reason: "host_updating" } },
    }),
  },
  cert: {
    title: "Сервер в локальной сети: ключ сменился",
    make: () => ({
      server: "192.168.1.10:7443",
      cards: [
        card(1, "WPC-10", { server_error: `err_cert_changed|${LAN_KEY}` }),
        card(2, "Render-Workstation", { server_error: `err_cert_changed|${LAN_KEY}` }),
      ],
    }),
  },
  connecting: {
    title: "Подключение",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: { event: "connecting" } } }),
  },
  waiting: {
    title: "Ждёт хост",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: { event: "waiting", reason: "хост занят другим подключением" } } }),
  },
  reconnecting: {
    title: "Переподключение",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: { event: "reconnecting", reason: "timeout" } } }),
  },
  unreachable: {
    title: "Хост не отвечает",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: UNREACHABLE } }),
  },
  monitor: {
    title: "У хоста нет мониторов: выбор",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: MONITOR_WANTED } }),
  },
  nomonitors: {
    title: "У хоста нет мониторов и драйвера",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: { event: "no_monitors" } } }),
  },
  streaming: {
    title: "Идёт сессия",
    make: () => ({ cards: CONCEPT(), video: { card: 1, since: 0, status: STREAMING } }),
  },
  failed: {
    title: "Окно видео упало",
    make: () => ({ cards: CONCEPT(), last_video: { card: 1, error: "exit code 1: NVDEC is not available on this GPU" } }),
  },
  outdated: { title: "Версия устарела: жёлтый футер", make: () => ({ cards: CONCEPT(), update: { ...NEWER } }) },
  updating: { title: "Обновление скачивается", make: () => ({ cards: CONCEPT(), update: { ...NEWER, phase: "downloading" } }) },
  updateFailed: { title: "Обновление не удалось", make: () => ({ cards: CONCEPT(), update: { ...NEWER, error: "err_update_sha" } }) },
  forced: {
    title: "Качество задаёт сервер",
    make: () => ({ cards: CONCEPT(), forced: { fps: 30, mbps: 40 } }),
  },
  forcedFps: {
    title: "Сервер задаёт только частоту",
    make: () => ({ cards: CONCEPT(), forced: { fps: 30, mbps: null } }),
  },
  forcedCap: {
    title: "Сервер задаёт только потолок",
    make: () => ({ cards: CONCEPT(), forced: { fps: null, mbps: 40 } }),
  },
  long: {
    title: "Длинные имена, много машин",
    make: () => ({
      server: "a-very-long-server-name.studio.example.com:7443",
      cards: [
        card(1, "RENDER-WORKSTATION-THIRD-FLOOR-COLOR-GRADING-SUITE-02", { account: "very.long.account.name" }),
        card(2, "WPC-10"),
        ...Array.from({ length: 10 }, (_, i) => card(i + 3, `Станция-${i + 1}`, { presence: i % 3 ? "online" : "offline" })),
      ],
    }),
  },
};

const sceneName = new URLSearchParams(location.search).get("scene") ?? "concept";
const scene = (SCENES[sceneName] ?? SCENES.concept).make();

const state: AppState = {
  settings: { name: scene.name ?? "anna-laptop", server: scene.server ?? SERVER, fps: 60, mbps: 0, borderless: true, shortcuts_to_host: true, keep_unlocked: false, lang: lang(), auto_update: true },
  fps_choices: [30, 60],
  mbps_presets: [10, 20, 30, 40, 60, 80, 100],
  cards: scene.cards ?? [],
  forced: scene.forced ?? { fps: null, mbps: null },
  update: scene.update ?? { server_version: VERSION, newer: false, phase: "", error: "" },
  video: scene.video ?? null,
  last_video: scene.last_video ?? null,
  log_path: "C:\\Users\\anna\\AppData\\Roaming\\Lumen\\logs\\client-2026-11-02-10-14-02.log",
  version: VERSION,
};

const later = (ms: number) => new Promise((r) => setTimeout(r, ms));
let nextId = 100;

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const a = (args ?? {}) as any;
  const find = () => {
    const i = state.cards.findIndex((c) => c.id === a.id);
    return i;
  };
  switch (cmd) {
    // The overlay page: what it asks of the core, for the checks.
    case "overlay_ui":
      (window as any).__overlayUi = a;
      return undefined as T;
    case "cancel_transfer":
    case "choose_monitor":
      // The host makes the monitor and the stream starts.
      setTimeout(() => state.video && (state.video.status = STREAMING), 800);
      ((window as any).__overlayCommands ??= []).push({ cmd, ...a });
      return undefined as T;
    case "show_monitor":
      ((window as any).__overlayCommands ??= []).push({ cmd, ...a });
      ((window as any).__overlayMockCommand as ((c: string, a: unknown) => void) | undefined)?.(cmd, a);
      return undefined as T;
    case "get_state":
      return structuredClone(state) as T;
    case "add_card":
      await later(500);
      if (!state.settings.name) throw "err_name_required";
      if (a.token.trim().toLowerCase() === "bad") throw "err_token";
      state.cards.push(card(nextId++, "Studio-05", { token: a.token.trim(), access: "pending" }));
      return undefined as T;
    case "set_identity": {
      await later(400);
      const name = String(a.name).trim();
      const raw = String(a.server).trim();
      if (!name) throw "err_name_required";
      if (!raw || /\s/.test(raw)) throw "err_server_address";
      const server = /:\d+$/.test(raw) ? raw : `${raw}:7443`;
      if (state.cards.length && name !== state.settings.name) throw "err_name_locked";
      if (state.cards.length && server !== state.settings.server) throw "err_server_has_machines";
      if (server !== state.settings.server) {
        if (raw.startsWith("bad")) throw "err_probe_timeout";
        if (raw.startsWith("192.168.") && !trusted.has(server)) throw `err_cert_untrusted|${LAN_KEY}`;
      }
      state.settings = { ...state.settings, name, server };
      return undefined as T;
    }
    case "trust_server":
      await later(200);
      trusted.add(/:\d+$/.test(a.server) ? a.server : `${a.server}:7443`);
      return undefined as T;
    case "request_access": {
      const i = find();
      state.cards[i] = card(state.cards[i].id, state.cards[i].machine, { ...state.cards[i], access: "pending", server_unknown: false });
      return undefined as T;
    }
    case "reboot_card": {
      await later(300);
      const c = state.cards[find()];
      if (c.presence === "offline" || c.presence === "updating") throw "err_reboot_offline";
      if (c.access !== "approved") throw "err_reboot_not_approved";
      // Restarting, offline a while, then back.
      const set = (presence: Presence) => {
        const i = state.cards.findIndex((x) => x.id === c.id);
        if (i >= 0) state.cards[i] = card(c.id, c.machine, { ...state.cards[i], presence });
      };
      set("restarting");
      setTimeout(() => set("offline"), 4000);
      setTimeout(() => set("online"), 12000);
      return undefined as T;
    }
    case "remove_card":
      state.cards.splice(find(), 1);
      return "" as T;
    case "save_settings":
      await later(100);
      state.settings = { ...state.settings, fps: a.fps, mbps: a.mbps, borderless: a.borderless, shortcuts_to_host: a.shortcutsToHost, keep_unlocked: a.keepUnlocked, auto_update: a.autoUpdate };
      return undefined as T;
    case "update_now":
      state.update = { ...state.update, phase: "downloading", error: "" };
      await later(1500);
      state.update = { ...state.update, phase: "installing" };
      return undefined as T;
    case "connect":
      state.video = { card: a.id, since: 0, status: { event: "connecting" } };
      setTimeout(() => state.video && (state.video.status = STREAMING), 1500);
      return undefined as T;
    case "disconnect":
      state.video = null;
      return undefined as T;
    case "set_lang":
      if (a.lang !== "en" && a.lang !== "ru") throw "err_lang";
      state.settings.lang = a.lang;
      return undefined as T;
    case "open_log_folder":
      return undefined as T;
    default:
      throw `mock: ${cmd}`;
  }
}
