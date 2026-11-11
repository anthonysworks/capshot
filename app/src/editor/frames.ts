/** Device frames around a picture or a video: a macOS or Windows window
 * (with its title), a MacBook, an iPhone. Shared by both editors; the
 * video editor sends what is drawn here to the exporter, so preview and
 * export look the same. */

import type { Rect } from "./shapes";

export type DeviceFrame = "none" | "macos" | "windows" | "macbook" | "iphone";

export const DEVICE_FRAMES: { value: DeviceFrame; label: string }[] = [
  { value: "none", label: "None" },
  { value: "macos", label: "macOS" },
  { value: "windows", label: "Windows" },
  { value: "macbook", label: "MacBook" },
  { value: "iphone", label: "iPhone" },
];

/** Frames drawn as windows have a title and a light or dark look. */
export const hasTitle = (f: DeviceFrame) => f === "macos" || f === "windows";
export const hasLook = (f: DeviceFrame) => f === "macos" || f === "windows";

export interface FrameStyle {
  kind: DeviceFrame;
  title: string;
  dark: boolean;
}

interface Insets {
  l: number;
  t: number;
  r: number;
  b: number;
}

/** Room the frame takes around a `w` × `h` screen, in the same unit. */
export function frameInsets(kind: DeviceFrame, w: number, h: number): Insets {
  const m = Math.max(w, h);
  const n = Math.min(w, h);
  switch (kind) {
    case "macos":
      return { l: 0, t: 0.03 * m, r: 0, b: 0 };
    case "windows":
      return { l: 0, t: 0.032 * m, r: 0, b: 0 };
    case "macbook": {
      const bezel = 0.024 * m;
      const lip = 0.07 * m;
      return { l: bezel + lip, t: bezel * 1.25, r: bezel + lip, b: bezel * 1.6 + 0.03 * m };
    }
    case "iphone": {
      const side = 0.05 * n + 0.012 * n;
      return { l: side, t: side, r: side, b: side };
    }
    default:
      return { l: 0, t: 0, r: 0, b: 0 };
  }
}

/** Where the device sits and where its screen is, on a canvas of the given
 * ratio around a `crop`-sized picture with `padding` (share of the canvas)
 * around the device. Same canvas rule as `fitLayout`. */
export function frameLayout(
  crop: { w: number; h: number },
  ratio: [number, number] | null,
  padding: number,
  kind: DeviceFrame,
): { out: { w: number; h: number }; device: Rect; screen: Rect } {
  const { w, h } = crop;
  const ins = frameInsets(kind, w, h);
  const W = w + ins.l + ins.r;
  const H = h + ins.t + ins.b;
  const r = ratio ? Math.max(1, ratio[0]) / Math.max(1, ratio[1]) : W / H;
  const pad = Math.max(0, Math.min(0.3, padding));
  let cw = W / H > r ? W : H * r;
  let ch = W / H > r ? W / r : H;
  cw /= 1 - 2 * pad;
  ch /= 1 - 2 * pad;
  const limit = Math.min(Math.max(W, H), 4096) / Math.max(cw, ch);
  if (limit < 1) {
    cw *= limit;
    ch *= limit;
  }
  cw = (Math.floor(cw) + 1) & ~1;
  ch = (Math.floor(ch) + 1) & ~1;
  const fit = Math.min((cw * (1 - 2 * pad)) / W, (ch * (1 - 2 * pad)) / H);
  const dw = W * fit;
  const dh = H * fit;
  const dx = (cw - dw) / 2;
  const dy = (ch - dh) / 2;
  const screen = {
    x: Math.round(dx + ins.l * fit),
    y: Math.round(dy + ins.t * fit),
    w: Math.floor(w * fit),
    h: Math.floor(h * fit),
  };
  return { out: { w: cw, h: ch }, device: { x: dx, y: dy, w: dw, h: dh }, screen };
}

