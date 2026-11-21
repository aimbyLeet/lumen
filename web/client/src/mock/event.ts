// `vite --mode mock`: the session's state for the overlay page (`overlay.html?scene=...`), as the core sends it.
import type { Column, OverlayState, Stats, Transfer } from "../overlay/api";

function graph(spike = false, lost = false): (Column | null)[] {
  return Array.from({ length: 200 }, (_, i) => {
    const wave = Math.sin(i / 7) * 2;
    const host = 3 + Math.sin(i / 19);
    if (lost && i % 37 === 20) return [20, 5, 2, true, host];
    if (spike && i % 23 === 5) return [70 + wave * 8, 6, 3, false, host];
    return [13 + wave, 5, 1 + Math.abs(wave) / 2, false, host];
  });
}

const stats = (more: Partial<Stats> = {}): Stats => ({
  frames_recent: 120,
  latency_ms: 19,
  net_ms: 13,
  net_p95_ms: 15,
  decode_ms: 5,
  decode_p95_ms: 6,
  show_ms: 1,
  encode_ms: 3,
  host_fps: 60,
  fps: 60,
  shown_fps: 60,
  rtt_ms: 12,
  loss_percent: 0,
  frames_lost_recent: 0,
  video_mbps: 21,
  cap_mbps: 40,
  audio: { playing: true, lag_ms: 48, gaps_recent: 0, gaps_span: 0 },
  format: { width: 3840, height: 2160, codec: "HEVC", bit_depth: 10, chroma: "4:2:0", primaries: 1, transfer: 1, matrix: 1, full_range: true },
  budget_ms: 16.67,
  columns: graph(),
  ...more,
});

const transfer = (id: number, stage: Transfer["stage"], more: Partial<Transfer> = {}): Transfer => ({
  outgoing: id % 2 === 0,
  download: true,
  set: null,
  batch: 1,
  image: false,
  transfer: id,
  name: `Съёмка_день_${id}_финальный_монтаж.mov`,
  len: 4_200_000_000,
  done: 1_300_000_000,
  stage,
  reason: null,
  speed: 9_500_000,
  started_ago_ms: 120_000,
  ended_ago_ms: stage === "saved" || stage === "cancelled" ? 1000 : null,
  ...more,
});

const base: OverlayState = {
  picture_scale: 1,
  view: { mode: "all", active: 0, count: 1, first: [0, 0, 1, 1] },
  stats_shown: false,
  stats: null,
  transfers: [],
  transfers_key: 0,
  notice: null,
};

