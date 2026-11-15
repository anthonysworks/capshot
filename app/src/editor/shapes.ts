/** Annotations of the image editor and how they are drawn. All coordinates
 * are in image pixels. */

export type ShapeKind =
  | "arrow"
  | "curve"
  | "line"
  | "rect"
  | "ellipse"
  | "pen"
  | "highlight"
  | "text"
  | "number"
  | "pixelate"
  | "blur"
  | "spotlight"
  | "image";

/** Rectangles and ellipses: outline only, tinted, or solid (redaction).
 * Text: no background, or a background box. */
/** `plain` (texts only): the text alone, with no outline and no box. */
export type Fill = "none" | "soft" | "solid" | "plain";

export interface Point {
  x: number;
  y: number;
}

export interface Shape {
  id: number;
  kind: ShapeKind;
  color: string;
  /** Stroke width (text: font scale). */
  size: number;
  fill: Fill;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  /** Freehand path (pen). */
  points?: Point[];
  text?: string;
  n?: number;
  /** Curved arrow: how far its middle sits from the straight line, as a
   * share of its length (negative: the other side). */
  bend?: number;
  /** Picture (kind `image`): its key in `pictures`. */
  picture?: number;
  /** 0–1; pictures only. */
  opacity?: number;
  /** Text style. */
  font?: TextFont;
  bold?: boolean;
  italic?: boolean;
}

export type TextFont = "sans" | "serif" | "mono" | "hand";

/** Typefaces offered for texts (all shipped with Windows). */
export const TEXT_FONTS: { value: TextFont; label: string; family: string }[] = [
  { value: "sans", label: "Sans", family: `"Segoe UI Variable Text", "Segoe UI", sans-serif` },
  { value: "serif", label: "Serif", family: `Georgia, "Times New Roman", serif` },
  { value: "mono", label: "Mono", family: `"Cascadia Mono", Consolas, monospace` },
  { value: "hand", label: "Hand", family: `"Segoe Print", "Comic Sans MS", cursive` },
];

