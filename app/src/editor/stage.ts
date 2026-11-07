/** Geometry of the video editor's stage: what the export will look like,
 * crop frames locked to a ratio, resizing annotations, and stacking time
 * bars without overlap. */

import { type Point, type Rect, type Shape, FREEHAND, LINEAR, badgeRadius, bendTowards, bounds, curvePoints, fontSize } from "./shapes";

/** How the source maps onto the exported frame: the `crop` area of the
 * source is drawn at `area` of an `out`-sized frame. */
export interface Layout {
  out: { w: number; h: number };
  crop: Rect;
  area: Rect;
  /** A background fills the frame around the video. */
  background: boolean;
}

/** Same rule as the exporter's canvas (crates/kadra-record/src/compose.rs):
 * a frame of the requested ratio around a `w` × `h` video, with a margin,
 * no larger than the source on its long side. */
export function fitLayout(crop: Rect, ratio: [number, number], padding: number): Layout {
  const { w, h } = crop;
  const r = Math.max(1, ratio[0]) / Math.max(1, ratio[1]);
  const pad = Math.max(0, Math.min(0.3, padding));
  let cw = w / h > r ? w : h * r;
  let ch = w / h > r ? w / r : h;
  cw /= 1 - 2 * pad;
  ch /= 1 - 2 * pad;
  const limit = Math.min(Math.max(w, h), 4096) / Math.max(cw, ch);
  if (limit < 1) {
    cw *= limit;
    ch *= limit;
  }
  cw = (Math.floor(cw) + 1) & ~1;
  ch = (Math.floor(ch) + 1) & ~1;
  const fit = Math.min((cw * (1 - 2 * pad)) / w, (ch * (1 - 2 * pad)) / h);
  const vw = Math.floor(w * fit);
  const vh = Math.floor(h * fit);
  return {
    out: { w: cw, h: ch },
    crop,
    area: { x: Math.floor((cw - vw) / 2), y: Math.floor((ch - vh) / 2), w: vw, h: vh },
    background: true,
  };
}

/** The crop shown as it is, without a background. */
export const plainLayout = (crop: Rect): Layout => ({
  out: { w: crop.w, h: crop.h },
  crop,
  area: { x: 0, y: 0, w: crop.w, h: crop.h },
  background: false,
});

/** The largest area of `ratio` inside `within`, centred on `centre`. */
export function ratioCrop(within: Rect, ratio: [number, number], centre?: Point): Rect {
  const r = ratio[0] / ratio[1];
  let w = within.w;
  let h = w / r;
  if (h > within.h) {
    h = within.h;
    w = h * r;
  }
  const c = centre ?? { x: within.x + within.w / 2, y: within.y + within.h / 2 };
  return clampRect({ x: c.x - w / 2, y: c.y - h / 2, w, h }, within);
}

/** `r` moved (not resized) to fit inside `within`. */
export function clampRect(r: Rect, within: Rect): Rect {
  const w = Math.min(r.w, within.w);
  const h = Math.min(r.h, within.h);
  return {
    x: Math.max(within.x, Math.min(within.x + within.w - w, r.x)),
    y: Math.max(within.y, Math.min(within.y + within.h - h, r.y)),
    w,
    h,
  };
}

/** Corners of a rectangle: top-left, top-right, bottom-right, bottom-left. */
export const corners = (r: Rect): Point[] => [
  { x: r.x, y: r.y },
  { x: r.x + r.w, y: r.y },
  { x: r.x + r.w, y: r.y + r.h },
  { x: r.x, y: r.y + r.h },
];

/** The rectangle between a fixed corner and a dragged one, optionally
 * keeping `ratio` (width / height), inside `within`. */
export function dragCorner(anchor: Point, to: Point, within: Rect, ratio: number | null, min = 16): Rect {
  const x = Math.max(within.x, Math.min(within.x + within.w, to.x));
  const y = Math.max(within.y, Math.min(within.y + within.h, to.y));
  let w = Math.max(min, Math.abs(x - anchor.x));
  let h = Math.max(min, Math.abs(y - anchor.y));
  if (ratio) {
    // Follow the larger movement, then shrink to stay inside.
    if (w / h > ratio) h = w / ratio;
    else w = h * ratio;
    const roomX = to.x >= anchor.x ? within.x + within.w - anchor.x : anchor.x - within.x;
    const roomY = to.y >= anchor.y ? within.y + within.h - anchor.y : anchor.y - within.y;
    const shrink = Math.min(1, roomX / w, roomY / h);
    w *= shrink;
    h *= shrink;
  }
  return {
    x: to.x >= anchor.x ? anchor.x : anchor.x - w,
    y: to.y >= anchor.y ? anchor.y : anchor.y - h,
    w,
    h,
  };
}

