// What is drawn over the video talks to the core only here.
// The state is `OverlayState` of apps/client/src/ui_api.rs; contract/overlay.example.ts is checked against these types.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** [net, decode, show, lost, host]: the slowest frame in a slice of the graphs, ms; host: the encode then, part of net. */
export type Column = [number, number, number, boolean, number];

export type Stats = {
  frames_recent: number;
  latency_ms: number;
  net_ms: number;
  net_p95_ms: number;
  decode_ms: number;
  decode_p95_ms: number;
  show_ms: number;
  encode_ms: number | null;
  host_fps: number | null;
  fps: number;
  shown_fps: number;
  rtt_ms: number | null;
  loss_percent: number;
  frames_lost_recent: number;
  video_mbps: number;
  /** 0: not known. */
  cap_mbps: number;
  audio: { playing: boolean; lag_ms: number | null; gaps_recent: number; gaps_span: number } | null;
  format: {
    width: number;
    height: number;
    codec: string;
    bit_depth: number;
    chroma: string;
    primaries: number;
    transfer: number;
    matrix: number;
    full_range: boolean;
  } | null;
  budget_ms: number;
  /** Oldest first, over 10 s. */
  columns: (Column | null)[];
};

export type Stage = "waiting" | "running" | "checking" | "saved" | "cancelled";
export type CancelReason = "user" | "disk" | "read" | "write" | "hash" | "not_allowed" | "no_user" | "locked";

export type Transfer = {
  outgoing: boolean;
  /** Into Downloads\Lumen (not a clipboard set's). */
  download: boolean;
  /** The clipboard set it belongs to (numbered per direction). */
  set: number | null;
  /** A download's batch: the files of one paste, numbered by the sender (a folder of them is one row). */
  batch: number | null;
  /** A format of a copied picture, not a file. */
  image: boolean;
  transfer: number;
  name: string;
  len: number;
  done: number;
  stage: Stage;
  reason: CancelReason | null;
  /** Bytes per second lately. */
  speed: number;
  started_ago_ms: number;
  ended_ago_ms: number | null;
};

export type OverlayState = {
  /** Screen pixels per picture pixel. */
  picture_scale: number;
  view: {
    /** "": before the first picture. */
    mode: "all" | "one" | "windows" | "";
    active: number;
    count: number;
    /** Monitor 1's place in the window, as shares: x, y, width, height. */
    first: [number, number, number, number];
  };
  stats_shown: boolean;
  stats: Stats | null;
  transfers: Transfer[];
  /** Ctrl+Alt+Shift+D, counted. */
  transfers_key: number;
  notice: { code: "two_windows_on_one_monitor"; serial: number } | null;
};

export const overlayApi = {
  /** The session's state, as it changes. */
  onState: (f: (s: OverlayState) => void) => listen<OverlayState>("overlay", (e) => f(e.payload)),
  /** Whether there is anything to show, and where clicks are taken: [x, y, width, height] in window pixels. */
  setUi: (visible: boolean, interactive: number[][]) => invoke<void>("overlay_ui", { visible, interactive }),
  cancelTransfer: (outgoing: boolean, transfer: number) => invoke<void>("cancel_transfer", { outgoing, transfer }),
  showMonitor: (index: number) => invoke<void>("show_monitor", { index }),
};