/** Canvas font of a text shape. */
export function textFont(s: Shape, size = fontSize(s.size)): string {
  const family = TEXT_FONTS.find((f) => f.value === s.font)?.family ?? TEXT_FONTS[0]!.family;
  return `${s.italic ? "italic " : ""}${s.bold === false ? 400 : 600} ${size}px ${family}`;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Shapes defined by two corners, which can be resized from them. */
export const BOXED: ShapeKind[] = ["rect", "ellipse", "pixelate", "blur", "spotlight", "image"];

/** Pictures brought into an editor, by the id a picture shape refers to. */
export const pictures = new Map<number, ImageBitmap>();
let nextPicture = 1;
export function addPicture(bitmap: ImageBitmap): number {
  const id = nextPicture++;
  pictures.set(id, bitmap);
  return id;
}
/** Drawn freehand, as a list of points. */
export const FREEHAND: ShapeKind[] = ["pen", "highlight"];
/** Width of the highlighter stroke for a given size. */
export const markerWidth = (size: number) => size * 5;
/** Shapes defined by two end points. */
export const LINEAR: ShapeKind[] = ["arrow", "curve", "line"];

/** Bend of a new curved arrow. */
export const DEFAULT_BEND = 0.25;

/** Middle of a curved arrow (on the curve) and its quadratic control point. */
export function curvePoints(s: Shape): { apex: Point; control: Point } {
  const dx = s.x2 - s.x1;
  const dy = s.y2 - s.y1;
  const bend = s.bend ?? DEFAULT_BEND;
  const mx = (s.x1 + s.x2) / 2;
  const my = (s.y1 + s.y2) / 2;
  // Perpendicular to the line, scaled by its length.
  const apex = { x: mx - dy * bend, y: my + dx * bend };
  return { apex, control: { x: 2 * apex.x - mx, y: 2 * apex.y - my } };
}

/** Bend that puts the middle of a curved arrow at `p`. */
export function bendTowards(s: Shape, p: Point): number {
  const dx = s.x2 - s.x1;
  const dy = s.y2 - s.y1;
  const len2 = dx * dx + dy * dy;
  if (!len2) return 0;
  const bend = ((p.x - (s.x1 + s.x2) / 2) * -dy + (p.y - (s.y1 + s.y2) / 2) * dx) / len2;
  return Math.max(-1.5, Math.min(1.5, bend));
}

/** Bend of a curved arrow drawn along `path`: it goes through the point
 * farthest from the straight line, the side the hand went; a straight drag
 * gives a gentle arc bowing upwards. */
export function drawnBend(s: Shape, path: Point[]): number {
  const dx = s.x2 - s.x1;
  const dy = s.y2 - s.y1;
  const length = Math.hypot(dx, dy);
  let best = 0;
  for (const q of path) {
    const b = bendTowards(s, q);
    if (Math.abs(b) > Math.abs(best)) best = b;
  }
  if (length > 0 && Math.abs(best) > 0.06) return best;
  return dx !== 0 ? -Math.sign(dx) * DEFAULT_BEND : Math.sign(dy || 1) * DEFAULT_BEND;
}

/** Points along a curved arrow, for hit testing and bounds. */
function curveSamples(s: Shape, count = 24): Point[] {
  const { control: c } = curvePoints(s);
  return Array.from({ length: count + 1 }, (_, i) => {
    const t = i / count;
    const u = 1 - t;
    return { x: u * u * s.x1 + 2 * u * t * c.x + t * t * s.x2, y: u * u * s.y1 + 2 * u * t * c.y + t * t * s.y2 };
  });
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
const FONT = `"Segoe UI Variable Text", "Segoe UI", sans-serif`;

/** Text colour that stays readable on `color`. */
export function contrast(color: string): string {
  const hex = color.replace("#", "");
  const [r = 0, g = 0, b = 0] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#111" : "#fff";
}

function textBox(ctx: CanvasRenderingContext2D, s: Shape): Rect {
  const size = fontSize(s.size);
  ctx.font = textFont(s, size);
  const lines = (s.text ?? "").split("\n");
  const w = Math.max(...lines.map((l) => ctx.measureText(l).width), 4);
  const pad = s.fill === "none" || s.fill === "plain" ? 0 : size * 0.3;
  return { x: s.x1 - pad, y: s.y1 - pad, w: w + 2 * pad, h: lines.length * size * 1.25 + 2 * pad };
}

/** Bounding box, used for selection and hit testing. */
export function bounds(ctx: CanvasRenderingContext2D, s: Shape): Rect {
  if (s.kind === "text") return textBox(ctx, s);
  if (s.kind === "number") {
    const r = badgeRadius(s.size);
    return { x: s.x1 - r, y: s.y1 - r, w: 2 * r, h: 2 * r };
  }
  if (s.kind === "curve") {
    const pts = curveSamples(s);
    const pad = s.size + 6;
    const x = Math.min(...pts.map((q) => q.x)) - pad;
    const y = Math.min(...pts.map((q) => q.y)) - pad;
    return { x, y, w: Math.max(...pts.map((q) => q.x)) + pad - x, h: Math.max(...pts.map((q) => q.y)) + pad - y };
  }
  if (FREEHAND.includes(s.kind) && s.points?.length) {
    const xs = s.points.map((p) => p.x);
    const ys = s.points.map((p) => p.y);
    const pad = (s.kind === "highlight" ? markerWidth(s.size) : s.size) / 2;
    const x = Math.min(...xs) - pad;
    const y = Math.min(...ys) - pad;
    return { x, y, w: Math.max(...xs) + pad - x, h: Math.max(...ys) + pad - y };
  }
  const r = normalize(s);
  const pad = s.size / 2;
  return { x: r.x - pad, y: r.y - pad, w: r.w + 2 * pad, h: r.h + 2 * pad };
}

function distanceToSegment(p: Point, a: Point, b: Point) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = dx * dx + dy * dy;
  const t = len ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len)) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/** Whether `p` touches the shape (lines are hit near the stroke only). */
