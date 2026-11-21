// Everything over the video, in one transparent window over the main window's picture. The look lives here, in overlay.css
// and in the styles all of Lumen shares; what it shows comes through api.ts. Where this page takes clicks is marked with `data-interactive`.
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { words } from "../i18n";
import { overlayApi, type OverlayState, type Stats as StatsData, type Transfer } from "./api";
import { breakdown, cancelAllShown, cancellable, chartPieces, chartX, clipboardLine, downloadLines, fresh, goingCount, graphTop, lines, metrics, NOTICE_MS, noticeText, overlayZoom, pressKey, roman, status, videoLine, visibleRows, withTimes, type Level, type Row, type Tone, type Visibility } from "./model";

const W = words(
  {
    stats: "Statistics",
    metrics: "Metrics",
    latency10: "Latency over 10 seconds",
    upTo: (top: string) => `up to ${top} ms · a red mark is a lost frame`,
    host: "Host",
    network: "Network",
    thisComputer: "This computer",
    video: "Video",
    ms: "ms",
    cancel: "Cancel",
    transfers: "Transfers",
    cancelAll: "Cancel all",
    downloads: "Downloads",
    noDownloads: "No downloads in this session",
    clipboard: "Clipboard",
    noFiles: "No files",
  },
  {
    stats: "Статистика",
    metrics: "Показатели",
    latency10: "Задержка за 10 секунд",
    upTo: (top: string) => `до ${top} мс · красная отметка — потерянный кадр`,
    host: "Хост",
    network: "Сеть",
    thisComputer: "Этот компьютер",
    video: "Видео",
    ms: "мс",
    cancel: "Отменить",
    transfers: "Передачи",
    cancelAll: "Отменить всё",
    downloads: "Загрузки",
    noDownloads: "Загрузок в этой сессии нет",
    clipboard: "Буфер",
    noFiles: "Файлов нет",
  },
);

export function Overlay() {
  const [s, setS] = useState<OverlayState | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    const off = overlayApi.onState(setS);
    return () => {
      off.then((f) => f());
    };
  }, []);

  // Transfers: when to show is decided here, the core only counts the key.
  const visibility = useRef<Visibility>(fresh());
  const lastKey = useRef(0);
  const rowsOnScreen = useRef(false);
  const now = Date.now();
  let rows: Row[] | null = null;
  if (s) {
    if (s.transfers_key !== lastKey.current) {
      if (lastKey.current !== 0 || s.transfers_key > 0) pressKey(visibility.current, rowsOnScreen.current, now);
      lastKey.current = s.transfers_key;
    }
    rows = visibleRows(visibility.current, withTimes(s.transfers, now), now);
    rowsOnScreen.current = rows !== null;
  }

  // A notice stays for a moment.
  const notice = s?.notice;
  const [noticeUntil, setNoticeUntil] = useState<{ serial: number; until: number } | null>(null);
  useEffect(() => {
    if (notice && notice.serial !== noticeUntil?.serial) setNoticeUntil({ serial: notice.serial, until: Date.now() + NOTICE_MS });
  }, [notice, noticeUntil]);
  const noticeShown = !!notice && !!noticeUntil && noticeUntil.serial === notice.serial && now < noticeUntil.until;

  const switcher = !!s && s.view.mode === "one" && s.view.count > 1;
  const stats = s?.stats_shown ? s.stats : null;
  const visible = !!stats || rows !== null || noticeShown || switcher;

  // The blocks' scale follows the window's width (overlayZoom): anew at once when the core resizes the window.
  useEffect(() => {
    const resized = () => tick((n) => n + 1);
    addEventListener("resize", resized);
    return () => removeEventListener("resize", resized);
  }, []);

  // The state comes at most once a second when nothing goes: what ends (a block's linger, a notice) needs its own clock.
  useEffect(() => {
    if (!visible) return;
    const t = setInterval(() => tick((n) => n + 1), 250);
    return () => clearInterval(t);
  }, [visible]);

  // What the core needs: whether to show the window, and where it takes clicks.
  const reported = useRef("");
  useLayoutEffect(() => {
    const dpr = window.devicePixelRatio || 1;
    const interactive = [...document.querySelectorAll("[data-interactive]")].map((e) => {
      const r = e.getBoundingClientRect();
      return [Math.floor(r.left * dpr), Math.floor(r.top * dpr), Math.ceil(r.width * dpr), Math.ceil(r.height * dpr)];
    });
    const report = JSON.stringify([visible, interactive]);
    if (report !== reported.current) {
      reported.current = report;
      overlayApi.setUi(visible, interactive);
    }
  });

  if (!s || !visible) return null;
  // Mode 1: on monitor 1's picture, not across the whole layout.
  const [x, y, w, h] = s.view.mode === "all" ? s.view.first : [0, 0, 1, 1];
  const zoom = overlayZoom(s.picture_scale, window.innerWidth * w);
  return (
    <div className="stage" style={{ left: `${x * 100}%`, top: `${y * 100}%`, width: `${w * 100}%`, height: `${h * 100}%`, ["--k" as string]: zoom }}>
      {switcher && <Switcher count={s.view.count} active={s.view.active} />}
      {stats && <StatsBlock s={stats} />}
      {rows && <Transfers rows={rows} all={s.transfers} />}
      {noticeShown && notice && <div className="notice">{noticeText(notice.code)}</div>}
    </div>
  );
}

