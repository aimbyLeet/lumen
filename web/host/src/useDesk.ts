// The desk of the Monitors tab: how the monitors' places map to the screen, and dragging one of them.
import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import type { Rect, Virtual } from "./api";
import { freeCandidates, holders, itemsOf, nearest, settle, type Candidate, type Settled } from "./layout";

/** Within this many screen pixels of a snap point the monitor sticks to it. */
const STICK_PX = 22;
/** Less than this is a click, not a drag. */
const MOVED_PX = 3;

export type View = { scale: number; left: number; top: number };

/** The picture fits with room around it to drop a monitor on any side. */
function fit(rects: Rect[], virtuals: Virtual[], w: number, h: number): View {
  const minX = Math.min(...rects.map((r) => r.x));
  const minY = Math.min(...rects.map((r) => r.y));
  const maxX = Math.max(...rects.map((r) => r.x + r.width));
  const maxY = Math.max(...rects.map((r) => r.y + r.height));
  const roomX = Math.max(1920, ...virtuals.map((v) => v.width)) * 0.75;
  const roomY = Math.max(1080, ...virtuals.map((v) => v.height)) * 0.75;
  const scale = Math.min(w / (maxX - minX + 2 * roomX), h / (maxY - minY + 2 * roomY));
  return { scale, left: w / 2 - ((minX + maxX) / 2) * scale, top: h / 2 - ((minY + maxY) / 2) * scale };
}

export type Drag = {
  /** Index in the list of virtual monitors. */
  index: number;
  grabX: number;
  grabY: number;
  startX: number;
  startY: number;
  /** The picture does not rescale under the mouse. */
  view: View;
  free: Candidate[];
  moved: boolean;
  x: number;
  y: number;
  target: Candidate | null;
  stuck: boolean;
  /** How everything stands if it is dropped now. */
  preview: Settled | null;
};

type Args = {
  real: Rect[];
  virtuals: Virtual[];
  /** All the monitors, the real ones first. */
  rects: Rect[];
  /** Nothing can be moved now. */
  locked: boolean;
  onPick: (n: number) => void;
  /** Dropped: the new arrangement, and the dragged monitor's new index. */
  onDrop: (virtuals: Virtual[], index: number) => void;
};

export function useDesk({ real, virtuals, rects, locked, onPick, onDrop }: Args) {
  const canvas = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [drag, setDrag] = useState<Drag | null>(null);
  const realCount = real.length;
  const view = drag?.view ?? fit(rects, virtuals, size.w, size.h);
  const dragging = !!drag;
  /** Being dragged by now, not just pressed. */
  const moving = drag?.moved ? drag : null;

  useLayoutEffect(() => {
    const el = canvas.current!;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const seen = new ResizeObserver(measure);
    seen.observe(el);
    return () => seen.disconnect();
  }, []);

  useEffect(() => {
    if (!dragging) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDrag(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dragging]);

  /** A place on the desk under the mouse. */
  const at = (e: PointerEvent, v: View) => {
    const box = canvas.current!.getBoundingClientRect();
    return { x: (e.clientX - box.left - v.left) / v.scale, y: (e.clientY - box.top - v.top) / v.scale };
  };

  const follow = (d: Drag, x: number, y: number): Drag => {
    const target = nearest(d.free, x, y);
    if (!target) return { ...d, x, y, target: null, stuck: false, preview: null };
    const stuck = Math.hypot(target.x - x, target.y - y) * d.view.scale < STICK_PX;
    const items = itemsOf(real, virtuals).map((item, j) => (j === d.index ? { ...item, x: target.x, y: target.y } : item));
    return { ...d, x, y, target, stuck, preview: settle(real, items) };
  };

  const handlers = (n: number) => ({
    onPointerDown: (e: PointerEvent<HTMLDivElement>) => {
      onPick(n);
      if (locked || n < realCount || e.button !== 0) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      const r = rects[n];
      const grab = at(e, view);
      setDrag({
        index: n - realCount,
        grabX: grab.x - r.x,
        grabY: grab.y - r.y,
        startX: e.clientX,
        startY: e.clientY,
        view,
        free: freeCandidates(rects, holders(rects, realCount, n), r, rects.filter((_, j) => j !== n)),
        moved: false,
        x: r.x,
        y: r.y,
        target: null,
        stuck: false,
        preview: null,
      });
    },
    onPointerMove: (e: PointerEvent<HTMLDivElement>) => {
      if (!drag) return;
      if (!drag.moved && Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < MOVED_PX) return;
      const p = at(e, drag.view);
      setDrag(follow({ ...drag, moved: true }, p.x - drag.grabX, p.y - drag.grabY));
    },
    onPointerUp: () => {
      if (!drag) return;
      if (drag.moved && drag.preview) onDrop(drag.preview.virtuals, drag.preview.order.indexOf(drag.index));
      setDrag(null);
    },
    onPointerCancel: () => setDrag(null),
  });

  /** Where a monitor stands now: while dragging, the others may have moved aside. */
  const shown = (n: number): Rect => {
    if (moving && n === realCount + moving.index) {
      const place = moving.stuck && moving.target ? moving.target : moving;
      return { ...rects[n], x: place.x, y: place.y };
    }
    if (moving?.preview && n >= realCount) return moving.preview.rects[realCount + moving.preview.order.indexOf(n - realCount)];
    return rects[n];
  };
  /** What a virtual monitor is attached to now, dropped or about to be. */
  const attachOf = (i: number) => (moving?.preview ? moving.preview.virtuals[moving.preview.order.indexOf(i)].attach : virtuals[i].attach);

  const box = (r: Rect) => ({ left: view.left + r.x * view.scale, top: view.top + r.y * view.scale, width: r.width * view.scale, height: r.height * view.scale });
  const point = (p: { x: number; y: number }) => ({ left: view.left + p.x * view.scale, top: view.top + p.y * view.scale });

  return { canvas, measured: size.w > 0, moving, handlers, shown, attachOf, box, point };
}

export type Desk = ReturnType<typeof useDesk>;