export function hit(ctx: CanvasRenderingContext2D, s: Shape, p: Point, slack: number) {
  if (s.kind === "curve") {
    const pts = curveSamples(s);
    return pts.some((a, i) => i > 0 && distanceToSegment(p, pts[i - 1] ?? a, a) <= s.size / 2 + slack);
  }
  if (LINEAR.includes(s.kind)) {
    return distanceToSegment(p, { x: s.x1, y: s.y1 }, { x: s.x2, y: s.y2 }) <= s.size / 2 + slack;
  }
  if (FREEHAND.includes(s.kind) && s.points) {
    const pts = s.points;
    const half = (s.kind === "highlight" ? markerWidth(s.size) : s.size) / 2;
    return pts.some((a, i) => i > 0 && distanceToSegment(p, pts[i - 1] ?? a, a) <= half + slack);
  }
  const b = bounds(ctx, s);
  return p.x >= b.x - slack && p.x <= b.x + b.w + slack && p.y >= b.y - slack && p.y <= b.y + b.h + slack;
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
  ctx.beginPath();
  ctx.rect(r.x, r.y, r.w, r.h);
  ctx.clip();
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(scratch, 0, 0, w, h, r.x, r.y, w * BLOCK, h * BLOCK);
  ctx.restore();
}

/** Blurred areas, per image, so that redraws (every mouse move) stay cheap. */
const blurred = new WeakMap<object, Map<string, HTMLCanvasElement>>();

function blur(ctx: CanvasRenderingContext2D, image: CanvasImageSource, r: Rect, radius: number) {
  const x = Math.floor(r.x);
  const y = Math.floor(r.y);
  const w = Math.max(1, Math.ceil(r.w));
  const h = Math.max(1, Math.ceil(r.h));
  const key = `${x},${y},${w},${h},${radius}`;
  let cache = blurred.get(image);
  if (!cache) {
    cache = new Map();
    blurred.set(image, cache);
  }
  let layer = cache.get(key);
  if (!layer) {
    // Only the area (plus a margin the blur reads from) is processed.
    const m = Math.ceil(radius * 5);
    layer = document.createElement("canvas");
    layer.width = w;
    layer.height = h;
    const lctx = layer.getContext("2d");
    if (!lctx) return;
    // Strong enough that even large text cannot be read.
    lctx.filter = `blur(${radius * 1.6}px)`;
    lctx.drawImage(image, x - m, y - m, w + 2 * m, h + 2 * m, -m, -m, w + 2 * m, h + 2 * m);
    if (cache.size > 64) cache.clear();
    cache.set(key, layer);
  }
  ctx.drawImage(layer, x, y);
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

/** Arrow along a curve; the head follows the curve's direction at its end. */
function curvedArrow(ctx: CanvasRenderingContext2D, s: Shape) {
  const { control: c } = curvePoints(s);
  const length = Math.hypot(s.x2 - s.x1, s.y2 - s.y1);
  const head = Math.min(length * 0.5, 10 + s.size * 3);
  let angle = Math.atan2(s.y2 - c.y, s.x2 - c.x);
  if (Math.hypot(s.x2 - c.x, s.y2 - c.y) < 1) angle = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
  // The curve ends inside the head so its end does not poke out.
  const ex = s.x2 - Math.cos(angle) * head * 0.6;
  const ey = s.y2 - Math.sin(angle) * head * 0.6;
  ctx.beginPath();
  ctx.moveTo(s.x1, s.y1);
  ctx.quadraticCurveTo(c.x, c.y, ex, ey);
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

/** Smooth freehand path through the midpoints of the samples. */
function pen(ctx: CanvasRenderingContext2D, points: Point[]) {
  const [first, ...rest] = points;
  if (!first) return;
  ctx.beginPath();
  ctx.moveTo(first.x, first.y);
  if (!rest.length) ctx.lineTo(first.x + 0.01, first.y);
  for (let i = 0; i < rest.length; i++) {
    const p = rest[i] as Point;
    const next = rest[i + 1];
    if (next) ctx.quadraticCurveTo(p.x, p.y, (p.x + next.x) / 2, (p.y + next.y) / 2);
    else ctx.lineTo(p.x, p.y);
  }
  ctx.stroke();
}

function roundRect(ctx: CanvasRenderingContext2D, r: Rect, radius: number) {
  ctx.beginPath();
  ctx.roundRect(r.x, r.y, r.w, r.h, radius);
}

/** Draws the image, then the annotations. */
export function render(
  ctx: CanvasRenderingContext2D,
  image: CanvasImageSource & { width: number; height: number },
  shapes: Shape[],
  scratch: HTMLCanvasElement,
) {
  ctx.drawImage(image, 0, 0);
  // Hiding tools act on the original pixels, under every annotation.
  for (const s of shapes) {
    if (s.kind === "pixelate") pixelate(ctx, image, normalize(s), scratch);
    if (s.kind === "blur") blur(ctx, image, normalize(s), Math.max(6, s.size * 2.5));
  }
  const spots = shapes.filter((s) => s.kind === "spotlight");
  if (spots.length) {
    // Everything but the spotlights is dimmed.
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, 0, image.width, image.height);
    for (const s of spots) {
      const r = normalize(s);
      ctx.moveTo(r.x + r.w, r.y + r.h / 2);
      ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, r.h / 2, 0, 0, Math.PI * 2, true);
    }
    ctx.fillStyle = "rgba(0,0,0,0.6)";
    ctx.fill("evenodd");
    ctx.restore();
  }
  renderAnnotations(ctx, shapes);
}

