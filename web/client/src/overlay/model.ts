// What the core's numbers mean: words, advice, when things show. No React here: `npm test` checks it.
import { words } from "../i18n.ts";
import { U } from "../units.ts";
import type { CancelReason, Stats, Transfer } from "./api";

const W = words(
  {
    noPicture: "No picture from the host",
    framesLost: "The network loses frames — the picture stutters",
    decodeSlow: "This computer cannot keep up showing the video",
    encodeSlow: "The remote computer cannot keep up compressing the video",
    packetsLost: "The network loses packets, so far without consequences",
    hostFps: "The remote computer sends fewer frames",
    rttLong: "Slow network response — the mouse and keyboard lag",
    jitter: "The network latency jumps",
    audioGaps: "The sound breaks up",
    allGood: "All good",
    latency: "Latency",
    smoothness: "Smoothness",
    rtt: "Network response",
    loss: "Loss",
    stream: "Stream",
    audio: "Sound",
    none: "none",
    ms: "ms",
    of: "of",
    fps: "frames/s",
    fullRange: "full range",
    limitedRange: "limited range",
    capture: "capture RGB 8 bit",
    bit: "bit",
    toHost: "Client → Host",
    toClient: "Host → Client",
    reasons: {
      user: "cancelled",
      disk: "no space on the disk",
      read: "could not read: no access or the path is unavailable",
      write: "could not write",
      hash: "the file arrived damaged",
      not_allowed: "file transfer is turned off by the administrator",
      no_user: "nobody is signed in to Windows on the host",
      locked: "the host is locked — sign in to Windows on the host",
    } as Record<CancelReason, string>,
    left: "left",
    waitingOther: "waiting for the other side",
    checking: "checking",
    done: "done",
    took: "in",
    picture: "Picture",
    files: (n: number) => (n === 1 ? `${n} file` : `${n} files`),
    notSent: (k: number, n: number) => `${k} of ${n} not transferred`,
    twoWindows: "Two windows on one monitor: move the windows to their own monitors",
  },
  {
    noPicture: "Нет изображения от хоста",
    framesLost: "Сеть теряет кадры — изображение подёргивается",
    decodeSlow: "Этот компьютер не успевает показывать видео",
    encodeSlow: "Удалённый компьютер не успевает сжимать видео",
    packetsLost: "Сеть теряет пакеты, пока без последствий",
    hostFps: "Удалённый компьютер отдаёт меньше кадров",
    rttLong: "Долгий отклик сети — мышь и клавиатура запаздывают",
    jitter: "Задержка в сети скачет",
    audioGaps: "Звук прерывается",
    allGood: "Всё хорошо",
    latency: "Задержка",
    smoothness: "Плавность",
    rtt: "Отклик сети",
    loss: "Потери",
    stream: "Поток",
    audio: "Звук",
    none: "нет",
    ms: "мс",
    of: "из",
    fps: "кадров/с",
    fullRange: "полный диапазон",
    limitedRange: "ограниченный диапазон",
    capture: "захват RGB 8 бит",
    bit: "бит",
    toHost: "Клиент → Хост",
    toClient: "Хост → Клиент",
    reasons: {
      user: "отменено",
      disk: "нет места на диске",
      read: "не удалось прочитать: нет доступа или путь недоступен",
      write: "не удалось записать",
      hash: "файл пришёл с ошибкой",
      not_allowed: "передача файлов отключена администратором",
      no_user: "на хосте никто не вошёл в Windows",
      locked: "хост заблокирован — войдите в Windows на хосте",
    },
    left: "осталось",
    waitingOther: "ожидание другой стороны",
    checking: "проверка",
    done: "готово",
    took: "за",
    picture: "Картинка",
    files: (n: number) => {
      const tens = n % 100;
      const ones = n % 10;
      if (tens >= 11 && tens <= 14) return `${n} файлов`;
      if (ones === 1) return `${n} файл`;
      return ones >= 2 && ones <= 4 ? `${n} файла` : `${n} файлов`;
    },
    notSent: (k: number, n: number) => `не передано ${k} из ${n}`,
    twoWindows: "Два окна на одном мониторе: разнесите окна по своим мониторам",
  },
);

export type Level = "dim" | "normal" | "warn" | "bad";

export type Status = { level: Level; title: string };
type Metric = { label: string; value: string; level: Level };
type Breakdown = { hostMs: number; netMs: number; localMs: number };

