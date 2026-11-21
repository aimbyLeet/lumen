// How monitors stand next to each other, for the scheme. The core's rule (vdd::layout::place) stays:
// a virtual monitor attaches to a monitor with a smaller number by a side and an alignment (12 snap points on
// each neighbour). The scheme works in desktop positions and turns them back into that rule (`settle`).
import type { Align, Rect, Side, Virtual } from "./api";

const SIDES: Side[] = ["right", "left", "above", "below"];
const ALIGNS: Align[] = ["start", "center", "end"];

export type Size = { width: number; height: number };
/** A place for a monitor: where it stands and what the core would be told. */
export type Candidate = { x: number; y: number; attach: Virtual["attach"] };
export type Item = { v: Virtual; x: number; y: number };
/** `order[i]`: which of the given items became the i-th virtual monitor. */
export type Settled = { virtuals: Virtual[]; rects: Rect[]; order: number[] };

function spot(to: Rect, own: Size, side: Side, align: Align): [number, number] {
  const along = (start: number, len: number, mine: number) => (align === "start" ? start : align === "center" ? start + Math.trunc((len - mine) / 2) : start + len - mine);
  switch (side) {
    case "left":
      return [to.x - own.width, along(to.y, to.height, own.height)];
    case "right":
      return [to.x + to.width, along(to.y, to.height, own.height)];
    case "above":
      return [along(to.x, to.width, own.width), to.y - own.height];
    default:
      return [along(to.x, to.width, own.width), to.y + to.height];
  }
}

/** The 12 snap points on one neighbour; `to` is the neighbour's number. */
function candidates(neighbour: Rect, to: number, own: Size): Candidate[] {
  return SIDES.flatMap((side) =>
    ALIGNS.map((align) => {
      const [x, y] = spot(neighbour, own, side, align);
      return { x, y, attach: { to, side, align } };
    }),
  );
}

/** The point on the neighbour's edge a snap point belongs to: what the scheme marks. */
export function anchor(neighbour: Rect, c: Candidate): { x: number; y: number } {
  const { side, align } = c.attach;
  const along = (start: number, len: number) => (align === "start" ? start : align === "center" ? start + len / 2 : start + len);
  if (side === "left" || side === "right") return { x: side === "left" ? neighbour.x : neighbour.x + neighbour.width, y: along(neighbour.y, neighbour.height) };
  return { x: along(neighbour.x, neighbour.width), y: side === "above" ? neighbour.y : neighbour.y + neighbour.height };
}

