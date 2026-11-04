/** Cursors offered in the video editor, drawn on a canvas so that the
 * preview and the export look the same. */

import type { CursorTrack } from "../lib/api";

export type CursorStyle = "original" | "light" | "dark" | "dot";

export const CURSOR_STYLES: { value: CursorStyle; label: string }[] = [
  { value: "original", label: "As recorded" },
  { value: "light", label: "White arrow" },
  { value: "dark", label: "Black arrow" },
  { value: "dot", label: "Dot" },
];

/** Base size of the replacement cursors, in video pixels (scaled after). */
const SIZE = 32;

/** The arrow outline, for a 32 px cursor with its tip at (2, 2). */
function arrowPath(ctx: CanvasRenderingContext2D) {
  ctx.beginPath();
  ctx.moveTo(2, 2);
  ctx.lineTo(2, 24);
  ctx.lineTo(7.5, 19);
  ctx.lineTo(11.5, 28);
  ctx.lineTo(15, 26.5);
  ctx.lineTo(11, 17.5);
  ctx.lineTo(18.5, 17.5);
  ctx.closePath();
}

/** Draws a replacement cursor; returns its canvas and hot spot. */
export function drawCursor(style: Exclude<CursorStyle, "original">, color: string): { canvas: HTMLCanvasElement; hotspot: [number, number] } {
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const ctx = canvas.getContext("2d");
  if (!ctx) return { canvas, hotspot: [0, 0] };
  ctx.lineJoin = "round";
  if (style === "dot") {
    ctx.beginPath();
    ctx.arc(16, 16, 9, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.globalAlpha = 0.9;
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#fff";
    ctx.stroke();
    return { canvas, hotspot: [16, 16] };
  }
  arrowPath(ctx);
  ctx.shadowColor = "rgba(0,0,0,0.35)";
  ctx.shadowBlur = 3;
  ctx.shadowOffsetY = 1;
  ctx.fillStyle = style === "light" ? "#fff" : "#111";
  ctx.fill();
  ctx.shadowColor = "transparent";
  ctx.lineWidth = 1.6;
  ctx.strokeStyle = style === "light" ? "#111" : "#fff";
  ctx.stroke();
  return { canvas, hotspot: [2, 2] };
}

/** Images of the recorded cursor shapes. */
export async function loadShapes(track: CursorTrack): Promise<{ image: HTMLImageElement; hotspot: [number, number] }[]> {
  return Promise.all(
    track.shapes.map(
      (shape) =>
        new Promise<{ image: HTMLImageElement; hotspot: [number, number] }>((resolve) => {
          const image = new Image();
          image.onload = () => resolve({ image, hotspot: shape.hotspot });
          image.onerror = () => resolve({ image, hotspot: shape.hotspot });
          image.src = `data:image/png;base64,${shape.png}`;
        }),
    ),
  );
}

/** Cursor position at `t` (interpolated), or `null` when hidden. */
export function cursorAt(track: CursorTrack, t: number): { x: number; y: number; shape: number } | null {
  const samples = track.samples;
  let lo = 0;
  let hi = samples.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((samples[mid]?.t ?? 0) <= t) lo = mid + 1;
    else hi = mid;
  }
  const before = samples[lo - 1];
  if (!before || !before.visible) return null;
  const after = samples[lo];
  if (after && after.visible && after.t > before.t) {
    const k = (t - before.t) / (after.t - before.t);
    return { x: before.x + (after.x - before.x) * k, y: before.y + (after.y - before.y) * k, shape: before.shape };
  }
  return { x: before.x, y: before.y, shape: before.shape };
}

/** How long a click mark lasts, as in the export. */
export const CLICK_MARK = 0.45;