/** Handles of a selected annotation: the ends of lines and arrows (and the
 * middle of curved ones), the corners of everything else. */
export function shapeHandles(ctx: CanvasRenderingContext2D, s: Shape): Point[] {
  if (s.kind === "curve") return [{ x: s.x1, y: s.y1 }, { x: s.x2, y: s.y2 }, curvePoints(s).apex];
  if (LINEAR.includes(s.kind)) return [{ x: s.x1, y: s.y1 }, { x: s.x2, y: s.y2 }];
  return corners(bounds(ctx, s));
}

/** `original` resized by dragging handle `handle` to `to`. Texts and
 * numbered steps grow uniformly (their font follows); boxes and drawings
 * stretch; lines move the dragged end. */
export function resizeShape(
  ctx: CanvasRenderingContext2D,
  original: Shape,
  handle: number,
  to: Point,
): Shape {
  const s: Shape = structuredClone(original);
  if (LINEAR.includes(s.kind)) {
    if (handle === 2) s.bend = bendTowards(s, to);
    else if (handle === 0) [s.x1, s.y1] = [to.x, to.y];
    else [s.x2, s.y2] = [to.x, to.y];
    return s;
  }
  const b = bounds(ctx, original);
  const cs = corners(b);
  const anchor = cs[(handle + 2) % 4] as Point;
  const grabbed = cs[handle] as Point;
  const fx = Math.max(0.05, Math.abs(to.x - anchor.x) / Math.max(1, Math.abs(grabbed.x - anchor.x)));
  const fy = Math.max(0.05, Math.abs(to.y - anchor.y) / Math.max(1, Math.abs(grabbed.y - anchor.y)));

  if (s.kind === "text" || s.kind === "number") {
    const f = Math.max(fx, fy);
    if (s.kind === "text") s.size = Math.max(0.2, (fontSize(original.size) * f - 16) / 6);
    else s.size = Math.max(0.2, (badgeRadius(original.size) * f - 11) / 2);
  } else {
    const map = (p: Point) => ({ x: anchor.x + (p.x - anchor.x) * fx, y: anchor.y + (p.y - anchor.y) * fy });
    const a = map({ x: s.x1, y: s.y1 });
    const c = map({ x: s.x2, y: s.y2 });
    [s.x1, s.y1, s.x2, s.y2] = [a.x, a.y, c.x, c.y];
    if (FREEHAND.includes(s.kind) && s.points) s.points = s.points.map(map);
  }
  // Keep the opposite corner where it was.
  const after = corners(bounds(ctx, s))[(handle + 2) % 4] as Point;
  const dx = anchor.x - after.x;
  const dy = anchor.y - after.y;
  s.x1 += dx;
  s.x2 += dx;
  s.y1 += dy;
  s.y2 += dy;
  if (s.points) s.points = s.points.map((p) => ({ x: p.x + dx, y: p.y + dy }));
  return s;
}

/** Rows for time bars so that overlapping ones never share a row. */
export function packRows(items: { id: number; start: number; end: number }[]): { row: Map<number, number>; count: number } {
  const ends: number[] = [];
  const row = new Map<number, number>();
  for (const it of [...items].sort((a, b) => a.start - b.start || a.id - b.id)) {
    let r = ends.findIndex((e) => e <= it.start + 1e-6);
    if (r < 0) {
      r = ends.length;
      ends.push(0);
    }
    ends[r] = it.end;
    row.set(it.id, r);
  }
  return { row, count: Math.max(1, ends.length) };
}

/** The soft click mixed into exports, played in the preview. */
export function playClick(audio: AudioContext, volume = 0.35) {
  const rate = audio.sampleRate;
  const length = Math.floor(rate * 0.09);
  const buffer = audio.createBuffer(1, length, rate);
  const data = buffer.getChannelData(0);
  let phase = 0;
  for (let i = 0; i < length; i++) {
    const t = i / rate;
    const pitch = 240 + 240 * Math.exp(-t * 60);
    phase += (2 * Math.PI * pitch) / rate;
    const envelope = Math.min(1, t / 0.003) * Math.exp(-t * 55);
    data[i] = envelope * (0.85 * Math.sin(phase) + 0.15 * Math.sin(2 * phase)) * volume;
  }
  const node = audio.createBufferSource();
  node.buffer = buffer;
  node.connect(audio.destination);
  node.start();
}
