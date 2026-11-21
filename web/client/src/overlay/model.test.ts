// `npm test`: what the numbers mean (was in apps/client/src/overlay.rs and transfers.rs).
import assert from "node:assert/strict";
import { test } from "node:test";
import { setLang } from "../i18n.ts";
import type { Stats, Transfer } from "./api.ts";
// The words below are the Russian ones; English is checked at the end.
setLang("ru");

import { breakdown, cancelAllShown, cancellable, downloadLines, goingCount, chartPieces, chartX, clipboardLine, direction, fresh, graphTop, lines, LINGER_MS, metrics, overlayZoom, pressKey, progress, roman, SIDE_BY_SIDE, size, state, status, videoLine, visibleRows, withTimes, type Row } from "./model.ts";

const healthy = (more: Partial<Stats> = {}): Stats => ({
  frames_recent: 120,
  latency_ms: 27,
  net_ms: 20,
  net_p95_ms: 20,
  decode_ms: 5,
  decode_p95_ms: 5,
  show_ms: 2,
  encode_ms: 2.5,
  host_fps: 60,
  fps: 60,
  shown_fps: 60,
  rtt_ms: 10,
  loss_percent: 0,
  frames_lost_recent: 0,
  video_mbps: 21,
  cap_mbps: 40,
  audio: null,
  format: { width: 3840, height: 2160, codec: "HEVC", bit_depth: 10, chroma: "4:2:0", primaries: 1, transfer: 1, matrix: 1, full_range: true },
  budget_ms: 16.67,
  columns: [],
  ...more,
});
const metric = (s: Stats, label: string) => metrics(s).find((m) => m.label === label);

test("numbers of a healthy stream", () => {
  const s = healthy();
  assert.deepEqual(status(s), { level: "normal", title: "Всё хорошо" });
  assert.equal(metric(s, "Задержка")?.value, "27 мс");
  assert.equal(metric(s, "Плавность")?.value, "60 из 60 кадров/с");
  assert.equal(metric(s, "Поток")?.value, "21 из 40 Мбит/с");
  assert.equal(metric(s, "Потери")?.value, "0.0 %");
  const b = breakdown(s)!;
  assert.ok(Math.abs(b.hostMs - 2.5) < 0.01 && Math.abs(b.netMs - 17.5) < 0.01 && Math.abs(b.localMs - 7) < 0.01, JSON.stringify(b));
});

test("the status names the bottleneck", () => {
  const slow = status(healthy({ decode_p95_ms: 18 }));
  assert.equal(slow.level, "bad");
  assert.match(slow.title, /Этот компьютер/);
  const saved = healthy({ loss_percent: 3 });
  assert.equal(status(saved).level, "warn");
  assert.match(status(saved).title, /без последствий/);
  assert.equal(metric(saved, "Потери")?.level, "warn");
  const lossy = healthy({ loss_percent: 3, frames_lost_recent: 2 });
  assert.equal(status(lossy).level, "bad");
  assert.match(status(lossy).title, /теряет кадры/);
  assert.equal(metric(lossy, "Потери")?.level, "bad");
  assert.match(status(healthy({ encode_ms: 15 })).title, /не успевает сжимать/);
  assert.match(status(healthy({ host_fps: 40 })).title, /отдаёт меньше/);
  assert.match(status(healthy({ rtt_ms: 80 })).title, /Долгий отклик/);
  assert.match(status(healthy({ net_p95_ms: 60 })).title, /скачет/);
});

test("the video line names the format", () => {
  const f = healthy().format!;
  assert.equal(videoLine(f), "3840×2160 · захват RGB 8 бит → HEVC 10 бит 4:2:0 · BT.709, полный диапазон");
  const other = videoLine({ ...f, codec: "AV1", chroma: "4:4:4", full_range: false, matrix: 9 });
  assert.ok(other.endsWith("AV1 10 бит 4:4:4 · H.273 1/1/9, ограниченный диапазон"), other);
});

test("the sound and its gaps", () => {
  const audio = { playing: true, lag_ms: 42.4, gaps_recent: 0, gaps_span: 0 };
  assert.equal(metric(healthy({ audio }), "Звук")?.value, "42 мс");
  assert.equal(metric(healthy({ audio }), "Звук")?.level, "normal");
  const gap = healthy({ audio: { ...audio, gaps_recent: 1, gaps_span: 1 } });
  assert.equal(metric(gap, "Звук")?.level, "warn");
  assert.match(status(gap).title, /Звук прерывается/);
  assert.equal(metric(healthy({ audio: { ...audio, playing: false } }), "Звук")?.value, "нет");
  assert.equal(metric(healthy(), "Звук"), undefined);
});

