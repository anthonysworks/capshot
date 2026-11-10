/** Camera zoom: between two times the video is shown enlarged around a
 * point (fixed, or the cursor), easing in and out. Same view as
 * crates/capshot-record/src/zoom.rs. */

import type { CursorTrack, ZoomRegion } from "../lib/api";
import { cursorAt, smoothTrack } from "./cursors";
import type { Rect } from "./shapes";

/** Longest time a zoom takes to go in or out, in seconds. */
const EASE = 0.7;
/** Stiffness of the camera following the cursor. */
export const CAMERA_STIFFNESS = 3.5;
export const ZOOM_SCALES = [1.5, 2, 2.5, 3];
/** Length of a zoom added by hand, in seconds. */
export const ZOOM_LENGTH = 3;

const ease = (u: number) => {
  const v = Math.max(0, Math.min(1, u));
  return v < 0.5 ? 4 * v * v * v : 1 - (-2 * v + 2) ** 3 / 2;
};

/** How far zoomed in at `t`: 0 outside, 1 in the middle. */
export function zoomWeight(r: ZoomRegion, t: number): number {
  if (t <= r.start || t >= r.end) return 0;
  const ramp = Math.max(1e-3, Math.min(EASE, (r.end - r.start) / 2));
  return Math.min(ease((t - r.start) / ramp), ease((r.end - t) / ramp));
}

/** The camera track (the cursor slowed down), when a zoom follows it. */
export function cameraTrack(regions: ZoomRegion[], track: CursorTrack | null): CursorTrack | null {
  return track && regions.some((r) => r.focus === null) ? smoothTrack(track, CAMERA_STIFFNESS) : null;
}

/** The part of `crop` (video pixels) in view at `t`. */
export function zoomView(regions: ZoomRegion[], camera: CursorTrack | null, crop: Rect, t: number): Rect {
  let best: ZoomRegion | null = null;
  let weight = 0;
  for (const r of regions) {
    if (r.end <= r.start || r.scale <= 1) continue;
    const w = zoomWeight(r, t);
    if (w > weight) [best, weight] = [r, w];
  }
  if (!best) return crop;
  const scale = 1 + (Math.max(1, Math.min(6, best.scale)) - 1) * weight;
  const w = crop.w / scale;
  const h = crop.h / scale;
  const at = best.focus ? { x: best.focus[0], y: best.focus[1] } : camera ? cursorAt(camera, t) : null;
  const fx = at?.x ?? crop.x + crop.w / 2;
  const fy = at?.y ?? crop.y + crop.h / 2;
  return {
    x: Math.max(crop.x, Math.min(crop.x + crop.w - w, fx - w / 2)),
    y: Math.max(crop.y, Math.min(crop.y + crop.h - h, fy - h / 2)),
    w,
    h,
  };
}

/** Zooms on the clicks: clicks close in time share one zoom, which starts
 * a little before the first and ends a while after the last. */
export function autoZooms(track: CursorTrack, duration: number, scale = 2): ZoomRegion[] {
  const out: ZoomRegion[] = [];
  const clicks = track.clicks.filter((c) => !c.right).map((c) => c.t).sort((a, b) => a - b);
  for (const t of clicks) {
    const last = out.at(-1);
    if (last && t - 0.6 < last.end + 0.8) last.end = Math.min(duration, t + 1.6);
    else out.push({ start: Math.max(0, t - 0.6), end: Math.min(duration, t + 1.6), scale, focus: null });
  }
  return out.filter((r) => r.end - r.start > 0.5);
}