/** Draws the annotations only (no image, no hiding tools): video overlays. */
export function renderAnnotations(ctx: CanvasRenderingContext2D, shapes: Shape[]) {
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
      case "curve":
        ctx.shadowColor = "rgba(0,0,0,0.35)";
        ctx.shadowBlur = s.size;
        curvedArrow(ctx, s);
        break;
      case "line":
        ctx.beginPath();
        ctx.moveTo(s.x1, s.y1);
        ctx.lineTo(s.x2, s.y2);
        ctx.stroke();
        break;
      case "pen":
        pen(ctx, s.points ?? []);
        break;
      case "rect":
      case "ellipse": {
        ctx.beginPath();
        if (s.kind === "rect") ctx.rect(r.x, r.y, r.w, r.h);
        else ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, r.h / 2, 0, 0, Math.PI * 2);
        if (s.fill !== "none") {
          ctx.globalAlpha = s.fill === "soft" ? 0.25 : 1;
          ctx.fill();
          ctx.globalAlpha = 1;
        }
        if (s.fill !== "solid") ctx.stroke();
        break;
      }
      case "image": {
        const bitmap = s.picture === undefined ? undefined : pictures.get(s.picture);
        if (bitmap) {
          ctx.globalAlpha = s.opacity ?? 1;
          ctx.imageSmoothingQuality = "high";
          ctx.drawImage(bitmap, r.x, r.y, r.w, r.h);
        }
        break;
      }
      case "highlight":
        // A felt marker: wide, flat-ended, see-through (visible on dark
        // screens too, unlike a multiply blend).
        ctx.globalAlpha = 0.45;
        if (s.points?.length) {
          ctx.lineWidth = markerWidth(s.size);
          ctx.lineCap = "butt";
          pen(ctx, s.points);
        } else {
          ctx.fillRect(r.x, r.y, r.w, r.h);
        }
        break;
      case "text": {
        const size = fontSize(s.size);
        let ink = s.color;
        if (s.fill !== "none" && s.fill !== "plain") {
          // A box behind the text: tinted or solid.
          const box = textBox(ctx, s);
          roundRect(ctx, box, size * 0.25);
          ctx.globalAlpha = s.fill === "soft" ? 0.85 : 1;
          ctx.fill();
          ctx.globalAlpha = 1;
          ink = contrast(s.color);
        } else if (s.fill === "none") {
          ctx.lineWidth = Math.max(2, size / 7);
          ctx.strokeStyle = contrast(s.color);
        }
        ctx.font = textFont(s, size);
        ctx.textBaseline = "top";
        ctx.fillStyle = ink;
        (s.text ?? "").split("\n").forEach((line, i) => {
          const y = s.y1 + i * size * 1.25;
          if (s.fill === "none") ctx.strokeText(line, s.x1, y);
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
        ctx.font = `700 ${radius * 1.1}px ${FONT}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(String(s.n ?? 1), s.x1, s.y1 + radius * 0.05);
        break;
      }
    }
    ctx.restore();
  }
}

// ── Frame ("beautify") ─────────────────────────────────────────────────────

export interface Frame {
  enabled: boolean;
  /** Index in BACKGROUNDS. */
  background: number;
  padding: number;
  radius: number;
  shadow: boolean;
}

export const BACKGROUNDS: { name: string; stops: string[] }[] = [
  { name: "Ocean", stops: ["#4f7cff", "#7b4dff"] },
  { name: "Sunset", stops: ["#ff7a59", "#ff3d7f"] },
  { name: "Mint", stops: ["#34d399", "#0ea5e9"] },
  { name: "Night", stops: ["#1f2937", "#0b1120"] },
  { name: "Paper", stops: ["#f5f5f4", "#e2e0dd"] },
  { name: "Sky", stops: ["#7dd3fc", "#2563eb"] },
  { name: "Peach", stops: ["#fdba74", "#f472b6"] },
  { name: "Lagoon", stops: ["#22d3ee", "#6366f1"] },
  { name: "Grape", stops: ["#c084fc", "#4c1d95"] },
  { name: "Forest", stops: ["#4ade80", "#14532d"] },
  { name: "Ember", stops: ["#f59e0b", "#b91c1c"] },
  { name: "Slate", stops: ["#94a3b8", "#334155"] },
  { name: "Aurora", stops: ["#a7f3d0", "#3b82f6", "#a855f7"] },
  { name: "Candy", stops: ["#f9a8d4", "#a5b4fc"] },
  { name: "Ink", stops: ["#111827", "#000000"] },
  { name: "White", stops: ["#ffffff", "#f1f5f9"] },
];

/** Index of the background the user mixes from two colours of their choice. */
export const CUSTOM_WALL = BACKGROUNDS.length;

/** Colours of background `index`; `custom` are the user's own. */
export const wallStops = (index: number, custom: string[]): string[] =>
  index === CUSTOM_WALL ? custom : (BACKGROUNDS[index]?.stops ?? ["#444", "#222"]);

export function paintBackground(ctx: CanvasRenderingContext2D, r: Rect, index: number, custom: string[]) {
  const stops = wallStops(index, custom);
  const g = ctx.createLinearGradient(r.x, r.y, r.x + r.w, r.y + r.h);
  stops.forEach((c, i) => g.addColorStop(i / Math.max(1, stops.length - 1), c));
  ctx.fillStyle = g;
  ctx.fillRect(r.x, r.y, r.w, r.h);
}

/** Runs `draw` clipped to `r` with rounded corners, over a drop shadow. */
export function paintCard(
  ctx: CanvasRenderingContext2D,
  draw: () => void,
  r: Rect,
  frame: Frame,
) {
  if (frame.shadow) {
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,0.45)";
    ctx.shadowBlur = Math.max(12, frame.padding / 2);
    ctx.shadowOffsetY = Math.max(4, frame.padding / 8);
    roundRect(ctx, r, frame.radius);
    ctx.fillStyle = "#000";
    ctx.fill();
    ctx.restore();
  }
  ctx.save();
  roundRect(ctx, r, frame.radius);
  ctx.clip();
  draw();
  ctx.restore();
}