export const OVERLAY_SCENES: Record<string, { title: string; make: () => Partial<OverlayState> }> = {
  stats: { title: "Статистика: всё хорошо", make: () => ({ stats_shown: true, stats: stats() }) },
  lossy: {
    title: "Статистика: сеть теряет кадры",
    make: () => ({ stats_shown: true, stats: stats({ frames_lost_recent: 3, loss_percent: 6.2, shown_fps: 41, rtt_ms: 180, latency_ms: 140, columns: graph(true, true) }) }),
  },
  slow: { title: "Статистика: компьютер не успевает", make: () => ({ stats_shown: true, stats: stats({ decode_p95_ms: 19, decode_ms: 15, latency_ms: 70, shown_fps: 50, columns: graph(true) }) }) },
  noframes: { title: "Статистика: нет изображения", make: () => ({ stats_shown: true, stats: stats({ frames_recent: 0, shown_fps: 0, video_mbps: 0, columns: Array(200).fill(null) }) }) },
  transfers: {
    title: "Передачи файлов",
    make: () => ({
      transfers: [
        transfer(1, "running"),
        transfer(2, "waiting", { done: 0, speed: 0 }),
        transfer(3, "saved", { done: 4_200_000_000, started_ago_ms: 75_000, ended_ago_ms: 1000 }),
        transfer(4, "cancelled", { reason: "locked" }),
      ],
    }),
  },
  clipboard: {
    title: "Файл в буфер: оверлей сам не открывается",
    make: () => ({ transfers: [transfer(1, "running", { download: false, set: 5, name: "Кадр_0412.tif", len: 24_000_000, done: 15_000_000 })] }),
  },
  clipboardKeyed: {
    title: "Буфер: файлы идут, загрузок нет, открыто клавишей",
    make: () => ({
      transfers_key: 1,
      transfers: [
        transfer(1, "running", { download: false, set: 5, len: 24_000_000, done: 15_000_000 }),
        transfer(3, "running", { download: false, set: 5, len: 12_000_000, done: 3_000_000, speed: 0 }),
      ],
    }),
  },
  clipboardAndDownloads: {
    title: "Буфер и две загрузки: «Отменить всё»",
    make: () => ({
      transfers_key: 1,
      transfers: [transfer(1, "running"), transfer(2, "running", { done: 3_100_000_000 }), transfer(3, "saved", { download: false, set: 2, image: true, len: 8_000_000, done: 8_000_000, started_ago_ms: 3000, ended_ago_ms: 2000 })],
    }),
  },
  clipboardFailed: {
    title: "Буфер: не передалось, открыто клавишей",
    make: () => ({ transfers_key: 1, transfers: [transfer(1, "cancelled", { download: false, set: 5, name: "Кадр_0412.tif", len: 24_000_000, done: 6_000_000, reason: "locked" })] }),
  },
  manyDownloads: {
    title: "История сессии: 40 загрузок, прокрутка колесом",
    make: () => ({
      transfers: Array.from({ length: 40 }, (_, i) =>
        i < 37 ? transfer(i + 1, "saved", { done: 4_200_000_000, started_ago_ms: 600_000 - i * 10_000, ended_ago_ms: 590_000 - i * 10_000 }) : transfer(i + 1, "running", { done: 400_000_000 * (i - 36) }),
      ),
    }),
  },
  folder: {
    title: "Папка одной строкой: 143 файла, два из них ещё идут",
    make: () => ({
      transfers: [
        transfer(1, "saved", { batch: 1, name: "Рендер_v3.mov", started_ago_ms: 300_000, ended_ago_ms: 200_000 }),
        ...Array.from({ length: 143 }, (_, i) =>
          i < 141
            ? transfer(10 + i, "saved", { outgoing: false, batch: 2, name: `Проект_Осень/кадры/кадр_${i}.exr`, len: 40_000_000, done: 40_000_000, started_ago_ms: 90_000, ended_ago_ms: 30_000 })
            : transfer(10 + i, "running", { outgoing: false, batch: 2, name: `Проект_Осень/кадры/кадр_${i}.exr`, len: 40_000_000, done: 10_000_000, speed: 6_000_000, started_ago_ms: 90_000 }),
        ),
      ],
    }),
  },
  transfersEmpty: { title: "Передач не было, открыто клавишей", make: () => ({ transfers_key: 1 }) },
  both: { title: "Статистика и передачи", make: () => ({ stats_shown: true, stats: stats(), transfers: [transfer(1, "running")] }) },
  small: { title: "Картинка показана в половину размера", make: () => ({ picture_scale: 0.5, stats_shown: true, stats: stats(), transfers: [transfer(1, "running")] }) },
  mode1: { title: "Режим 1: два монитора", make: () => ({ view: { mode: "all", active: 0, count: 2, first: [0, 0, 0.5, 0.75] }, stats_shown: true, stats: stats(), transfers: [transfer(1, "running")] }) },
  mode2: { title: "Режим 2: кружки мониторов", make: () => ({ view: { mode: "one", active: 1, count: 3, first: [0, 0, 1, 1] } }) },
  notice: { title: "Сообщение (4 с, держится при обновлении страницы)", make: () => ({ stats_shown: true, stats: stats(), notice: { code: "two_windows_on_one_monitor", serial: 1 } }) },
  empty: { title: "Ничего не показано", make: () => ({}) },
};

export const overlaySceneName = new URLSearchParams(location.search).get("scene") ?? "stats";
const scene = (OVERLAY_SCENES[overlaySceneName] ?? OVERLAY_SCENES.stats).make();
/** What the checks and the panel change. */
const overlayMock = { state: { ...base, ...scene } as OverlayState };

// The buttons work here as in a session; the keys are the session's: Ctrl+Alt+Shift+O and D, or just O and D.
(window as any).__overlayMockCommand = (cmd: string, a: any) => {
  const s = overlayMock.state;
  if (cmd === "show_monitor") s.view.active = a.index;
  if (cmd === "cancel_transfer") {
    const t = s.transfers.find((x) => x.transfer === a.transfer && x.outgoing === a.outgoing);
    if (t) Object.assign(t, { stage: "cancelled", reason: "user", ended_ago_ms: 0, speed: 0 });
  }
};
addEventListener("keydown", (e) => {
  const s = overlayMock.state;
  if (e.code === "KeyD") s.transfers_key += 1;
  if (e.code === "KeyO") {
    s.stats_shown = !s.stats_shown;
    s.stats = s.stats_shown ? stats() : null;
  }
});
// What ended is older each time: a block goes after its 5 seconds, as in a session.
setInterval(() => {
  for (const t of overlayMock.state.transfers) {
    t.started_ago_ms += 200;
    if (t.ended_ago_ms !== null) t.ended_ago_ms += 200;
  }
}, 200);

export async function listen<T>(_name: string, handler: (e: { payload: T }) => void): Promise<() => void> {
  const t = setInterval(() => handler({ payload: structuredClone(overlayMock.state) as T }), 200);
  handler({ payload: structuredClone(overlayMock.state) as T });
  return () => clearInterval(t);
}
