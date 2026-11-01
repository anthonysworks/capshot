/** Annotations of the image editor and how they are drawn. All coordinates
 * are in image pixels. */

export type ShapeKind = "arrow" | "rect" | "ellipse" | "highlight" | "text" | "number" | "pixelate";

export interface Shape {
  id: number;
  kind: ShapeKind;
  color: string;
  /** Stroke width (text: font scale). */
  size: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  text?: string;
  n?: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export const normalize = (s: { x1: number; y1: number; x2: number; y2: number }): Rect => ({
  x: Math.min(s.x1, s.x2),
  y: Math.min(s.y1, s.y2),
  w: Math.abs(s.x2 - s.x1),
  h: Math.abs(s.y2 - s.y1),
});

/** Pixel block size of the pixelate tool: large enough that text cannot
 * be recovered. */
const BLOCK = 12;

export const fontSize = (size: number) => 16 + size * 6;
export const badgeRadius = (size: number) => 11 + size * 2;

/** Text colour that stays readable on `color`. */
function contrast(color: string): string {
  const hex = color.replace("#", "");
  const [r = 0, g = 0, b = 0] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#111" : "#fff";
}

/** Bounding box, used for selection and hit testing. */
export function bounds(ctx: CanvasRenderingContext2D, s: Shape): Rect {
  if (s.kind === "text") {
    ctx.font = `600 ${fontSize(s.size)}px "Segoe UI Variable Text", "Segoe UI", sans-serif`;
    const lines = (s.text ?? "").split("\n");
    const w = Math.max(...lines.map((l) => ctx.measureText(l).width), 4);
    return { x: s.x1, y: s.y1, w, h: lines.length * fontSize(s.size) * 1.25 };
  }
  if (s.kind === "number") {
    const r = badgeRadius(s.size);
    return { x: s.x1 - r, y: s.y1 - r, w: 2 * r, h: 2 * r };
  }
  const r = normalize(s);
  const pad = s.size / 2;
  return { x: r.x - pad, y: r.y - pad, w: r.w + 2 * pad, h: r.h + 2 * pad };
}

export function hit(ctx: CanvasRenderingContext2D, s: Shape, x: number, y: number, slack: number) {
  const b = bounds(ctx, s);
  return x >= b.x - slack && x <= b.x + b.w + slack && y >= b.y - slack && y <= b.y + b.h + slack;
}

function pixelate(ctx: CanvasRenderingContext2D, image: CanvasImageSource, r: Rect, scratch: HTMLCanvasElement) {
  const w = Math.max(1, Math.ceil(r.w / BLOCK));
  const h = Math.max(1, Math.ceil(r.h / BLOCK));
  scratch.width = w;
  scratch.height = h;
  const small = scratch.getContext("2d");
  if (!small) return;
  small.imageSmoothingEnabled = true;
  small.drawImage(image, r.x, r.y, r.w, r.h, 0, 0, w, h);
  ctx.save();
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(scratch, 0, 0, w, h, r.x, r.y, w * BLOCK, h * BLOCK);
  ctx.restore();
}

function arrow(ctx: CanvasRenderingContext2D, s: Shape) {
  const angle = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
  const length = Math.hypot(s.x2 - s.x1, s.y2 - s.y1);
  const head = Math.min(length * 0.6, 10 + s.size * 3);
  // The shaft stops inside the head so its end does not poke out.
  const shaft = Math.max(0, length - head * 0.6);
  ctx.beginPath();
  ctx.moveTo(s.x1, s.y1);
  ctx.lineTo(s.x1 + Math.cos(angle) * shaft, s.y1 + Math.sin(angle) * shaft);
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(s.x2, s.y2);
  for (const side of [-1, 1]) {
    const a = angle + Math.PI - side * 0.45;
    ctx.lineTo(s.x2 + Math.cos(a) * head, s.y2 + Math.sin(a) * head);
  }
  ctx.closePath();
  ctx.fill();
}

/** Draws the image, then the annotations. */
export function render(
  ctx: CanvasRenderingContext2D,
  image: CanvasImageSource,
  shapes: Shape[],
  scratch: HTMLCanvasElement,
) {
  ctx.drawImage(image, 0, 0);
  // Pixelation hides the original pixels, under every annotation.
  for (const s of shapes) {
    if (s.kind === "pixelate") pixelate(ctx, image, normalize(s), scratch);
  }
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const s of shapes) {
    ctx.save();
    ctx.strokeStyle = s.color;
    ctx.fillStyle = s.color;
    ctx.lineWidth = s.size;
    const r = normalize(s);
    switch (s.kind) {
      case "arrow":
        ctx.shadowColor = "rgba(0,0,0,0.35)";
        ctx.shadowBlur = s.size;
        arrow(ctx, s);
        break;
      case "rect":
        ctx.strokeRect(r.x, r.y, r.w, r.h);
        break;
      case "ellipse":
        ctx.beginPath();
        ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, r.h / 2, 0, 0, Math.PI * 2);
        ctx.stroke();
        break;
      case "highlight":
        ctx.globalAlpha = 0.38;
        ctx.globalCompositeOperation = "multiply";
        ctx.fillRect(r.x, r.y, r.w, r.h);
        break;
      case "text": {
        const size = fontSize(s.size);
        ctx.font = `600 ${size}px "Segoe UI Variable Text", "Segoe UI", sans-serif`;
        ctx.textBaseline = "top";
        ctx.lineWidth = Math.max(2, size / 7);
        ctx.strokeStyle = contrast(s.color);
        (s.text ?? "").split("\n").forEach((line, i) => {
          const y = s.y1 + i * size * 1.25;
          ctx.strokeText(line, s.x1, y);
          ctx.fillText(line, s.x1, y);
        });
        break;
      }
      case "number": {
        const radius = badgeRadius(s.size);
        ctx.shadowColor = "rgba(0,0,0,0.35)";
        ctx.shadowBlur = 4;
        ctx.beginPath();
        ctx.arc(s.x1, s.y1, radius, 0, Math.PI * 2);
        ctx.fill();
        ctx.shadowBlur = 0;
        ctx.fillStyle = contrast(s.color);
        ctx.font = `700 ${radius * 1.1}px "Segoe UI Variable Text", "Segoe UI", sans-serif`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(String(s.n ?? 1), s.x1, s.y1 + radius * 0.05);
        break;
      }
    }
    ctx.restore();
  }
}