test("no frames lately is said so", () => {
  const s = healthy({ frames_recent: 0 });
  assert.equal(status(s).level, "warn");
  assert.match(status(s).title, /Нет изображения/);
  assert.equal(breakdown(s), null);
  assert.equal(metric(s, "Задержка")?.value, "—");
});

test("a graph's line breaks where no frame came", () => {
  assert.deepEqual(chartPieces([0, 10, null, 20, 40], 20), ["0,100 25,50 ", "75,0 100,0 "], "over the top: at the top");
  assert.deepEqual(chartPieces([null, null], 20), []);
  assert.equal(chartX(0, 1), 0, "one slice: no division by zero");
});

test("monitors have roman numbers", () => {
  assert.deepEqual([1, 2, 3, 4, 5, 9, 12].map(roman), ["I", "II", "III", "IV", "V", "IX", "XII"]);
});

test("the graphs' common top", () => {
  assert.equal(graphTop(healthy({ budget_ms: 16.5 })), 16.5);
  const spiky = healthy({ budget_ms: 10, columns: [...Array(99).fill([30, 0, 0, false, 0]), [900, 0, 0, false, 0]] });
  assert.equal(graphTop(spiky), 45, "a lone spike does not set the scale");
  assert.equal(graphTop(healthy({ columns: Array(10).fill([900, 0, 0, false, 0]) })), 400);
});

test("each graph has its own line", () => {
  const l = lines(healthy({ columns: [[20, 5, 1, false, 3], null, [0, 0, 0, true, 3], [20, 5, 1, true, 3], [2, 5, -4, false, 3]] }));
  assert.deepEqual(l.host, [3, null, null, 3, 2], "the encode, within net");
  assert.deepEqual(l.net, [17, null, null, 17, 0], "net without the encode");
  assert.deepEqual(l.local, [6, null, null, 6, 1]);
  assert.deepEqual(l.lost, [2, 3], "a lost frame with no frame in the slice has no point");
});

const T0 = 1_000_000;
function row(id: number, stage: Transfer["stage"], startedAt: number, endedAt: number | null, more: Partial<Transfer> = {}): Row {
  return {
    outgoing: id % 2 === 0,
    download: true,
    set: null,
    batch: 1,
    image: false,
    transfer: id,
    name: `file${id}.mov`,
    len: 4_200_000_000,
    done: 1_000_000_000,
    stage,
    reason: null,
    speed: 5_000_000,
    started_ago_ms: 0,
    ended_ago_ms: null,
    startedAt,
    endedAt,
    ...more,
  };
}

test("shows while going and lingers", () => {
  const v = fresh();
  assert.equal(visibleRows(v, [], T0), null);
  assert.equal(visibleRows(v, [row(1, "running", T0, null)], T0)?.length, 1);
  const end = T0 + 10_000;
  const done = [row(1, "saved", T0, end)];
  assert.equal(visibleRows(v, done, end + 4000)?.length, 1);
  assert.equal(visibleRows(v, done, end + LINGER_MS), null);
  // Later, with nothing going, it does not come back by itself.
  assert.equal(visibleRows(v, done, end + LINGER_MS * 3), null);
});

test("a row ended at once shows too", () => {
  const v = fresh();
  const refused = row(1, "cancelled", T0, T0, { reason: "not_allowed" });
  assert.equal(visibleRows(v, [refused], T0)?.length, 1);
  assert.equal(visibleRows(v, [refused], T0 + LINGER_MS), null);
});

test("the clipboard's files never bring the block up, however long they take", () => {
  const v = fresh();
  const r = row(1, "running", T0, null, { download: false, set: 4 });
  assert.equal(visibleRows(v, [r], T0), null);
  assert.equal(visibleRows(v, [r], T0 + 60_000), null);
  const end = T0 + 90_000;
  assert.equal(visibleRows(v, [{ ...r, stage: "saved", endedAt: end }], end + 1000), null);
});

test("opened by the key, the block lists only downloads", () => {
  const v = fresh();
  v.manual = { kind: "pinned" };
  const clip = row(1, "running", T0, null, { download: false, set: 4 });
  const download = row(3, "running", T0, null);
  assert.deepEqual(visibleRows(v, [clip, download], T0)!.map((r) => r.transfer), [3]);
  assert.deepEqual(visibleRows(v, [clip], T0), []);
});

