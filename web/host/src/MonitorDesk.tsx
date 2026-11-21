import { words } from "../../client/src/i18n";
import { Icon, Pill } from "../../client/src/ui";
import type { Rect } from "./api";
import { LUMEN_MONITOR, anchor } from "./layout";
import type { Desk } from "./useDesk";

const W = words(
  {
    limit: "A client gets at most 3 monitors",
    off: "Monitors are set up while the host is on",
    moving: "Release to place it. Esc cancels",
    dragClient: "Drag monitor 2 or 3: it snaps to the edge of its neighbour",
    drag: "Drag a virtual monitor: it snaps to the edge of its neighbour",
    addClient: "Press + to add a monitor",
    add: "Press + to add a virtual monitor",
    firstTitle: "The primary monitor: the others are placed next to it",
    realTitle: "A real monitor: its place is set in Windows settings",
    virtualTitle: "Drag to place it elsewhere",
    removeOne: "Remove the monitor",
    removeNumber: (n: number) => `Remove monitor ${n}`,
    on: "On",
    offPill: "Off",
    addMonitor: "Add a monitor",
  },
  {
    limit: "Клиенту передаётся не больше 3 мониторов",
    off: "Мониторы настраиваются, пока хост включён",
    moving: "Отпустите, чтобы поставить. Esc — отмена",
    dragClient: "Перетащите монитор 2 или 3: он прилипнет к краю соседнего",
    drag: "Перетащите виртуальный монитор: он прилипнет к краю соседнего",
    addClient: "Нажмите +, чтобы добавить монитор",
    add: "Нажмите +, чтобы добавить виртуальный монитор",
    firstTitle: "Основной монитор: остальные ставятся рядом с ним",
    realTitle: "Настоящий монитор: его место задают настройки Windows",
    virtualTitle: "Перетащите, чтобы поставить в другое место",
    removeOne: "Убрать монитор",
    removeNumber: (n: number) => `Убрать монитор ${n}`,
    on: "Включены",
    offPill: "Выключены",
    addMonitor: "Добавить монитор",
  },
);

/** What the scheme shows about a virtual monitor. */
export type Mark = { over: boolean; pending: boolean; asleep: boolean };

/** A client's own monitor 1 on a host without a physical one: virtual, but where it is stays put. */
type First = { scale: number; pending: boolean };

type Props = {
  desk: Desk;
  /** All the monitors, the real ones first. */
  rects: Rect[];
  real: { name: string }[];
  /** Of the virtual ones. */
  scales: number[];
  marks: Mark[];
  pick: number;
  /** The virtual monitors are on. */
  on: boolean;
  /** The service does not answer. */
  off: boolean;
  disabled: boolean;
  count: string;
  canAdd: boolean;
  onAdd: () => void;
  onRemove: (i: number) => void;
  /** Monitor 1 is the client's own. */
  first?: First;
};

function hint(off: boolean, moving: boolean, hasVirtual: boolean, canEverAdd: boolean, client: boolean): string {
  const w = W();
  if (off) return w.off;
  if (moving) return w.moving;
  if (hasVirtual) return client ? w.dragClient : w.drag;
  if (!canEverAdd) return w.limit;
  return client ? w.addClient : w.add;
}

function title(isReal: boolean, isFirst: boolean): string {
  const w = W();
  if (isFirst) return w.firstTitle;
  return isReal ? w.realTitle : w.virtualTitle;
}

export function MonitorDesk({ desk, rects, real, scales, marks, pick, on, off, disabled, count, canAdd, onAdd, onRemove, first }: Props) {
  const w = W();
  const realCount = real.length;
  const { moving } = desk;
  const dragged = moving ? rects[realCount + moving.index] : null;
  return (
    <div className={`mon-canvas ${off ? "locked" : ""}`} ref={desk.canvas}>
      {desk.measured &&
        rects.map((_, n) => {
          const isFirst = !!first && n === 0;
          const isReal = n < realCount && !isFirst;
          const mark = isFirst ? { over: false, pending: first.pending, asleep: false } : n < realCount ? null : marks[n - realCount];
          const held = !!moving && n === realCount + moving.index;
          const r = desk.shown(n);
          const number = n + 1;
          return (
            <div
              key={n}
              className={`mon ${isReal ? "real" : "virtual"} ${isFirst ? "anchor" : ""} ${n === pick ? "picked" : ""} ${mark?.pending ? "pending" : ""} ${mark?.asleep ? "asleep" : ""} ${mark?.over ? "over" : ""} ${held ? "held" : ""}`}
              style={desk.box(r)}
              data-monitor={number}
              {...desk.handlers(n)}
              title={title(isReal, isFirst)}
            >
              <b>{number}</b>
              <span>
                {r.width}×{r.height}
              </span>
              <em>{isFirst ? `${LUMEN_MONITOR} · ${first.scale} %` : isReal ? real[n].name : `${LUMEN_MONITOR} · ${scales[n - realCount]} %`}</em>
              {isFirst ? null : isReal ? (
                <i className="mon-lock">
                  <Icon name="lock" size={12} />
                </i>
              ) : (
                !disabled &&
                !held && (
                  <button className="mon-close" title={w.removeOne} aria-label={w.removeNumber(number)} onPointerDown={(e) => e.stopPropagation()} onClick={() => onRemove(n - realCount)}>
                    <Icon name="close" size={14} />
                  </button>
                )
              )}
            </div>
          );
        })}
      {moving && dragged && (
        <>
          {moving.target && !moving.stuck && <div className="mon-slot" style={desk.box({ ...dragged, x: moving.target.x, y: moving.target.y })} />}
          {moving.free.map((c, k) => (
            <i key={k} className={`mon-snap ${c === moving.target ? "on" : ""}`} style={desk.point(anchor(rects[c.attach.to - 1], c))} />
          ))}
        </>
      )}
      <div className="mon-status">
        {/* All the virtual monitors together. Unapplied changes show on the scheme and the buttons. */}
        <Pill tone={on ? "on" : "idle"}>{on ? w.on : w.offPill}</Pill>
      </div>
      <div className="mon-tools">
        <span className="mon-count" title={w.limit}>
          {count}
        </span>
        <button className="btn square primary" aria-label={w.addMonitor} title={canAdd ? w.addMonitor : w.limit} disabled={disabled || !canAdd} onClick={onAdd}>
          <Icon name="add" size={20} />
        </button>
      </div>
      <p className="mon-hint">{hint(off, !!moving, marks.length > 0, canAdd, !!first)}</p>
    </div>
  );
}
