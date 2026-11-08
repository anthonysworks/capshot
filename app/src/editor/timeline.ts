/** Timeline of the video editor.
 *
 * Segments are consecutive parts of the video, each kept or removed and
 * played at its own speed; together they always cover the whole video.
 * The trim (in and out points) is separate: shortening the video from its
 * ends never forgets the cuts made inside, so dragging a trim handle back
 * restores exactly what was there. */

export interface Segment {
  start: number;
  end: number;
  kept: boolean;
  /** 2 plays twice as fast. */
  speed: number;
}

export interface Trim {
  start: number;
  end: number;
}

/** A part of the result: kept, inside the trim, at one speed. */
export interface Part {
  start: number;
  end: number;
  speed: number;
}

/** Shortest segment a split may create, in seconds. */
export const MIN_SEGMENT = 0.05;

export const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];

export const whole = (duration: number): Segment[] => [{ start: 0, end: duration, kept: true, speed: 1 }];

export const indexAt = (segments: Segment[], t: number) =>
  Math.max(
    0,
    segments.findIndex((s) => t >= s.start && t < s.end),
  );

/** Splits the segment under `t` in two (no-op too close to an edge). */
export function split(segments: Segment[], t: number): Segment[] {
  const i = segments.findIndex((s) => t > s.start + MIN_SEGMENT && t < s.end - MIN_SEGMENT);
  if (i < 0) return segments;
  const s = segments[i] as Segment;
  return [...segments.slice(0, i), { ...s, end: t }, { ...s, start: t }, ...segments.slice(i + 1)];
}

export const toggle = (segments: Segment[], i: number): Segment[] =>
  segments.map((s, j) => (j === i ? { ...s, kept: !s.kept } : s));

export const setSpeed = (segments: Segment[], i: number, speed: number): Segment[] =>
  segments.map((s, j) => (j === i ? { ...s, speed } : s));

/** What is exported: kept segments clipped to the trim, neighbours with
 * the same speed merged. */
export function parts(segments: Segment[], trim: Trim): Part[] {
  const out: Part[] = [];
  for (const s of segments) {
    if (!s.kept) continue;
    const start = Math.max(s.start, trim.start);
    const end = Math.min(s.end, trim.end);
    if (end - start < 1e-6) continue;
    const last = out[out.length - 1];
    if (last && Math.abs(last.end - start) < 1e-6 && last.speed === s.speed) last.end = end;
    else out.push({ start, end, speed: s.speed });
  }
  return out;
}

/** Length of the result, in seconds. */
export const outputLength = (ps: Part[]) => ps.reduce((sum, p) => sum + (p.end - p.start) / p.speed, 0);

export const partAt = (ps: Part[], t: number) => ps.find((p) => t >= p.start && t < p.end);

/** Where playback continues from `t`: `t` itself if it is kept, else the
 * start of the next part, or `null` after the last one. */
export function nextKept(ps: Part[], t: number): number | null {
  for (const p of ps) {
    if (t < p.start) return p.start;
    if (t < p.end) return t;
  }
  return null;
}

/** `t` moved to the nearest of `targets` closer than `reach`. */
export function snap(t: number, targets: number[], reach: number): number {
  let best = t;
  let distance = reach;
  for (const target of targets) {
    const d = Math.abs(target - t);
    if (d < distance) {
      distance = d;
      best = target;
    }
  }
  return best;
}

/** Ruler step (seconds) giving labels at least `minPx` apart. */
export function rulerStep(pxPerSecond: number, minPx = 70): number {
  const steps = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
  return steps.find((s) => s * pxPerSecond >= minPx) ?? 600;
}

export function format(t: number) {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `${m}:${s.toFixed(2).padStart(5, "0")}`;
}

/** Short ruler label: "0:05", "1:30", or "0:02.5" for sub-second steps. */
export function label(t: number, step: number) {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  const digits = step < 1 ? 1 : 0;
  return `${m}:${s.toFixed(digits).padStart(digits ? 4 : 2, "0")}`;
}

export const speedLabel = (speed: number) => `${speed}×`;

/** Removes the cut at the start of segment `i`, joining it with the one
 * before. The joined part is kept if either side was, at the speed of the
 * kept side (the left one when both were). */
export function join(segments: Segment[], i: number): Segment[] {
  const a = segments[i - 1];
  const b = segments[i];
  if (!a || !b) return segments;
  const kept = a.kept || b.kept;
  const speed = a.kept || !b.kept ? a.speed : b.speed;
  return [...segments.slice(0, i - 1), { start: a.start, end: b.end, kept, speed }, ...segments.slice(i + 1)];
}