test("the clipboard has one line: the newest set, as a whole", () => {
  assert.equal(clipboardLine([row(3, "running", T0, null)]), null, "a download is not the clipboard");
  const clip = (id: number, more: Partial<Transfer>) => row(id, "running", T0, null, { download: false, set: 4, len: 1000, done: 500, speed: 0, ...more });
  const old = clip(2, { set: 3, name: "old.mov", started_ago_ms: 9000, stage: "cancelled", reason: "user", ended_ago_ms: 8000 });
  const a = clip(4, { started_ago_ms: 2000 });
  const b = clip(6, { started_ago_ms: 1000 });
  const line = clipboardLine([old, a, b])!;
  assert.equal(line.direction, "Клиент → Хост");
  assert.equal(line.name, "2 файла");
  assert.equal(line.text, "1 КБ из 2 КБ");
  assert.equal(line.share, 0.5);
  assert.deepEqual(line.going.map((t) => t.transfer), [4, 6], "the old set's are not in it");
  assert.equal(clipboardLine([{ ...a, name: "a.mov" }])!.name, "a.mov");
  assert.equal(clipboardLine([{ ...a, image: true }, { ...b, image: true }])!.name, "Картинка");
  const saved = clipboardLine([{ ...a, stage: "saved" }, { ...b, stage: "saved" }])!;
  assert.deepEqual([saved.text, saved.tone, saved.share, saved.going.length], ["готово · 2 КБ", "done", 1, 0]);
  const failed = clipboardLine([{ ...a, stage: "saved" }, { ...b, stage: "cancelled", reason: "locked" }])!;
  assert.deepEqual([failed.text, failed.tone], ["хост заблокирован — войдите в Windows на хосте", "bad"]);
});

test("«Отменить всё» takes everything that goes, the clipboard's too", () => {
  const list = [row(1, "running", T0, null), row(2, "waiting", T0, null, { download: false, set: 1 }), row(3, "checking", T0, null), row(5, "saved", T0, T0), row(7, "cancelled", T0, T0)];
  assert.deepEqual(cancellable(list).map((t) => t.transfer), [1, 2, 3]);
  const clip = (id: number) => row(id, "running", T0, null, { download: false, set: 1 });
  assert.equal(cancelAllShown([row(1, "running", T0, null)]), false, "one download");
  assert.equal(cancelAllShown([clip(2), clip(4)]), false, "the clipboard is one thing, with its own button");
  assert.equal(cancelAllShown([row(1, "running", T0, null), clip(2)]), true);
  assert.equal(cancelAllShown([row(1, "running", T0, null), row(3, "running", T0, null)]), true);
  assert.equal(cancelAllShown([row(1, "saved", T0, T0), row(3, "running", T0, null)]), false, "what ended does not count");
});

test("a download that ended lingers, and goes", () => {
  const v = fresh();
  const r = row(1, "running", T0, null);
  assert.equal(visibleRows(v, [r], T0 + 500)?.length, 1);
  const end = T0 + 3000;
  const done = { ...r, stage: "saved" as const, endedAt: end };
  assert.equal(visibleRows(v, [done], end)?.length, 1);
  assert.equal(visibleRows(v, [done], end + 2 * LINGER_MS), null);
});

test("the key hides and shows the history", () => {
  const v = fresh();
  const old = row(1, "saved", T0, T0 + 1000);
  const on = row(2, "running", T0 + 2000, null);
  const now = T0 + 60_000;
  assert.equal(visibleRows(v, [old, on], now)?.length, 2, "up by itself: the whole session too");
  pressKey(v, true, now);
  assert.equal(visibleRows(v, [old, on], now), null);
  pressKey(v, false, now);
  assert.equal(visibleRows(v, [old, on], now)?.length, 2, "everything of the session");
  // Hidden, then a new transfer starts: back by itself.
  pressKey(v, true, now);
  const next = row(3, "running", now + 1000, null);
  assert.equal(visibleRows(v, [old, on, next], now + 1000)?.length, 3);
  // Hidden, then a paste is refused: back too, with the reason.
  const later = now + 30_000;
  pressKey(v, true, later);
  const refused = row(4, "cancelled", later + 1000, later + 1000, { reason: "locked" });
  assert.ok(visibleRows(v, [old, on, next, refused], later + 1000)?.some((r) => r.transfer === 4));
});

test("no cap on the rows: the session's downloads, oldest first", () => {
  const v = fresh();
  v.manual = { kind: "pinned" };
  const many = Array.from({ length: 150 }, (_, i) => row(i + 1, "saved", T0 + i, T0 + i + 1));
  const shown = visibleRows(v, many, T0 + 60_000)!;
  assert.equal(shown.length, 150);
  assert.deepEqual([shown[0].transfer, shown[149].transfer], [1, 150]);
});