/** Latency, ms, from which it shows as worth a look, and as a problem. */
const LATENCY_WARN = 60;
const LATENCY_BAD = 120;
/** The same for the ping. */
const RTT_WARN = 60;
const RTT_BAD = 150;

const level = (v: number, warn: number, bad: number): Level => (v >= bad ? "bad" : v >= warn ? "warn" : "normal");
const fixed = (v: number, digits = 0) => v.toFixed(digits);

/** How the session is, in words. */
export function status(s: Stats): Status {
  const w = W();
  const say = (l: Level, title: string): Status => ({ level: l, title });
  if (s.frames_recent === 0) return say("warn", w.noPicture);
  if (s.frames_lost_recent > 0) return say("bad", w.framesLost);
  if (s.decode_p95_ms > s.budget_ms * 0.9) return say("bad", w.decodeSlow);
  if (s.encode_ms !== null && s.encode_ms > s.budget_ms * 0.8) return say("bad", w.encodeSlow);
  if (s.loss_percent >= 1) return say("warn", w.packetsLost);
  if (s.host_fps !== null && s.host_fps < s.fps * 0.85) return say("warn", w.hostFps);
  if (s.rtt_ms !== null && s.rtt_ms >= RTT_WARN) return say("warn", w.rttLong);
  if (s.net_p95_ms - s.net_ms > 30) return say("warn", w.jitter);
  if (s.audio && s.audio.gaps_recent > 0) return say("warn", w.audioGaps);
  return say("normal", w.allGood);
}

/** The plain numbers. */
export function metrics(s: Stats): Metric[] {
  const w = W();
  const list: Metric[] = [
    { label: w.latency, value: s.frames_recent === 0 ? "—" : `${fixed(s.latency_ms)} ${w.ms}`, level: level(s.latency_ms, LATENCY_WARN, LATENCY_BAD) },
    { label: w.smoothness, value: `${fixed(s.shown_fps)} ${w.of} ${fixed(s.fps)} ${w.fps}`, level: level(s.fps - s.shown_fps, s.fps * 0.1, s.fps * 0.3) },
    { label: w.rtt, value: s.rtt_ms === null ? "—" : `${fixed(s.rtt_ms)} ${w.ms}`, level: level(s.rtt_ms ?? 0, RTT_WARN, RTT_BAD) },
    { label: w.loss, value: `${fixed(s.loss_percent, 1)} %`, level: s.frames_lost_recent > 0 ? "bad" : level(s.loss_percent, 1, 5) },
    { label: w.stream, value: s.cap_mbps > 0 ? `${fixed(s.video_mbps)} ${w.of} ${fixed(s.cap_mbps)} ${U().mbps}` : `${fixed(s.video_mbps)} ${U().mbps}`, level: "normal" },
  ];
  if (s.audio) {
    list.push({
      label: w.audio,
      // Nothing comes: the host has no output device, or the stream has just started.
      value: !s.audio.playing ? w.none : s.audio.lag_ms === null ? "—" : `${fixed(s.audio.lag_ms)} ${w.ms}`,
      level: s.audio.gaps_span > 0 ? "warn" : "normal",
    });
  }
  return list;
}

/** Where the latency goes: medians of the parts, scaled to add up to the shown latency. */
export function breakdown(s: Stats): Breakdown | null {
  if (s.frames_recent === 0) return null;
  const encode = s.encode_ms ?? 0;
  const host = Math.min(encode, s.net_ms);
  const parts = [host, Math.max(s.net_ms - host, 0), s.decode_ms + s.show_ms];
  const sum = parts[0] + parts[1] + parts[2];
  const k = sum > 0 ? s.latency_ms / sum : 0;
  return { hostMs: parts[0] * k, netMs: parts[1] * k, localMs: parts[2] * k };
}

/** The picture from the host's screen to this one. The host captures only BGRA8 (Desktop Duplication). */
export function videoLine(f: NonNullable<Stats["format"]>): string {
  const BT709 = 1;
  const colour = f.primaries === BT709 && f.transfer === BT709 && f.matrix === BT709 ? "BT.709" : `H.273 ${f.primaries}/${f.transfer}/${f.matrix}`;
  const w = W();
  const range = f.full_range ? w.fullRange : w.limitedRange;
  return `${f.width}×${f.height} · ${w.capture} → ${f.codec} ${f.bit_depth} ${w.bit} ${f.chroma} · ${colour}, ${range}`;
}