/** The outline of the screen: where the picture shows through. */
function screenPath(ctx: CanvasRenderingContext2D, kind: DeviceFrame, s: Rect) {
  const m = Math.max(s.w, s.h);
  const n = Math.min(s.w, s.h);
  ctx.beginPath();
  if (kind === "macos") ctx.roundRect(s.x, s.y, s.w, s.h, [0, 0, 0.0085 * m, 0.0085 * m]);
  else if (kind === "windows") ctx.roundRect(s.x, s.y, s.w, s.h, [0, 0, 0.006 * m, 0.006 * m]);
  else if (kind === "iphone") ctx.roundRect(s.x, s.y, s.w, s.h, 0.11 * n);
  else if (kind === "macbook") ctx.roundRect(s.x, s.y, s.w, s.h, [0.004 * m, 0.004 * m, 0, 0]);
  else ctx.rect(s.x, s.y, s.w, s.h);
}

/** Draws the device (and its shadow) around the screen `s`, under the
 * picture. `shadow` is 0–1. */
export function drawDevice(ctx: CanvasRenderingContext2D, style: FrameStyle, s: Rect, shadow: number) {
  const { kind } = style;
  const m = Math.max(s.w, s.h);
  const n = Math.min(s.w, s.h);
  const ins = frameInsets(kind, s.w, s.h);
  const shade = (blur: number, drop: number, draw: () => void) => {
    ctx.save();
    if (shadow > 0) {
      ctx.shadowColor = `rgba(0,0,0,${0.55 * Math.min(1, shadow)})`;
      ctx.shadowBlur = blur;
      ctx.shadowOffsetY = drop;
    }
    draw();
    ctx.restore();
  };

  if (kind === "macos" || kind === "windows") {
    const radius = (kind === "macos" ? 0.0085 : 0.006) * m;
    const bar = ins.t;
    const win = { x: s.x, y: s.y - bar, w: s.w, h: s.h + bar };
    const face = style.dark ? (kind === "macos" ? "#2b2b2e" : "#202020") : kind === "macos" ? "#ececee" : "#f3f3f3";
    shade(0.04 * m, 0.012 * m, () => {
      ctx.fillStyle = face;
      ctx.beginPath();
      ctx.roundRect(win.x, win.y, win.w, win.h, radius);
      ctx.fill();
    });
    // A hairline around the window and under the title bar.
    ctx.save();
    ctx.strokeStyle = style.dark ? "rgba(255,255,255,0.12)" : "rgba(0,0,0,0.14)";
    ctx.lineWidth = Math.max(1, 0.0008 * m);
    ctx.beginPath();
    ctx.roundRect(win.x, win.y, win.w, win.h, radius);
    ctx.stroke();
    ctx.restore();
    const cy = s.y - bar / 2;
    const text = style.dark ? "#e8e8ea" : "#2a2a2e";
    ctx.save();
    ctx.fillStyle = text;
    ctx.textBaseline = "middle";
    if (kind === "macos") {
      const r = bar * 0.21;
      ["#ff5f57", "#febc2e", "#28c840"].forEach((c, i) => {
        ctx.beginPath();
        ctx.arc(s.x + bar * 0.62 + i * r * 3.3, cy, r, 0, Math.PI * 2);
        ctx.fillStyle = c;
        ctx.fill();
      });
      ctx.fillStyle = text;
      ctx.font = `600 ${bar * 0.4}px "Inter Variable", system-ui, sans-serif`;
      ctx.textAlign = "center";
      ctx.fillText(clip(ctx, style.title, s.w * 0.6), s.x + s.w / 2, cy);
    } else {
      // App mark, title, then minimise, maximise and close.
      const icon = bar * 0.42;
      const ix = s.x + bar * 0.5;
      const g = ctx.createLinearGradient(ix, cy - icon / 2, ix + icon, cy + icon / 2);
      g.addColorStop(0, "#06b6fd");
      g.addColorStop(1, "#1b3af2");
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(ix + icon / 2, cy, icon / 2, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = text;
      ctx.font = `400 ${bar * 0.36}px "Segoe UI Variable", "Segoe UI", "Inter Variable", sans-serif`;
      ctx.textAlign = "left";
      ctx.fillText(clip(ctx, style.title, s.w * 0.55), ix + icon + bar * 0.35, cy);
      const bw = bar * 1.45;
      const g2 = bar * 0.16;
      ctx.strokeStyle = text;
      ctx.lineWidth = Math.max(1, bar * 0.03);
      const right = s.x + s.w;
      // Close
      let bx = right - bw / 2;
      ctx.beginPath();
      ctx.moveTo(bx - g2, cy - g2);
      ctx.lineTo(bx + g2, cy + g2);
      ctx.moveTo(bx + g2, cy - g2);
      ctx.lineTo(bx - g2, cy + g2);
      ctx.stroke();
      // Maximise
      bx -= bw;
      ctx.beginPath();
      ctx.roundRect(bx - g2, cy - g2, 2 * g2, 2 * g2, g2 * 0.25);
      ctx.stroke();
      // Minimise
      bx -= bw;
      ctx.beginPath();
      ctx.moveTo(bx - g2, cy);
      ctx.lineTo(bx + g2, cy);
      ctx.stroke();
    }
    ctx.restore();
    return;
  }

  if (kind === "macbook") {
    const bezel = 0.024 * m;
    const lid = { x: s.x - bezel, y: s.y - bezel * 1.25, w: s.w + 2 * bezel, h: s.h + bezel * 2.85 };
    const lidRadius = 0.022 * m;
    const baseH = 0.03 * m;
    const base = { x: lid.x - 0.07 * m, y: lid.y + lid.h - 0.002 * m, w: lid.w + 0.14 * m, h: baseH };
    shade(0.05 * m, 0.02 * m, () => {
      ctx.fillStyle = "#1a1a1c";
      ctx.beginPath();
      ctx.roundRect(lid.x, lid.y, lid.w, lid.h, [lidRadius, lidRadius, 0, 0]);
      ctx.fill();
      ctx.beginPath();
      ctx.roundRect(base.x, base.y, base.w, base.h, [0, 0, baseH * 0.9, baseH * 0.9]);
      ctx.fill();
    });
    // Aluminium rim of the lid, black glass inside.
    ctx.fillStyle = "#c9cacd";
    ctx.beginPath();
    ctx.roundRect(lid.x, lid.y, lid.w, lid.h, [lidRadius, lidRadius, 0, 0]);
    ctx.fill();
    const rim = 0.0025 * m;
    ctx.fillStyle = "#0c0c0e";
    ctx.beginPath();
    ctx.roundRect(lid.x + rim, lid.y + rim, lid.w - 2 * rim, lid.h - rim, [lidRadius - rim, lidRadius - rim, 0, 0]);
    ctx.fill();
    // Camera.
    ctx.fillStyle = "#2a2d36";
    ctx.beginPath();
    ctx.arc(s.x + s.w / 2, lid.y + bezel * 0.62, bezel * 0.14, 0, Math.PI * 2);
    ctx.fill();
    // Base: brushed aluminium, a groove to open it.
    const g = ctx.createLinearGradient(0, base.y, 0, base.y + base.h);
    g.addColorStop(0, "#e4e5e7");
    g.addColorStop(0.45, "#c6c7ca");
    g.addColorStop(1, "#8e9095");
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.roundRect(base.x, base.y, base.w, base.h, [0, 0, baseH * 0.9, baseH * 0.9]);
    ctx.fill();
    ctx.fillStyle = "rgba(0,0,0,0.18)";
    const notch = base.w * 0.12;
    ctx.beginPath();
    ctx.roundRect(base.x + (base.w - notch) / 2, base.y, notch, baseH * 0.32, [0, 0, baseH * 0.3, baseH * 0.3]);
    ctx.fill();
    return;
  }

  if (kind === "iphone") {
    const bezel = 0.05 * n;
    const frame = 0.012 * n;
    const body = { x: s.x - bezel, y: s.y - bezel, w: s.w + 2 * bezel, h: s.h + 2 * bezel };
    const outer = 0.11 * n + bezel;
    const portrait = s.h >= s.w;
    // Side buttons, then the titanium band and the black glass.
    ctx.fillStyle = "#8f9094";
    const button = (along: number, length: number, side: "a" | "b") => {
      const t = frame * 0.9;
      if (portrait) {
        const x = side === "a" ? body.x - t : body.x + body.w;
        ctx.beginPath();
        ctx.roundRect(x, body.y + body.h * along, t, body.h * length, t / 2);
        ctx.fill();
      } else {
        const y = side === "a" ? body.y - t : body.y + body.h;
        ctx.beginPath();
        ctx.roundRect(body.x + body.w * along, y, body.w * length, t, t / 2);
        ctx.fill();
      }
    };
    button(0.17, 0.05, "a");
    button(0.25, 0.09, "a");
    button(0.36, 0.09, "a");
    button(0.28, 0.14, "b");
    shade(0.05 * n, 0.02 * n, () => {
      const g = ctx.createLinearGradient(body.x, body.y, body.x + body.w, body.y + body.h);
      g.addColorStop(0, "#a7a8ac");
      g.addColorStop(0.5, "#6e6f73");
      g.addColorStop(1, "#a2a3a7");
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.roundRect(body.x, body.y, body.w, body.h, outer);
      ctx.fill();
    });
    ctx.fillStyle = "#050506";
    ctx.beginPath();
    ctx.roundRect(body.x + frame * 0.45, body.y + frame * 0.45, body.w - frame * 0.9, body.h - frame * 0.9, outer - frame * 0.45);
    ctx.fill();
  }
}

/** Draws what goes over the picture: everything outside the screen again
 * (so its corners look round), and the iPhone's camera island. `under` is
 * the canvas drawn by `drawDevice` (and the background). */
export function drawFront(ctx: CanvasRenderingContext2D, kind: DeviceFrame, s: Rect, under: CanvasImageSource) {
  ctx.save();
  ctx.drawImage(under, 0, 0);
  ctx.globalCompositeOperation = "destination-out";
  screenPath(ctx, kind, s);
  ctx.fill();
  ctx.restore();
  if (kind === "iphone") {
    const n = Math.min(s.w, s.h);
    const long = 0.27 * n;
    const short = 0.078 * n;
    const gap = 0.035 * n;
    ctx.fillStyle = "#000";
    ctx.beginPath();
    if (s.h >= s.w) ctx.roundRect(s.x + (s.w - long) / 2, s.y + gap, long, short, short / 2);
    else ctx.roundRect(s.x + gap, s.y + (s.h - long) / 2, short, long, short / 2);
    ctx.fill();
  }
}

/** `text` shortened with an ellipsis to fit `width`. */
function clip(ctx: CanvasRenderingContext2D, text: string, width: number): string {
  if (ctx.measureText(text).width <= width) return text;
  let t = text;
  while (t.length > 1 && ctx.measureText(`${t}…`).width > width) t = t.slice(0, -1);
  return `${t}…`;
}

/** The backdrop (background and device) and the front, at `out` size. */
export function renderFrame(
  out: { w: number; h: number },
  screen: Rect,
  style: FrameStyle,
  shadow: number,
  paintBackground: (ctx: CanvasRenderingContext2D) => void,
): { under: HTMLCanvasElement; over: HTMLCanvasElement } {
  const under = document.createElement("canvas");
  under.width = out.w;
  under.height = out.h;
  const u = under.getContext("2d");
  const over = document.createElement("canvas");
  over.width = out.w;
  over.height = out.h;
  const o = over.getContext("2d");
  if (!u || !o) return { under, over };
  paintBackground(u);
  drawDevice(u, style, screen, shadow);
  drawFront(o, style.kind, screen, under);
  return { under, over };
}