/** Mode 2: which monitor is shown; a click shows another. */
function Switcher({ count, active }: { count: number; active: number }) {
  return (
    <div className="switcher">
      {Array.from({ length: count }, (_, i) => (
        <button key={i} data-interactive className={i === active ? "dot on" : "dot"} onClick={() => overlayApi.showMonitor(i)}>
          {roman(i + 1)}
        </button>
      ))}
    </div>
  );
}

/** The heading and the panel, as in the admin's blocks. */
function Section({ title, side, children, className }: { title: ReactNode; side?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section>
      <div className="section-title">
        <span>{title}</span>
        {side && <span className="section-side">{side}</span>}
      </div>
      <div className={className ? `panel ${className}` : "panel"}>{children}</div>
    </section>
  );
}

const PILL: Record<Level, string> = { normal: "ok", dim: "", warn: "wait", bad: "bad" };

function StatsBlock({ s }: { s: StatsData }) {
  const w = W();
  const st = status(s);
  const parts = breakdown(s);
  const top = graphTop(s);
  const l = lines(s);
  return (
    <div className="block stats">
      <header className="block-head">
        <h3>{w.stats}</h3>
        <span className={`pill ${PILL[st.level]}`}>
          <i />
          {st.title}
        </span>
      </header>
      <div className="block-body">
        <Section title={w.metrics} className="metrics">
          {metrics(s).map((m) => (
            <div className="metric" key={m.label}>
              <span>{m.label}</span>
              <b className={m.level}>{m.value}</b>
            </div>
          ))}
        </Section>
        <Section title={w.latency10} side={w.upTo(top.toFixed(0))} className="charts">
          <Chart name={w.host} value={parts?.hostMs} line={l.host} top={top} />
          <Chart name={w.network} value={parts?.netMs} line={l.net} top={top} lost={l.lost} />
          <Chart name={w.thisComputer} value={parts?.localMs} line={l.local} top={top} />
        </Section>
        {s.format && (
          <Section title={w.video} className="video-line">
            {videoLine(s.format)}
          </Section>
        )}
      </div>
    </div>
  );
}

/** One measure, one line; all on the same scale. The block takes no clicks: it has no hover. */
function Chart({ name, value, line, top, lost = [] }: { name: string; value?: number; line: (number | null)[]; top: number; lost?: number[] }) {
  return (
    <div className="chart">
      <div className="chart-head">
        <span>{name}</span>
        <b>{value === undefined ? "—" : `${value.toFixed(0)} ${W().ms}`}</b>
      </div>
      <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-label={name}>
        {lost.map((i) => (
          <line key={i} className="chart-lost" x1={chartX(i, line.length)} x2={chartX(i, line.length)} y1="0" y2="100" />
        ))}
        {chartPieces(line, top).map((p, i) => (
          <polyline key={i} className="chart-line" points={p} />
        ))}
      </svg>
    </div>
  );
}

/** A transfer, or the clipboard: its name, how it goes, the bar. */
function TransferRow({ name, way, text, tone, share, cancel, className }: { name: string; way: string; text: string; tone: Tone; share: number; cancel?: () => void; className?: string }) {
  return (
    <div className={className ? `row ${className}` : "row"}>
      <div className="row-head">
        <b title={name}>{name}</b>
        {cancel && (
          <button className="btn small" onClick={cancel}>
            {W().cancel}
          </button>
        )}
      </div>
      <div className="row-way">{way}</div>
      <div className={`transfer-state ${tone}`}>{text}</div>
      <div className="bar">
        <i className={tone} style={{ width: `${share * 100}%` }} />
      </div>
    </div>
  );
}

/** Downloads upward, the newest next to the clipboard, which is always at the bottom. */
function Transfers({ rows, all }: { rows: Row[]; all: Transfer[] }) {
  const w = W();
  const on = cancellable(all);
  const count = goingCount(all);
  const clip = clipboardLine(all);
  const cancel = (list: Transfer[]) => list.forEach((t) => overlayApi.cancelTransfer(t.outgoing, t.transfer));
  return (
    <div className="block transfers" data-interactive>
      <header className="block-head">
        <h3>
          {w.transfers}
          {count > 0 ? ` · ${count}` : ""}
        </h3>
        {cancelAllShown(all) && (
          <button className="btn small" onClick={() => cancel(on)}>
            {w.cancelAll}
          </button>
        )}
      </header>
      <div className="block-body">
        <Section title={w.downloads} className="rows-list">
          {rows.length === 0 && <div className="row empty">{w.noDownloads}</div>}
          {downloadLines(rows)
            .reverse()
            .map((l) => (
              <TransferRow key={l.key} className="transfer" name={l.name} way={l.direction} text={l.text} tone={l.tone} share={l.share} cancel={l.going.length > 0 ? () => cancel(l.going) : undefined} />
            ))}
        </Section>
        <Section title={w.clipboard} className="rows-list clipboard">
          {clip ? <TransferRow name={clip.name} way={clip.direction} text={clip.text} tone={clip.tone} share={clip.share} cancel={clip.going.length > 0 ? () => cancel(clip.going) : undefined} /> : <div className="row empty">{w.noFiles}</div>}
        </Section>
      </div>
    </div>
  );
}