/** One line per graph: where the latency goes, over time; null: no frame in the slice. */
type Lines = { host: (number | null)[]; net: (number | null)[]; local: (number | null)[]; lost: number[] };

export function lines(s: Stats): Lines {
  const pick = (f: (c: NonNullable<Stats["columns"][number]>) => number) => s.columns.map((c) => (c && (c[0] > 0 || !c[3]) ? f(c) : null));
  return {
    host: pick((c) => Math.min(c[4], Math.max(c[0], 0))),
    net: pick((c) => Math.max(c[0] - c[4], 0)),
    // Clock offset not known yet: a part may come out below zero.
    local: pick((c) => Math.max(c[1] + c[2], 0)),
    lost: s.columns.flatMap((c, i) => (c && c[3] ? [i] : [])),
  };
}

/** Across a graph, 0..100, the `i`th of `count` slices. */
export const chartX = (i: number, count: number) => (i / Math.max(count - 1, 1)) * 100;

/** The pieces of a graph's line, as `points` of SVG in a 100×100 box: no frame in a slice breaks the line. */
export function chartPieces(line: (number | null)[], top: number): string[] {
  const pieces: string[] = [];
  let piece = "";
  line.forEach((v, i) => {
    if (v === null) {
      if (piece) pieces.push(piece);
      piece = "";
    } else {
      piece += `${chartX(i, line.length)},${100 - Math.min(v / top, 1) * 100} `;
    }
  });
  if (piece) pieces.push(piece);
  return pieces;
}

/** The graphs' common top, ms, so that they compare: scaled to the usual values, a lone spike reaches the top. */
export function graphTop(s: Stats): number {
  const l = lines(s);
  const all = [...l.host, ...l.net, ...l.local].flatMap((v) => (v === null ? [] : [v])).sort((a, b) => a - b);
  const usual = all.length ? all[Math.floor(((all.length - 1) * 95) / 100)] : 0;
  return Math.min(Math.max(s.budget_ms, usual * 1.5), 400);
}

// ---- Transfers

/** How long the block stays after the last transfer ended. */
export const LINGER_MS = 5000;

/** Times are milliseconds on one clock. */
export type Row = Transfer & { startedAt: number; endedAt: number | null };

export const withTimes = (list: Transfer[], now: number): Row[] =>
  list.map((t) => ({ ...t, startedAt: now - t.started_ago_ms, endedAt: t.ended_ago_ms === null ? null : now - t.ended_ago_ms }));

/** What Ctrl+Alt+Shift+D did last: nothing; shown (the session's history, until the key again); hidden at this time. */
type Manual = { kind: "auto" } | { kind: "pinned" } | { kind: "dismissed"; at: number };

export type Visibility = {
  manual: Manual;
  /** Rows seen so far: a new one (even ended at once) brings the block up. */
  seen: number;
  /** When it last came up by itself: only what ends after it keeps it up. */
  since: number | null;
};

export const fresh = (): Visibility => ({ manual: { kind: "auto" }, seen: 0, since: null });

export const going = (t: Transfer) => t.stage === "waiting" || t.stage === "running" || t.stage === "checking";

/** Ctrl+Alt+Shift+D, `shown`: whether the block is on the screen now. */
export function pressKey(v: Visibility, shown: boolean, now: number) {
  v.manual = shown ? { kind: "dismissed", at: now } : { kind: "pinned" };
}

/**
 * The download rows to show now (oldest first), or null: hidden. Changes `v`. Shown, the block lists every download of
 * the session; what does not fit scrolls (the owner's choice 2026-11-12). The clipboard's files never bring the
 * block up: they have the one line of `clipboardLine`, for when the user opens it (the owner's choice 2026-11-02).
 */
export function visibleRows(v: Visibility, rows: Row[], now: number): Row[] | null {
  const shownOnes = rows.filter((r) => r.download);
  if (v.manual.kind === "pinned") return shownOnes;
  if (v.manual.kind === "dismissed") {
    const at = v.manual.at;
    // A row new since the key (a transfer, a refused paste) brings it back.
    if (!shownOnes.some((r) => r.startedAt > at)) return null;
    v.manual = { kind: "auto" };
    v.since = now;
  }
  if (shownOnes.length > v.seen && v.since === null) v.since = now;
  v.seen = shownOnes.length;
  const anyGoing = shownOnes.some(going);
  const ends = shownOnes.flatMap((r) => (r.endedAt === null ? [] : [r.endedAt]));
  const lastEnd = ends.length ? Math.max(...ends) : null;
  const lingering = v.since !== null && lastEnd !== null && now - lastEnd < LINGER_MS;
  if (!anyGoing && !lingering) {
    v.since = null;
    return null;
  }
  if (v.since === null) v.since = now;
  return shownOnes;
}

