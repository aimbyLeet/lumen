import { words } from "../../client/src/i18n";
import { byAspect } from "../../client/src/ui";
import type { Align, Resolution, Side, Virtual } from "./api";

const W = words(
  {
    monitor: "Monitor",
    name: "Name",
    resolution: "Resolution",
    resolution60: "Resolution, 60 Hz",
    realNote: "A real monitor is set up in Windows.",
    scale: "Scale",
    place: "Place",
    primary: "Primary",
    over: "Extra: a client gets at most 3 monitors.",
    sides: { right: "Right of", left: "Left of", above: "Above", below: "Below" } as Record<Side, string>,
    ofMonitor: "monitor",
    center: "centered",
    top: "top edges aligned",
    left: "left edges aligned",
    bottom: "bottom edges aligned",
    right: "right edges aligned",
  },
  {
    monitor: "Монитор",
    name: "Название",
    resolution: "Разрешение",
    resolution60: "Разрешение, 60 Гц",
    realNote: "Настоящий монитор настраивается в Windows.",
    scale: "Масштаб",
    place: "Место",
    primary: "Основной",
    over: "Лишний: клиенту передаётся не больше 3 мониторов.",
    sides: { right: "Справа от", left: "Слева от", above: "Сверху от", below: "Снизу от" },
    ofMonitor: "монитора",
    center: "по центру",
    top: "по верхнему краю",
    left: "по левому краю",
    bottom: "по нижнему краю",
    right: "по правому краю",
  },
);

function alignName(side: Side, align: Align): string {
  const w = W();
  if (align === "center") return w.center;
  const across = side === "left" || side === "right";
  if (align === "start") return across ? w.top : w.left;
  return across ? w.bottom : w.right;
}

/** The picked real monitor: only what it is. */
export function RealMonitorPanel({ number, name, width, height }: { number: number; name: string; width: number; height: number }) {
  const w = W();
  return (
    <aside className="mon-panel">
      <header>
        <h3>
          {w.monitor} {number}
        </h3>
      </header>
      <dl className="mon-facts">
        <dt>{w.name}</dt>
        <dd>{name || "—"}</dd>
        <dt>{w.resolution}</dt>
        <dd>
          {width}×{height}
        </dd>
      </dl>
      <p className="mon-note">{w.realNote}</p>
    </aside>
  );
}

type Props = {
  number: number;
  v: { width: number; height: number; scale: number };
  /** Where it stands now, dragged or not; `null`: the client's own monitor 1, the others stand next to it. */
  attach: Virtual["attach"] | null;
  resolutions: Resolution[];
  scales: number[];
  disabled: boolean;
  over: boolean;
  onChange: (patch: Partial<Pick<Virtual, "width" | "height" | "scale">>) => void;
};

/** The picked virtual monitor: its resolution and scale in fields, its place in words. */
export function VirtualMonitorPanel({ number, v, attach, resolutions, scales, disabled, over, onChange }: Props) {
  const w = W();
  return (
    <aside className="mon-panel">
      <header>
        <h3>
          {w.monitor} {number}
        </h3>
      </header>
      <label className="field">
        <span>{w.resolution60}</span>
        <select
          aria-label={w.resolution}
          disabled={disabled}
          value={`${v.width}x${v.height}`}
          onChange={(e) => {
            const [width, height] = e.target.value.split("x").map(Number);
            onChange({ width, height });
          }}
        >
          {byAspect(resolutions).map(([aspect, rs]) => (
            <optgroup key={aspect} label={aspect}>
              {rs.map((r) => (
                <option key={`${r.width}x${r.height}`} value={`${r.width}x${r.height}`}>
                  {r.width}×{r.height}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
      </label>
      <label className="field">
        <span>{w.scale}</span>
        <select aria-label={w.scale} disabled={disabled} value={v.scale} onChange={(e) => onChange({ scale: Number(e.target.value) })}>
          {scales.map((sc) => (
            <option key={sc} value={sc}>
              {sc} %
            </option>
          ))}
        </select>
      </label>
      <dl className="mon-facts">
        <dt>{w.place}</dt>
        <dd className="mon-place">{attach ? `${w.sides[attach.side]} ${w.ofMonitor} ${attach.to}, ${alignName(attach.side, attach.align)}` : w.primary}</dd>
      </dl>
      {over && <p className="mon-note field-error">{w.over}</p>}
    </aside>
  );
}
