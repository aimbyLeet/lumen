// The window's one door to the app (apps/host-app): every screen talks to the core through this file only.
import type { UpdateInfo } from "../../client/src/Update";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Lang } from "../../client/src/i18n";

export type Side = "left" | "right" | "above" | "below";
export type Align = "start" | "center" | "end";
// vdd::layout
export type Virtual = { width: number; height: number; scale: number; attach: { to: number; side: Side; align: Align } };
export type MonitorSettings = { enabled: boolean; virtuals: Virtual[] };
export type Rect = { x: number; y: number; width: number; height: number };
export type Resolution = { width: number; height: number; aspect: string };
/** A client's own monitors on a host without a physical one: monitor 1 virtual too. */
export type ClientSet = { primary: { width: number; height: number; scale: number }; virtuals: Virtual[] };

export type MonitorsInfo = {
  state: {
    installed: boolean;
    driver: string;
    settings: MonitorSettings;
    up: { monitor: number; width: number; height: number }[];
    error: string;
    /** Windows has a physical monitor (the machine's virtual ones are up only then). */
    physical: boolean;
    /**
     * No physical monitor, a client in session: its own set (`set: null` — it is being asked). `parked`: nobody in
     * session, the last client's set kept for its next session (on in Windows only with no other display).
     */
    client: { key: string; set: ClientSet | null; parked: boolean } | null;
  };
  /** The real monitors of this desktop, as numbered: the primary one first. */
  real: (Rect & { name: string })[];
  /** How many of the real ones the client gets (those on NVIDIA's GPU). */
  shown: number;
  /** Virtual monitors that can be shown here (3 monitors at most). */
  allowed: number;
  /** A monitor of Lumen VDD is on in Windows. */
  lumen_on: boolean;
  catalog: { resolutions: Resolution[]; scales: number[] };
};

/**
 * What the service keeps. The window does not show `manual_addrs` and `server_ca`, but sends back what it
 * got: a machine set up with them goes on working. `server_fingerprint`: the key the user trusted, of a server
 * with its own certificate (a local network), or "".
 */
export type Settings = { server: string; machine: string; manual_addrs: string; server_ca: string; server_fingerprint: string };

// desktop::hostsvc::HostStatus
export type Status = {
  state: "stopped" | "starting" | "online" | "offline" | "refused" | "session" | "error";
  detail: string;
  client: string;
  fps: number;
  mbps: number;
  /** Unix seconds. */
  since: number;
  /** `ip:port` the host announces. */
  addrs: string[];
  /** The server's version ("": not known yet); the version being installed ("": none); why the last update failed. */
  server_version: string;
  updating: string;
  update_error: string;
};

export type AppState = {
  settings: Settings;
  defaults: { machine: string };
  token: string | null;
  registered: boolean;
  status: Status;
  service_error: string | null;
  /** The service is installed but stopped: nobody can connect. */
  host_off: boolean;
  autostart: boolean;
  /** The service keeps the computer from sleeping. */
  keep_awake: boolean;
  /** The host updates itself when a newer client connects. */
  auto_update: boolean;
  /** The server's version and the host's update. */
  update: UpdateInfo;
  /** This Lumen's version: clients must run the same. */
  version: string;
  monitors: MonitorsInfo;
  /** The window's language (this Windows user's choice); changed by `setLang` only. */
  lang: Lang;
};

export const api = {
  state: () => invoke<AppState>("get_state"),
  save: (settings: Settings) => invoke<void>("save", { settings }),
  newToken: () => invoke<string>("new_token"),
  /** Fails with `err_cert_untrusted|<fingerprint>` for a server with its own certificate not trusted yet. */
  probe: (server: string, serverCa: string, serverFingerprint: string) => invoke<void>("probe", { server, serverCa, serverFingerprint }),
  /** Returns what the user should know afterwards. */
  unregister: () => invoke<string>("unregister"),
  reregister: () => invoke<void>("reregister"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  setKeepAwake: (enabled: boolean) => invoke<void>("set_keep_awake", { enabled }),
  setAutoUpdate: (enabled: boolean) => invoke<void>("set_auto_update", { enabled }),
  /** The host restarts and updates to the server's newer version; the installer closes this window, it opens again after. */
  updateNow: () => invoke<void>("update_now"),
  startHost: () => invoke<void>("start_host"),
  setMonitors: (settings: MonitorSettings) => invoke<void>("set_monitors", { settings }),
  resetMonitors: () => invoke<void>("reset_monitors"),
  /** A host without a physical monitor: the set of the client in session (its key). */
  setClientMonitors: (client: string, set: ClientSet) => invoke<void>("set_client_monitors", { client, set }),
  setLang: (lang: Lang) => invoke<void>("set_lang", { lang }),
  openLogFolder: () => invoke<void>("open_log_folder"),
  /** The tray's menu: the words are here, what the items do is the core's. */
  setTray: (items: { open: string; quit: string }) => invoke<void>("set_tray", items),
  window: {
    minimize: () => getCurrentWindow().minimize(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
    close: () => getCurrentWindow().close(),
    isMaximized: () => getCurrentWindow().isMaximized(),
  },
};