export function overlaps(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

const at = (c: { x: number; y: number }, own: Size): Rect => ({ x: c.x, y: c.y, width: own.width, height: own.height });
const clear = (r: Rect, others: Rect[]) => !others.some((o) => overlaps(r, o));

/** All free snap points on the given monitors (indexes of `rects`). */
export function freeCandidates(rects: Rect[], targets: number[], own: Size, others: Rect[]): Candidate[] {
  return targets.flatMap((t) => candidates(rects[t], t + 1, own)).filter((c) => clear(at(c, own), others));
}

export function nearest(list: Candidate[], x: number, y: number): Candidate | null {
  let best: Candidate | null = null;
  let bestDist = Infinity;
  for (const c of list) {
    const dist = (c.x - x) ** 2 + (c.y - y) ** 2;
    if (dist < bestDist) {
      best = c;
      bestDist = dist;
    }
  }
  return best;
}

/** What the attach rules say, without refusing overlaps: `settle` deals with those. */
export function looseRects(real: Rect[], virtuals: Virtual[]): Rect[] {
  const rects: Rect[] = real.map(({ x, y, width, height }) => ({ x, y, width, height }));
  for (const v of virtuals) {
    const [x, y] = spot(rects[v.attach.to - 1] ?? rects[0], v, v.attach.side, v.attach.align);
    rects.push(at({ x, y }, v));
  }
  return rects;
}

export const itemsOf = (real: Rect[], virtuals: Virtual[]): Item[] => {
  const rects = looseRects(real, virtuals);
  return virtuals.map((v, i) => ({ v, x: rects[real.length + i].x, y: rects[real.length + i].y }));
};

function snapOn(placed: Rect[], item: Item): Candidate | null {
  for (let t = 0; t < placed.length; t++) {
    const c = candidates(placed[t], t + 1, item.v).find((k) => k.x === item.x && k.y === item.y);
    if (c && clear(at(c, item.v), placed)) return c;
  }
  return null;
}

/**
 * Desktop positions → the core's rule. A monitor that stands on a snap point of an already placed one stays
 * where it is; one that lost its neighbour or overlaps goes to the nearest free snap point. The core only attaches
 * to smaller numbers, so a monitor attached to one with a bigger number takes the number after it.
 */
export function settle(real: Rect[], items: Item[]): Settled {
  const rects: Rect[] = real.map(({ x, y, width, height }) => ({ x, y, width, height }));
  const left = items.map((item, i) => ({ item, i }));
  const virtuals: Virtual[] = [];
  const order: number[] = [];
  const put = (entry: (typeof left)[number], c: Candidate) => {
    rects.push(at(c, entry.item.v));
    virtuals.push({ ...entry.item.v, attach: c.attach });
    order.push(entry.i);
    left.splice(left.indexOf(entry), 1);
  };
  while (left.length) {
    const exact = left.map((entry) => ({ entry, c: snapOn(rects, entry.item) })).find((e) => e.c);
    if (exact?.c) {
      put(exact.entry, exact.c);
      continue;
    }
    const entry = left[0];
    const all = freeCandidates(rects, rects.map((_, t) => t), entry.item.v, rects);
    put(entry, nearest(all, entry.item.x, entry.item.y)!);
  }
  return { virtuals, rects, order };
}

/**
 * Which monitors can hold the dragged one: reached from the real ones without it. What was attached
 * only to the dragged monitor is about to move itself, so it cannot be a neighbour.
 */
export function holders(rects: Rect[], realCount: number, dragged: number): number[] {
  const reached = new Set<number>();
  for (let i = 0; i < realCount; i++) reached.add(i);
  for (let grew = true; grew; ) {
    grew = false;
    rects.forEach((r, i) => {
      if (i === dragged || reached.has(i)) return;
      const touches = [...reached].some((t) => candidates(rects[t], t + 1, r).some((c) => c.x === r.x && c.y === r.y));
      if (touches) {
        reached.add(i);
        grew = true;
      }
    });
  }
  return [...reached];
}

/** A new monitor's first place: next to the last monitor, to the right if there is room. */
function firstFree(rects: Rect[], own: Size): Candidate {
  for (let t = rects.length - 1; t >= 0; t--) {
    const free = candidates(rects[t], t + 1, own).find((c) => clear(at(c, own), rects));
    if (free) return free;
  }
  throw new Error("no room next to any monitor");
}

export const sameRect = (a: Rect, b: Rect) => a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

/** The same picture: the same places, sizes and scales. How it is attached does not matter. */
export function sameLayout(real: Rect[], a: Virtual[], b: Virtual[]): boolean {
  if (a.length !== b.length) return false;
  const ra = looseRects(real, a);
  const rb = looseRects(real, b);
  return ra.every((r, i) => sameRect(r, rb[i])) && a.every((v, i) => v.scale === b[i].scale);
}

/** A monitor added with «+»: Full HD at 100 %. */
const NEW_MONITOR = { width: 1920, height: 1080, scale: 100 };

/** What Lumen's own monitors are called (Lumen VDD's EDID says so since 0.2.3; the owner's wish, like ParsecVDA). */
export const LUMEN_MONITOR = "LumenVDA";

/** «+»: a new monitor next to the last one; it is the last in the list. */
export function withAdded(rects: Rect[], virtuals: Virtual[]): Virtual[] {
  return [...virtuals, { ...NEW_MONITOR, attach: firstFree(rects, NEW_MONITOR).attach }];
}

/** The cross on virtual monitor `i`: the others settle without it. */
export function withRemoved(real: Rect[], virtuals: Virtual[], i: number): Virtual[] {
  return settle(real, itemsOf(real, virtuals).filter((_, j) => j !== i)).virtuals;
}

/**
 * A new size or scale of virtual monitor `i`: it keeps its neighbour; what no longer fits moves to the nearest
 * snap point. `picked`: its number among all the monitors from 0, after the numbers may have changed.
 */
export function withChanged(real: Rect[], virtuals: Virtual[], i: number, patch: Partial<Virtual>): { virtuals: Virtual[]; picked: number } {
  const done = settle(real, itemsOf(real, virtuals.map((v, j) => (j === i ? { ...v, ...patch } : v))));
  return { virtuals: done.virtuals, picked: real.length + done.order.indexOf(i) };
}