export const direction = (t: Transfer) => (t.outgoing ? W().toHost : W().toClient);

export function size(bytes: number): string {
  const u = U();
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} ${u.gb}`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} ${u.mb}`;
  if (bytes >= 1e3) return `${(bytes / 1e3).toFixed(0)} ${u.kb}`;
  return `${bytes} ${u.b}`;
}

export function duration(secs: number): string {
  const u = U();
  const s = Math.round(secs);
  if (s < 60) return `${s} ${u.s}`;
  if (s < 3600) return `${Math.floor(s / 60)} ${u.min} ${s % 60} ${u.s}`;
  return `${Math.floor(s / 3600)} ${u.h} ${Math.floor((s % 3600) / 60)} ${u.min}`;
}

const reason = (r: CancelReason | null) => W().reasons[r ?? "user"];

export type Tone = "normal" | "done" | "bad";

/** How far it has come, how fast, and how long is left. */
function progressText(done: number, len: number, speed: number): string {
  const w = W();
  const so_far = `${size(done)} ${w.of} ${size(len)}`;
  return speed > 1 ? `${so_far} · ${((speed * 8) / 1e6).toFixed(1)} ${U().mbps} · ${w.left} ${duration(Math.max(len - done, 0) / speed)}` : so_far;
}

/** The second line of a row: progress, speed and time left, or how it ended. */
export function state(t: Transfer): { text: string; tone: Tone } {
  const w = W();
  switch (t.stage) {
    case "waiting":
      return { text: w.waitingOther, tone: "normal" };
    case "checking":
      return { text: w.checking, tone: "normal" };
    case "running":
      return { text: progressText(t.done, t.len, t.speed), tone: "normal" };
    case "saved": {
      const took = t.ended_ago_ms === null ? 0 : (t.started_ago_ms - t.ended_ago_ms) / 1000;
      return { text: `${w.done} · ${size(t.len)} ${w.took} ${duration(took)}`, tone: "done" };
    }
    case "cancelled":
      return { text: reason(t.reason), tone: "bad" };
  }
}

/** The share done, 0..1. */
export function progress(t: Transfer): number {
  if (t.stage === "saved" || t.stage === "checking") return 1;
  return t.len === 0 ? 0 : Math.min(Math.max(t.done / t.len, 0), 1);
}

/** The one line of the clipboard: what was copied last, as a whole. One, as the clipboard is overwritten. */
type ClipboardLine = { direction: string; name: string; text: string; tone: Tone; share: number; going: Transfer[] };

export function clipboardLine(list: Transfer[]): ClipboardLine | null {
  const w = W();
  const clips = list.filter((t) => t.set !== null);
  if (clips.length === 0) return null;
  // Oldest first: the last one started belongs to the newest set.
  const newest = clips.reduce((a, b) => (b.started_ago_ms <= a.started_ago_ms ? b : a));
  const set = clips.filter((t) => t.set === newest.set && t.outgoing === newest.outgoing);
  const name = set.some((t) => t.image) ? w.picture : set.length === 1 ? set[0].name : w.files(set.length);
  const len = set.reduce((sum, t) => sum + t.len, 0);
  const done = set.reduce((sum, t) => sum + progress(t) * t.len, 0);
  const on = set.filter(going);
  const failed = set.find((t) => t.stage === "cancelled");
  let text: string;
  let tone: Tone = "normal";
  if (on.length > 0) {
    const speed = on.reduce((sum, t) => sum + t.speed, 0);
    text = on.every((t) => t.stage === "waiting") ? w.waitingOther : progressText(done, len, speed);
  } else if (failed) {
    text = reason(failed.reason);
    tone = "bad";
  } else {
    text = `${w.done} · ${size(len)}`;
    tone = "done";
  }
  return { direction: direction(newest), name, text, tone, share: len === 0 ? (on.length ? 0 : 1) : done / len, going: on };
}

/** A line of the downloads: a file, or a folder copied as a whole (its files, of one paste, as one). */
type DownloadLine<T extends Transfer = Transfer> = { key: string; name: string; direction: string; text: string; tone: Tone; share: number; files: T[]; going: T[] };