test("a folder of one paste is one line; files apart and other pastes are not in it", () => {
  const f = (id: number, more: Partial<Transfer>) => row(id, "running", T0, null, { outgoing: false, len: 10_000, done: 5000, speed: 0, ...more });
  const list = [
    f(1, { name: "a.mov" }),
    f(2, { name: "Проект/x/1.exr" }),
    f(3, { name: "Проект/2.exr", stage: "saved", done: 10_000 }),
    f(4, { name: "Проект/3.exr", batch: 2 }),
    f(5, { name: "Проект/4.exr", outgoing: true }),
    f(6, { name: "Другая/1.exr" }),
  ];
  const lines = downloadLines(list);
  assert.deepEqual(
    lines.map((l) => [l.name, l.files.map((t) => t.transfer)]),
    [
      ["a.mov", [1]],
      ["Проект · 2 файла", [2, 3]],
      ["Проект · 1 файл", [4]],
      ["Проект · 1 файл", [5]],
      ["Другая · 1 файл", [6]],
    ],
  );
  const folder = lines[1];
  assert.equal(folder.share, 0.75);
  assert.deepEqual(folder.going.map((t) => t.transfer), [2], "«Отменить» takes what still goes");
  assert.equal(folder.text, "15 КБ из 20 КБ");
  assert.equal(goingCount(list), 5, "a folder counts once");
  assert.equal(cancelAllShown([f(2, { name: "П/1" }), f(3, { name: "П/2" })]), false, "one folder: its own «Отменить»");
});

test("a folder that ended: done, or what did not come", () => {
  const f = (id: number, more: Partial<Transfer>) => row(id, "saved", T0, T0, { len: 1_000_000, done: 1_000_000, name: `П/${id}`, started_ago_ms: 70_000, ended_ago_ms: 5000, ...more });
  const done = downloadLines([f(1, {}), f(3, { started_ago_ms: 60_000, ended_ago_ms: 10_000 })])[0];
  assert.deepEqual([done.text, done.tone, done.share], ["готово · 2.0 МБ за 1 мин 5 с", "done", 1]);
  const some = downloadLines([f(1, {}), f(3, { stage: "cancelled", reason: "disk" })])[0];
  assert.deepEqual([some.text, some.tone], ["не передано 1 из 2 · нет места на диске", "bad"]);
  const all = downloadLines([f(1, { stage: "cancelled", reason: "user" })])[0];
  assert.equal(all.tone, "bad");
});

test("ages become times", () => {
  const [r] = withTimes([{ ...row(1, "saved", 0, 0), started_ago_ms: 5000, ended_ago_ms: 1000 }], T0);
  assert.deepEqual([r.startedAt, r.endedAt], [T0 - 5000, T0 - 1000]);
});

test("words", () => {
  const r = row(1, "running", T0, null);
  assert.equal(direction(r), "Хост → Клиент");
  assert.deepEqual(state(r), { text: "1.00 ГБ из 4.20 ГБ · 40.0 Мбит/с · осталось 10 мин 40 с", tone: "normal" });
  assert.deepEqual(state(row(2, "cancelled", T0, T0, { reason: "disk" })), { text: "нет места на диске", tone: "bad" });
  assert.equal(size(512), "512 Б");
  assert.ok(Math.abs(progress(r) - 1 / 4.2) < 1e-6);
  assert.equal(progress(row(1, "saved", T0, T0)), 1);
  const saved = { ...row(1, "saved", T0, T0), started_ago_ms: 75_000, ended_ago_ms: 5000 };
  assert.equal(state(saved).text, "готово · 4.20 ГБ за 1 мин 10 с");
});

test("the overlays follow the picture", () => {
  assert.equal(overlayZoom(1, 3840), 1);
  assert.equal(overlayZoom(0.5, 3840), 0.5, "shown at half size: half as large");
  assert.equal(overlayZoom(2, 3840), 1, "shown larger: not larger");
  const narrow = overlayZoom(1, 1280);
  assert.ok(narrow < 1 && narrow * SIDE_BY_SIDE <= 1280 + 1e-3, "narrow: the two keep apart");
});

test("English: units, plurals and reasons", () => {
  setLang("en");
  assert.equal(size(1_500_000), "1.5 MB");
  assert.equal(direction({ outgoing: true } as Transfer), "Client → Host");
  const t = { outgoing: false, download: false, set: 1, image: false, transfer: 1, name: "a", len: 10, done: 10, stage: "saved", reason: null, speed: 0, started_ago_ms: 0, ended_ago_ms: 0 } as Transfer;
  assert.equal(clipboardLine([t, { ...t, transfer: 2 }])?.name, "2 files");
  assert.equal(state({ ...t, stage: "cancelled", reason: "disk" }).text, "no space on the disk");
  setLang("ru");
});
