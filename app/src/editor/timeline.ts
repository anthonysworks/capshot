/** Segments of the video editor: consecutive parts of the video, each kept
 * or removed. Together they always cover the whole video. */

export interface Segment {
  start: number;
  end: number;
  kept: boolean;
}

/** Shortest segment a split may create, in seconds. */
export const MIN_SEGMENT = 0.05;

export const whole = (duration: number): Segment[] => [{ start: 0, end: duration, kept: true }];

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

/** Removes everything before `t` (`before`) or from `t` on. */
export function trimAt(segments: Segment[], t: number, before: boolean): Segment[] {
  return split(segments, t).map((s) => ((before ? s.end <= t + 1e-9 : s.start >= t - 1e-9) ? { ...s, kept: false } : s));
}

export const toggle = (segments: Segment[], i: number): Segment[] =>
  segments.map((s, j) => (j === i ? { ...s, kept: !s.kept } : s));

/** Kept parts, adjacent ones merged, as `[start, end]` pairs. */
export function keptRanges(segments: Segment[]): [number, number][] {
  const out: [number, number][] = [];
  for (const s of segments) {
    if (!s.kept) continue;
    const last = out[out.length - 1];
    if (last && Math.abs(last[1] - s.start) < 1e-6) last[1] = s.end;
    else out.push([s.start, s.end]);
  }
  return out;
}

export const keptLength = (segments: Segment[]) =>
  keptRanges(segments).reduce((sum, [a, b]) => sum + (b - a), 0);

/** Where playback continues from `t`: `t` itself if it is kept, else the
 * start of the next kept part, or `null` after the last one. */
export function nextKept(segments: Segment[], t: number): number | null {
  for (const [a, b] of keptRanges(segments)) {
    if (t < a) return a;
    if (t < b) return t;
  }
  return null;
}

export function format(t: number) {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `${m}:${s.toFixed(2).padStart(5, "0")}`;
}