/** The folder a transferred name is in, if any: the first part of `folder/…/file`. */
const folderOf = (name: string): string | null => {
  const at = name.indexOf("/");
  return at > 0 ? name.slice(0, at) : null;
};

/** One line per file, a folder's files (one batch, one direction) in one, where its first file was; oldest first. */
export function downloadLines<T extends Transfer>(list: T[]): DownloadLine<T>[] {
  const lines: DownloadLine<T>[] = [];
  const folders = new Map<string, DownloadLine<T>>();
  for (const t of list) {
    const folder = folderOf(t.name);
    if (folder === null || t.batch === null) {
      const st = state(t);
      lines.push({ key: `${t.outgoing}-${t.transfer}`, name: t.name, direction: direction(t), text: st.text, tone: st.tone, share: progress(t), files: [t], going: going(t) ? [t] : [] });
      continue;
    }
    const key = `${t.outgoing}-b${t.batch}-${folder}`;
    const line = folders.get(key);
    if (line) line.files.push(t);
    else {
      const fresh: DownloadLine<T> = { key, name: folder, direction: direction(t), text: "", tone: "normal", share: 0, files: [t], going: [] };
      folders.set(key, fresh);
      lines.push(fresh);
    }
  }
  for (const line of folders.values()) folderState(line);
  return lines;
}

/** A folder's line, from its files: as one transfer. */
function folderState(line: DownloadLine<Transfer>) {
  const w = W();
  const set = line.files;
  line.name = `${line.name} · ${w.files(set.length)}`;
  const len = set.reduce((sum, t) => sum + t.len, 0);
  const done = set.reduce((sum, t) => sum + progress(t) * t.len, 0);
  line.going = set.filter(going);
  line.share = len === 0 ? (line.going.length ? 0 : 1) : done / len;
  const failed = set.filter((t) => t.stage === "cancelled");
  if (line.going.length > 0) {
    const speed = line.going.reduce((sum, t) => sum + t.speed, 0);
    line.text = line.going.every((t) => t.stage === "waiting") ? w.waitingOther : progressText(done, len, speed);
  } else if (failed.length === set.length) {
    line.text = reason(failed[0].reason);
    line.tone = "bad";
  } else if (failed.length > 0) {
    line.text = `${w.notSent(failed.length, set.length)} · ${reason(failed[0].reason)}`;
    line.tone = "bad";
  } else {
    const took = Math.max(...set.map((t) => t.started_ago_ms)) - Math.min(...set.map((t) => t.ended_ago_ms ?? 0));
    line.text = `${w.done} · ${size(len)} ${w.took} ${duration(Math.max(took, 0) / 1000)}`;
    line.tone = "done";
  }
}

/** What «Cancel all» cancels. */
export const cancellable = (list: Transfer[]) => list.filter(going);

/** The things that go: each download line (a folder is one), and the clipboard as one. */
export function goingCount(list: Transfer[]): number {
  const downloads = downloadLines(list.filter((t) => t.set === null)).filter((l) => l.going.length > 0).length;
  return downloads + (list.some((t) => t.set !== null && going(t)) ? 1 : 0);
}

/** It shows when more than one thing goes (the clipboard and a single line have their own «Cancel»). */
export const cancelAllShown = (list: Transfer[]): boolean => goingCount(list) > 1;

/** A monitor's number on its circle (the owner's choice 2026-11-02). */
export function roman(n: number): string {
  const digits: [number, string][] = [[10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"]];
  let out = "";
  for (const [value, digit] of digits) {
    while (n >= value) {
      out += digit;
      n -= value;
    }
  }
  return out;
}

// ---- Size

/** The width (CSS pixels) the two blocks take side by side: the stats' centred, the transfers' at the right edge. */
export const SIDE_BY_SIDE = 2 * (640 / 2 + 16 + 440 + 24);

/**
 * As large as at 1:1 times the picture's scale: a picture shown smaller shrinks them with it, a larger one
 * does not blow them up (the owner's choice 2026-10-27). On a narrow picture they shrink further: the two never overlap.
 */
export const overlayZoom = (pictureScale: number, width: number) => Math.min(Math.min(pictureScale, 1), width / SIDE_BY_SIDE);

/** A notice the core asks for, in words. */
export const noticeText = (code: "two_windows_on_one_monitor") => ({ two_windows_on_one_monitor: W().twoWindows })[code];
/** How long a notice stays. */
export const NOTICE_MS = 4000;
