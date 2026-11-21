// The hub's one door to the app: every screen talks to the core through this
// file only. The core (apps/client-app) gives states and codes; words are the
// UI's. A new feature is a new call here. contract/*.example.ts is what the core
// really sends: `tsc` checks it against these types.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Lang } from "./i18n";

export type Access = "" | "none" | "pending" | "approved" | "rejected";
export type Presence = "" | "offline" | "online" | "busy" | "inactive" | "updating" | "restarting";

/** Another version on the host (`Card::host_update`). */
export type HostUpdate = "" | "on_connect" | "off" | "newer";

/** What the machine's one button (besides «Remove») does now. `update`: an older host is asked to update to this
 * version, then the session; `update_off`: an older host whose switch is off; `update_client`: a newer host — this
 * client updates first (its footer). The last two: nothing to do. */
export type ButtonOp = "connect" | "update" | "update_off" | "update_client" | "request" | "pending" | "rejected";

/** A machine: a host's token on this client's one server. */
export type Card = {
  id: number;
  token: string;
  machine: string;
  account: string;
  /** null: not asked yet; false: no host has this token. */
  found: boolean | null;
  access: Access;
  presence: Presence;
  /** The Lumen version the host runs ("": unknown). */
  host_version: string;
  /** The host's last update failed: its code ("": no failure). */
  update_failed: string;
  /** Another version on the host: "" — the same (or unknown); `on_connect` — older, it updates
   * itself when this client connects; `off` — older, its switch is off; `newer` — newer than this client. */
  host_update: HostUpdate;
  server_error: string;
  /** The server does not know this client (its database was reset). */
  server_unknown: boolean;
  button: { op: ButtonOp; enabled: boolean };
};

/** What the server sets for this client, each on its own; null: the client chooses. */
type Forced = { fps: number | null; mbps: number | null };

type Route = { host: string; via?: string; adapter?: string; default?: boolean };

/** A monitor for a host without one: resolution and scale, %. */
export type MonitorChoice = { width: number; height: number; scale: number };
export type Resolution = { width: number; height: number; aspect: string };

/** The video window's session, as it last said (apps/client/src/status.rs). */
export type VideoStatus =
  | { event: "connecting" }
  | { event: "waiting"; reason: string }
  | { event: "reconnecting"; reason: string }
  | { event: "unreachable"; routes: Route[]; host_nets: string[]; client_nets: string[] }
  | { event: "streaming"; host: string; host_net?: string; via?: string; adapter?: string; client_net?: string }
  /** The host has no monitor: it asks for ours (`ask` — the question's number), `suggest` — like this computer's. */
  | { event: "monitor_wanted"; ask: number; suggest: MonitorChoice; resolutions: Resolution[]; scales: number[] }
  /** The host has no monitor, and none can be made (no Lumen VDD there). */
  | { event: "no_monitors" };

export type Settings = {
  name: string;
  /** The one server, `host:port` ("" until the welcome window). Changed with the name by `setIdentity` only. */
  server: string;
  fps: number;
  mbps: number;
  borderless: boolean;
  shortcuts_to_host: boolean;
  /** The host does not lock Windows after the session. */
  keep_unlocked: boolean;
  /** The interface's language (i18n.ts); changed by `setLang` only. */
  lang: Lang;
  /** A newer version on the server is installed at start. */
  auto_update: boolean;
};

/** The server's version and this program's update (apps/client-app/src/update.rs). */
export type Update = {
  /** "": not known yet, or no installer on the server. */
  server_version: string;
  /** Higher than this one: the footer is yellow. */
  newer: boolean;
  /** "": nothing under way. */
  phase: "" | "downloading" | "installing";
  /** The last try failed: its code. */
  error: string;
};

export type AppState = {
  /** `name` and `server` are empty until the welcome window; nothing else before them. */
  settings: Settings;
  fps_choices: number[];
  mbps_presets: number[];
  cards: Card[];
  forced: Forced;
  update: Update;
  video: { card: number; since: number; status: VideoStatus } | null;
  last_video: { card: number; error: string } | null;
  log_path: string;
  /** This Lumen's version: the host must run the same. */
  version: string;
};

/** Asks the core for something; false: it failed, and the screen shows why. */
export type Run = (f: () => Promise<unknown>) => Promise<boolean>;

export const api = {
  state: () => invoke<AppState>("get_state"),
  /** Keeps the machine only if the server knows the token; asks the admin for access. */
  addMachine: (token: string) => invoke<void>("add_card", { token }),
  /** The name and the server (only with no machines); another server is kept only if it answers. */
  setIdentity: (name: string, server: string) => invoke<void>("set_identity", { name, server }),
  /** The user trusts this key of a server with its own certificate (after `err_cert_untrusted` / `err_cert_changed`). */
  trustServer: (server: string, fingerprint: string) => invoke<void>("trust_server", { server, fingerprint }),
  requestAccess: (id: number) => invoke<void>("request_access", { id }),
  /** Returns what the user should know afterwards ("": nothing). */
  removeMachine: (id: number) => invoke<string>("remove_card", { id }),
  /** Restarts the machine (the card shows «restarting» until it is back). */
  rebootMachine: (id: number) => invoke<void>("reboot_card", { id }),
  saveSettings: (s: Settings) =>
    invoke<void>("save_settings", {
      fps: s.fps,
      mbps: s.mbps,
      borderless: s.borderless,
      shortcutsToHost: s.shortcuts_to_host,
      keepUnlocked: s.keep_unlocked,
      autoUpdate: s.auto_update,
    }),
  /** Downloads the newer version and installs it: this program closes and starts again. */
  updateNow: () => invoke<void>("update_now"),
  connect: (id: number, title: string) => invoke<void>("connect", { id, title }),
  disconnect: () => invoke<void>("disconnect"),
  /** The answer to a host without monitors: it makes one like this and shows it. */
  chooseMonitor: (m: MonitorChoice) => invoke<void>("choose_monitor", { width: m.width, height: m.height, scale: m.scale }),
  setLang: (lang: Lang) => invoke<void>("set_lang", { lang }),
  openLogFolder: () => invoke<void>("open_log_folder"),
  window: {
    minimize: () => getCurrentWindow().minimize(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
    close: () => getCurrentWindow().close(),
    isMaximized: () => getCurrentWindow().isMaximized(),
  },
};
