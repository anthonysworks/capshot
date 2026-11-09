<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { editorCopy, editorImageSaveAs, editorRead, editorSave, editorSaveTo, showInFolder } from "../lib/api";
  import { VIVID } from "./colors";
  import Icon from "./Icon.svelte";
  import Kbd from "./Kbd.svelte";
  import type { IconName } from "./icons";
  import {
    BACKGROUNDS,
    BOXED,
    FREEHAND,
    type Fill,
    type Frame,
    LINEAR,
    bendTowards,
    curvePoints,
    drawnBend,
    TEXT_FONTS,
    type TextFont,
    textFont,
    type Point,
    type Rect,
    type Shape,
    type ShapeKind,
    bounds,
    fontSize,
    hit,
    normalize,
    paintBackground,
    paintCard,
    render,
  } from "./shapes";

  let { path }: { path: string } = $props();

  type Tool = "select" | "crop" | "frame" | ShapeKind;
  type ToolInfo = { tool: Tool; icon: IconName; label: string; key: string };
  /** Tools grouped by what they are for. */
  const GROUPS: { title: string; tools: ToolInfo[] }[] = [
    { title: "Select", tools: [{ tool: "select", icon: "select", label: "Select, move, resize", key: "V" }] },
    {
      title: "Shapes",
      tools: [
        { tool: "arrow", icon: "arrow", label: "Arrow", key: "A" },
        { tool: "curve", icon: "curve", label: "Curved arrow (drag its middle to bend it)", key: "Q" },
        { tool: "line", icon: "line", label: "Line", key: "L" },
        { tool: "rect", icon: "rect", label: "Rectangle", key: "R" },
        { tool: "ellipse", icon: "ellipse", label: "Ellipse", key: "E" },
      ],
    },
    {
      title: "Draw",
      tools: [
        { tool: "pen", icon: "pen", label: "Pencil (freehand)", key: "P" },
        { tool: "highlight", icon: "highlight", label: "Highlighter", key: "H" },
      ],
    },
    {
      title: "Text",
      tools: [
        { tool: "text", icon: "text", label: "Text", key: "T" },
        { tool: "number", icon: "number", label: "Numbered step", key: "N" },
      ],
    },
    {
      title: "Hide",
      tools: [
        { tool: "spotlight", icon: "spotlight", label: "Spotlight (dims the rest)", key: "S" },
        { tool: "blur", icon: "blur", label: "Blur", key: "U" },
        { tool: "pixelate", icon: "pixelate", label: "Pixelate (hides text for good)", key: "B" },
      ],
    },
    {
      title: "Image",
      tools: [
        { tool: "crop", icon: "crop", label: "Crop", key: "C" },
        { tool: "frame", icon: "frame", label: "Frame: background, margin, corners", key: "F" },
      ],
    },
  ];
  const TOOLS = GROUPS.flatMap((g) => g.tools);
  /** Default colour of each tool (the highlighter is yellow); the last
   * colour picked for a tool is remembered. */
  const toolColors: Partial<Record<Tool, string>> = { highlight: "#ffee00" };
  let lastColor = VIVID[0]!;
  const COLORS = VIVID;
  const SIZES = [
    { level: 1, label: "Thin" },
    { level: 2, label: "Medium" },
    { level: 4, label: "Thick" },
  ];
  const FILLS: { fill: Fill; label: string }[] = [
    { fill: "none", label: "Outline" },
    { fill: "soft", label: "Tinted" },
    { fill: "solid", label: "Solid" },
  ];
  /** Tools whose shapes use the fill setting. */
  const FILLABLE: ShapeKind[] = ["rect", "ellipse", "text"];

  let image = $state<ImageBitmap | null>(null);
  let error = $state("");
  let tool = $state<Tool>("arrow");
  let color = $state<string>(VIVID[0]!);
  let level = $state(2);
  let fill = $state<Fill>("none");
  /** Style of new texts (and of the selected one). */
  let textLook = $state<{ font: TextFont; bold: boolean; italic: boolean }>({ font: "sans", bold: true, italic: false });
  /** While drawing a curved arrow: the path followed, to bend it that way. */
  let curvePath: Point[] = [];
  let shapes = $state<Shape[]>([]);
  let crop = $state<Rect | null>(null);
  let frame = $state<Frame>({ enabled: false, background: 0, padding: 64, radius: 12, shadow: true });
  let draft = $state<Shape | null>(null);
  let cropDraft = $state<Rect | null>(null);
  let selected = $state<number | null>(null);
  let past = $state<string[]>([]);
  let future = $state<string[]>([]);
  let dirty = $state(false);
  /** Asking whether to close with unsaved changes. */
  let closing = $state(false);
  let toast = $state("");
  let editing = $state<{ x: number; y: number; text: string; id: number | null } | null>(null);
  /** Physical pixels per image pixel, and where the image starts. */
  let view = $state({ s: 1, ox: 0, oy: 0, dpr: 1 });
  let fitMode = $state(true);
  let spaceDown = $state(false);
  let copyPath = $state<string | null>(null);

  let canvas: HTMLCanvasElement;
  let stage: HTMLDivElement;
  const scratch = document.createElement("canvas");
  let nextId = 1;
  type Drag =
    | { kind: "draw" | "crop"; start: Point; moved: boolean }
    | { kind: "move"; last: Point; moved: boolean }
    | { kind: "handle"; handle: number; moved: boolean }
    | { kind: "pan"; last: { x: number; y: number }; moved: boolean };
  let drag = $state<Drag | null>(null);

  /** Stroke width for the current level, relative to the image size so
   * annotations look the same on a small capture and a 4K one. */
  const unit = $derived(image ? Math.max(1, Math.round(Math.max(image.width, image.height) / 1400)) : 1);
  const stroke = $derived((level + 2) * unit);
  const selectedShape = $derived(shapes.find((s) => s.id === selected) ?? null);
  /** 100 %: one image pixel per screen pixel. */
  const zoomLabel = $derived(`${Math.round(view.s * 100)} %`);

  onMount(() => {
    (async () => {
      try {
        const bytes = await editorRead(path);
        image = await createImageBitmap(new Blob([bytes]));
        const n = bytes.byteLength;
        const weight = n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`;
        const name = path.split(/[\\/]/).pop() ?? path;
        getCurrentWindow()
          .setTitle(`Kadra Editor · ${name} · ${image.width}×${image.height} · ${weight}`)
          .catch(() => {});
      } catch (e) {
        error = String(e);
      }
    })();
    const observer = new ResizeObserver(() => layout());
    observer.observe(stage);
    const unlisten = getCurrentWindow().onCloseRequested((event) => {
      // Browser dialogs can be blocked in the webview: ask in the page.
      if (dirty) {
        event.preventDefault();
        closing = true;
      }
    });
    return () => {
      observer.disconnect();
      unlisten.then((f) => f());
    };
  });

  // ── View ───────────────────────────────────────────────────────────────

  function layout() {
    if (!canvas || !stage) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(stage.clientWidth * dpr);
    canvas.height = Math.round(stage.clientHeight * dpr);
    if (!image) return;
    if (fitMode) fit();
    else view = { ...view, dpr };
  }

  /** Fits the whole result in the window (never enlarging past 100 %). */
  function fit() {
    if (!image || !canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const margin = 24 * dpr;
    const area = crop ?? { x: 0, y: 0, w: image.width, h: image.height };
    const pad = frame.enabled ? frame.padding : 0;
    const w = area.w + 2 * pad;
    const h = area.h + 2 * pad;
    const s = Math.min((canvas.width - margin * 2) / w, (canvas.height - margin * 2) / h, 1);
    view = {
      s,
      ox: (canvas.width - w * s) / 2 + (pad - area.x) * s,
      oy: (canvas.height - h * s) / 2 + (pad - area.y) * s,
      dpr,
    };
    fitMode = true;
  }

  function zoomTo(s: number, cx = canvas.width / 2, cy = canvas.height / 2) {
    const clamped = Math.max(0.05, Math.min(8, s));
    view = {
      ...view,
      s: clamped,
      ox: cx - ((cx - view.ox) * clamped) / view.s,
      oy: cy - ((cy - view.oy) * clamped) / view.s,
    };
    fitMode = false;
  }

  function wheel(e: WheelEvent) {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    if (e.ctrlKey) {
      const factor = Math.exp(-e.deltaY * 0.0015);
      zoomTo(view.s * factor, (e.clientX - rect.left) * view.dpr, (e.clientY - rect.top) * view.dpr);
    } else {
      const dx = (e.shiftKey ? e.deltaY : e.deltaX) * view.dpr;
      const dy = (e.shiftKey ? 0 : e.deltaY) * view.dpr;
      view = { ...view, ox: view.ox - dx, oy: view.oy - dy };
      fitMode = false;
    }
  }

  // Fit when the image arrives, and again when the frame or the crop
  // changes the size of the result (unless the user zoomed).
  $effect(() => {
    void $state.snapshot(crop);
    void frame.enabled;
    void frame.padding;
    if (image && fitMode) fit();
  });

  // Redraw whenever anything visible changes.
  $effect(() => {
    const state = $state.snapshot({ shapes, crop, draft, cropDraft, selected, view, editing, frame, tool });
    if (!image || !canvas) return;
    draw(state.shapes as Shape[], state.draft as Shape | null);
  });

  function draw(list: Shape[], extra: Shape | null) {
    const ctx = canvas.getContext("2d");
    if (!ctx || !image) return;
    const { s, ox, oy } = view;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(s, 0, 0, s, ox, oy);
    const visible = list.filter((sh) => sh.id !== editing?.id);
    const shown = extra ? [...visible, extra] : visible;
    const area = crop ?? { x: 0, y: 0, w: image.width, h: image.height };
    const cropping = tool === "crop";
    const img = image;

    if (frame.enabled && !cropping) {
      const p = frame.padding;
      paintBackground(ctx, { x: area.x - p, y: area.y - p, w: area.w + 2 * p, h: area.h + 2 * p }, frame.background);
      paintCard(ctx, () => render(ctx, img, shown, scratch), area, frame);
    } else if (crop && !cropping) {
      ctx.save();
      ctx.beginPath();
      ctx.rect(area.x, area.y, area.w, area.h);
      ctx.clip();
      render(ctx, img, shown, scratch);
      ctx.restore();
    } else {
      render(ctx, img, shown, scratch);
    }

    // While cropping, the whole image shows with the outside dimmed.
    const cropArea = cropDraft ?? (cropping ? crop : null);
    if (cropArea) {
      ctx.fillStyle = "rgba(0,0,0,0.55)";
      const { width: w, height: h } = img;
      ctx.fillRect(0, 0, w, cropArea.y);
      ctx.fillRect(0, cropArea.y + cropArea.h, w, h - cropArea.y - cropArea.h);
      ctx.fillRect(0, cropArea.y, cropArea.x, cropArea.h);
      ctx.fillRect(cropArea.x + cropArea.w, cropArea.y, w - cropArea.x - cropArea.w, cropArea.h);
      outline(ctx, cropArea);
    }
    const chosen = list.find((sh) => sh.id === selected);
    if (chosen && tool === "select") {
      outline(ctx, bounds(ctx, chosen));
      for (const h of handles(chosen)) dot(ctx, h);
    }
  }

  function outline(ctx: CanvasRenderingContext2D, r: Rect) {
    const px = 1 / view.s;
    ctx.save();
    ctx.lineWidth = 1.5 * px;
    ctx.setLineDash([6 * px, 4 * px]);
    ctx.strokeStyle = "#fff";
    ctx.strokeRect(r.x, r.y, r.w, r.h);
    ctx.lineDashOffset = 6 * px;
    ctx.strokeStyle = "#000";
    ctx.strokeRect(r.x, r.y, r.w, r.h);
    ctx.restore();
  }

  function dot(ctx: CanvasRenderingContext2D, p: Point) {
    const r = (5 * view.dpr) / view.s;
    ctx.save();
    ctx.beginPath();
    ctx.arc(p.x, p.y, r, 0, Math.PI * 2);
    ctx.fillStyle = "#fff";
    ctx.fill();
    ctx.lineWidth = (1.5 * view.dpr) / view.s;
    ctx.strokeStyle = "#3b63e6";
    ctx.stroke();
    ctx.restore();
  }

  /** Resize handles: end points of lines, corners of boxes. */
  function handles(s: Shape): Point[] {
    if (s.kind === "curve") return [{ x: s.x1, y: s.y1 }, { x: s.x2, y: s.y2 }, curvePoints(s).apex];
    if (LINEAR.includes(s.kind)) {
      return [
        { x: s.x1, y: s.y1 },
        { x: s.x2, y: s.y2 },
      ];
    }
    if (BOXED.includes(s.kind)) {
      return [
        { x: s.x1, y: s.y1 },
        { x: s.x2, y: s.y1 },
        { x: s.x2, y: s.y2 },
        { x: s.x1, y: s.y2 },
      ];
    }
    return [];
  }

  // ── History ────────────────────────────────────────────────────────────

  const serialize = () =>
    JSON.stringify({ shapes: $state.snapshot(shapes), crop: $state.snapshot(crop), frame: $state.snapshot(frame) });

  function remember() {
    past.push(serialize());
    if (past.length > 200) past.shift();
    future = [];
    dirty = true;
  }

  function commit(change: () => void) {
    remember();
    change();
  }

  function restore(from: string[], to: string[]) {
    const state = from.pop();
    if (state === undefined) return;
    to.push(serialize());
    const parsed = JSON.parse(state);
    shapes = parsed.shapes;
    crop = parsed.crop;
    frame = parsed.frame;
    selected = null;
    dirty = true;
  }

  const undo = () => restore(past, future);
  const redo = () => restore(future, past);

  // ── Pointer ────────────────────────────────────────────────────────────

  function toImage(e: { clientX: number; clientY: number }): Point {
    const rect = canvas.getBoundingClientRect();
    return {
      x: ((e.clientX - rect.left) * view.dpr - view.ox) / view.s,
      y: ((e.clientY - rect.top) * view.dpr - view.oy) / view.s,
    };
  }

  function clamp(p: Point): Point {
    if (!image) return p;
    return { x: Math.max(0, Math.min(image.width, p.x)), y: Math.max(0, Math.min(image.height, p.y)) };
  }

  function topmost(p: Point): Shape | undefined {
    const ctx = canvas.getContext("2d");
    if (!ctx) return undefined;
    return [...shapes].reverse().find((sh) => hit(ctx, sh, p, (6 * view.dpr) / view.s));
  }

  function handleAt(p: Point): number {
    if (!selectedShape) return -1;
    return handles(selectedShape).findIndex((h) => Math.hypot(h.x - p.x, h.y - p.y) * view.s <= 9 * view.dpr);
  }

  function make(kind: ShapeKind, p: Point, extra: Partial<Shape>): Shape {
    return {
      id: nextId++,
      kind,
      color,
      size: kind === "text" ? (level + 1) * unit : kind === "blur" ? 6 * unit : stroke,
      fill: FILLABLE.includes(kind) ? fill : "none",
      ...(kind === "text" ? { ...textLook } : {}),
      x1: p.x,
      y1: p.y,
      x2: p.x,
      y2: p.y,
      ...extra,
    };
  }

  function pointerDown(e: PointerEvent) {
    if (!image) return;
    // Keeps the focus where it is (the text box opened below).
    e.preventDefault();
    if (editing) return commitText();
    canvas.setPointerCapture(e.pointerId);
    if (e.button === 1 || (e.button === 0 && spaceDown)) {
      drag = { kind: "pan", last: { x: e.clientX, y: e.clientY }, moved: false };
      return;
    }
    if (e.button !== 0) return;
    const raw = toImage(e);
    const p = clamp(raw);

    if (tool === "select") {
      const handle = handleAt(raw);
      if (handle >= 0) {
        remember();
        drag = { kind: "handle", handle, moved: false };
        return;
      }
      const found = topmost(raw);
      selected = found?.id ?? null;
      if (found) {
        remember();
        drag = { kind: "move", last: raw, moved: false };
      }
      return;
    }
    if (tool === "text") {
      const found = topmost(raw);
      editing =
        found?.kind === "text"
          ? { x: found.x1, y: found.y1, text: found.text ?? "", id: found.id }
          : { x: p.x, y: p.y, text: "", id: null };
      return;
    }
    if (tool === "number") {
      const n = Math.max(0, ...shapes.map((sh) => sh.n ?? 0)) + 1;
      commit(() => shapes.push(make("number", p, { n })));
      return;
    }
    if (tool === "crop") {
      drag = { kind: "crop", start: p, moved: false };
      cropDraft = { x: p.x, y: p.y, w: 0, h: 0 };
      return;
    }
    drag = { kind: "draw", start: p, moved: false };
    if (tool === "frame") return;
    curvePath = [p];
    draft = make(tool, p, FREEHAND.includes(tool) ? { points: [p] } : {});
  }

  function pointerMove(e: PointerEvent) {
    const current = drag;
    if (!current) return;
    if (current.kind === "pan") {
      view = {
        ...view,
        ox: view.ox + (e.clientX - current.last.x) * view.dpr,
        oy: view.oy + (e.clientY - current.last.y) * view.dpr,
      };
      current.last = { x: e.clientX, y: e.clientY };
      fitMode = false;
      return;
    }
    const raw = toImage(e);
    const p = clamp(raw);
    const sh = selectedShape;
    if (current.kind === "move") {
      if (!sh) return;
      const dx = raw.x - current.last.x;
      const dy = raw.y - current.last.y;
      current.moved ||= dx !== 0 || dy !== 0;
      sh.x1 += dx;
      sh.x2 += dx;
      sh.y1 += dy;
      sh.y2 += dy;
      if (sh.points) sh.points = sh.points.map((q) => ({ x: q.x + dx, y: q.y + dy }));
      current.last = raw;
    } else if (current.kind === "handle") {
      if (!sh) return;
      current.moved = true;
      if (LINEAR.includes(sh.kind)) {
        if (current.handle === 2) sh.bend = bendTowards(sh, raw);
        else         if (current.handle === 0) [sh.x1, sh.y1] = [p.x, p.y];
        else [sh.x2, sh.y2] = [p.x, p.y];
      } else {
        // Corners in order: (x1,y1) (x2,y1) (x2,y2) (x1,y2).
        if (current.handle === 0 || current.handle === 3) sh.x1 = p.x;
        else sh.x2 = p.x;
        if (current.handle === 0 || current.handle === 1) sh.y1 = p.y;
        else sh.y2 = p.y;
      }
    } else {
      current.moved ||= Math.hypot(p.x - current.start.x, p.y - current.start.y) * view.s > 3;
      if (current.kind === "crop") {
        cropDraft = normalize({ x1: current.start.x, y1: current.start.y, x2: p.x, y2: p.y });
      } else if (draft) {
        if (FREEHAND.includes(draft.kind)) draft.points = [...(draft.points ?? []), p];
        else if (e.shiftKey && LINEAR.includes(draft.kind)) {
          // Shift: snap to 45°.
          const angle = Math.round(Math.atan2(p.y - draft.y1, p.x - draft.x1) / (Math.PI / 4)) * (Math.PI / 4);
          const length = Math.hypot(p.x - draft.x1, p.y - draft.y1);
          draft.x2 = draft.x1 + Math.cos(angle) * length;
          draft.y2 = draft.y1 + Math.sin(angle) * length;
        } else if (e.shiftKey && BOXED.includes(draft.kind)) {
          // Shift: square or circle.
          const side = Math.max(Math.abs(p.x - draft.x1), Math.abs(p.y - draft.y1));
          draft.x2 = draft.x1 + Math.sign(p.x - draft.x1 || 1) * side;
          draft.y2 = draft.y1 + Math.sign(p.y - draft.y1 || 1) * side;
        } else {
          draft.x2 = p.x;
          draft.y2 = p.y;
        }
        if (draft.kind === "curve") {
          curvePath.push(p);
          draft.bend = drawnBend(draft, curvePath);
        }
      }
    }
  }

  function pointerUp() {
    const current = drag;
    drag = null;
    if (!current || current.kind === "pan") return;
    if (current.kind === "move" || current.kind === "handle") {
      // Nothing changed: forget the history entry taken on press.
      if (!current.moved) past.pop();
      return;
    }
    if (current.kind === "crop") {
      const area = cropDraft;
      cropDraft = null;
      if (area && current.moved && area.w > 4 && area.h > 4) {
        commit(
          () => (crop = { x: Math.round(area.x), y: Math.round(area.y), w: Math.round(area.w), h: Math.round(area.h) }),
        );
      }
      return;
    }
    if (draft) {
      const shape = draft;
      draft = null;
      if (current.moved) commit(() => shapes.push(shape));
    }
  }

  function doubleClick(e: MouseEvent) {
    const found = topmost(toImage(e));
    if (found?.kind === "text") {
      selected = found.id;
      editing = { x: found.x1, y: found.y1, text: found.text ?? "", id: found.id };
    }
  }

  // ── Text ───────────────────────────────────────────────────────────────

  /** Focuses the text box as soon as it exists, so that no key typed right
   * after the click is lost (or taken as a tool shortcut). */
  function focusNow(node: HTMLTextAreaElement) {
    node.focus();
  }

  function commitText() {
    if (!editing) return;
    const { x, y, text, id } = editing;
    editing = null;
    const trimmed = text.replace(/\s+$/, "");
    const existing = shapes.find((sh) => sh.id === id);
    if (existing) {
      if (existing.text === trimmed) return;
      commit(() => {
        if (trimmed) existing.text = trimmed;
        else shapes = shapes.filter((sh) => sh.id !== id);
      });
    } else if (trimmed) {
      commit(() => shapes.push(make("text", { x, y }, { text: trimmed })));
    }
  }

  function textKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") editing = null;
    else if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      commitText();
    }
  }

  // ── Style of the selection ─────────────────────────────────────────────

  function setColor(c: string) {
    color = c;
    if (tool === "select") lastColor = c;
    else toolColors[tool] = c;
    const sh = selectedShape;
    if (sh && tool === "select") commit(() => (sh.color = c));
  }

  function setLevel(l: number) {
    level = l;
    const sh = selectedShape;
    if (sh && tool === "select" && sh.kind !== "pixelate") {
      commit(() => (sh.size = sh.kind === "text" ? (l + 1) * unit : (l + 2) * unit));
    }
  }

  function setFill(f: Fill) {
    fill = f;
    const sh = selectedShape;
    if (sh && tool === "select" && FILLABLE.includes(sh.kind)) commit(() => (sh.fill = f));
  }

  /** Font, bold, italic: for new texts and the selected one. */
  function setTextLook(change: Partial<typeof textLook>) {
    textLook = { ...textLook, ...change };
    const sh = selectedShape;
    if (sh?.kind === "text") commit(() => Object.assign(sh, change));
  }

  function setFrame(change: Partial<Frame>) {
    commit(() => (frame = { ...frame, ...change }));
  }

  // ── Output ─────────────────────────────────────────────────────────────

  async function flatten(): Promise<Uint8Array> {
    if (!image) throw new Error("no image");
    const img = image;
    const area = crop ?? { x: 0, y: 0, w: img.width, h: img.height };
    const content = document.createElement("canvas");
    content.width = area.w;
    content.height = area.h;
    const cctx = content.getContext("2d");
    if (!cctx) throw new Error("no canvas");
    cctx.translate(-area.x, -area.y);
    render(cctx, img, $state.snapshot(shapes) as Shape[], scratch);

    let out = content;
    if (frame.enabled) {
      const p = frame.padding;
      out = document.createElement("canvas");
      out.width = area.w + 2 * p;
      out.height = area.h + 2 * p;
      const ctx = out.getContext("2d");
      if (!ctx) throw new Error("no canvas");
      paintBackground(ctx, { x: 0, y: 0, w: out.width, h: out.height }, frame.background);
      paintCard(ctx, () => ctx.drawImage(content, p, p), { x: p, y: p, w: area.w, h: area.h }, frame);
    }
    const blob = await new Promise<Blob | null>((resolve) => out.toBlob(resolve, "image/png"));
    if (!blob) throw new Error("cannot encode the image");
    return new Uint8Array(await blob.arrayBuffer());
  }

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function notify(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 3000);
  }

  async function copy() {
    try {
      await editorCopy(await flatten());
      notify("Copied to the clipboard");
    } catch (e) {
      notify(String(e));
    }
  }

  async function save() {
    try {
      await editorSave(path, await flatten());
      dirty = false;
      notify("Saved");
    } catch (e) {
      notify(String(e));
    }
  }

  /** Saves to a name and place picked in a dialog. */
  async function saveAs() {
    try {
      const output = await editorImageSaveAs(path);
      if (!output) return;
      copyPath = await editorSaveTo(path, output, await flatten());
      dirty = false;
      notify(`Saved as ${copyPath.split(/[\\/]/).pop()}`);
    } catch (e) {
      notify(String(e));
    }
  }

  // ── Keyboard ───────────────────────────────────────────────────────────

  function duplicate() {
    if (!selectedShape) return;
    const copyOf: Shape = { ...$state.snapshot(selectedShape), id: nextId++ };
    nudge(copyOf, 12 * unit, 12 * unit);
    commit(() => shapes.push(copyOf));
    selected = copyOf.id;
  }

  function removeSelected() {
    const id = selected;
    if (id === null) return;
    selected = null;
    commit(() => (shapes = shapes.filter((sh) => sh.id !== id)));
  }

  function nudge(sh: Shape, dx: number, dy: number) {
    sh.x1 += dx;
    sh.x2 += dx;
    sh.y1 += dy;
    sh.y2 += dy;
    if (sh.points) sh.points = sh.points.map((q) => ({ x: q.x + dx, y: q.y + dy }));
  }

  function keyDown(e: KeyboardEvent) {
    if (editing) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (e.key === " ") {
      spaceDown = true;
      e.preventDefault();
      return;
    }
    if (ctrl && key === "z") {
      e.preventDefault();
      if (e.shiftKey) redo();
      else undo();
    } else if (ctrl && key === "y") {
      e.preventDefault();
      redo();
    } else if (ctrl && key === "c") {
      e.preventDefault();
      copy();
    } else if (ctrl && key === "s") {
      e.preventDefault();
      if (e.shiftKey) saveAs();
      else save();
    } else if (ctrl && key === "d" && selectedShape) {
      e.preventDefault();
      const duplicate: Shape = { ...$state.snapshot(selectedShape), id: nextId++ };
      nudge(duplicate, 12 * unit, 12 * unit);
      commit(() => shapes.push(duplicate));
      selected = duplicate.id;
    } else if (ctrl && key === "0") {
      e.preventDefault();
      fit();
    } else if (ctrl && key === "1") {
      e.preventDefault();
      zoomTo(1);
    } else if (ctrl && (key === "=" || key === "+" || key === "-")) {
      e.preventDefault();
      zoomTo(view.s * (key === "-" ? 1 / 1.25 : 1.25));
    } else if ((e.key === "Delete" || e.key === "Backspace") && selected !== null) {
      const id = selected;
      selected = null;
      commit(() => (shapes = shapes.filter((sh) => sh.id !== id)));
    } else if (e.key.startsWith("Arrow") && selectedShape) {
      e.preventDefault();
      const step = (e.shiftKey ? 10 : 1) * unit;
      const dx = e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0;
      const dy = e.key === "ArrowUp" ? -step : e.key === "ArrowDown" ? step : 0;
      const sh = selectedShape;
      commit(() => nudge(sh, dx, dy));
    } else if (e.key === "Escape") {
      selected = null;
      draft = null;
      cropDraft = null;
      if (tool === "crop") tool = "select";
    } else if (e.key === "Enter" && tool === "crop") {
      tool = "select";
    } else if (!ctrl && !e.altKey) {
      const found = TOOLS.find((t) => t.key.toLowerCase() === key);
      if (found) pick(found.tool);
    }
  }

  /** Switches tool, with that tool's colour. */
  function pick(next: Tool) {
    if (tool !== "select" && tool !== "crop" && tool !== "frame" && !toolColors[tool]) lastColor = color;
    tool = next;
    if (next !== "select" && next !== "crop" && next !== "frame") color = toolColors[next] ?? lastColor;
    if (next !== "select") selected = null;
  }

  function keyUp(e: KeyboardEvent) {
    if (e.key === " ") spaceDown = false;
  }

  const textStyle = $derived.by(() => {
    if (!editing) return "";
    const left = (view.ox + editing.x * view.s) / view.dpr;
    const top = (view.oy + editing.y * view.s) / view.dpr;
    const size = (fontSize((level + 1) * unit) * view.s) / view.dpr;
    const look = editing.id !== null ? shapes.find((sh) => sh.id === editing?.id) : null;
    const font = textFont({ ...(look ?? { size: level + 1 }), ...(look ? {} : textLook) } as Shape, size);
    return `left:${left}px;top:${top}px;font:${font};color:${color}`;
  });

  const showFill = $derived(
    FILLABLE.includes(tool as ShapeKind) ||
      (tool === "select" && !!selectedShape && FILLABLE.includes(selectedShape.kind)),
  );
  const cursor = $derived(
    spaceDown || drag?.kind === "pan" ? "grab" : tool === "select" ? "default" : tool === "text" ? "text" : "crosshair",
  );
</script>

<svelte:window onkeydown={keyDown} onkeyup={keyUp} />

<div class="editor">
  {#if closing}
    <div class="backdrop">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="close-title">
        <h2 id="close-title">Close without saving?</h2>
        <p>Your changes to this image will be lost.</p>
        <div class="dialog-actions">
          <button
            type="button"
            class="action primary"
            onclick={async () => {
              closing = false;
              await save();
              if (!dirty) getCurrentWindow().destroy();
            }}><Icon name="save" /> Save and close</button
          >
          <button type="button" class="action danger" onclick={() => getCurrentWindow().destroy()}>Close without saving</button>
          <button type="button" class="action" onclick={() => (closing = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}
  <header class="topbar">
    <div class="tb-side">
    <div class="group">
      <button type="button" class="icon" title="Undo (Ctrl+Z)" aria-label="Undo" disabled={!past.length} onclick={undo}
        ><Icon name="undo" /></button
      >
      <button type="button" class="icon" title="Redo (Ctrl+Y)" aria-label="Redo" disabled={!future.length} onclick={redo}
        ><Icon name="redo" /></button
      >
    </div>
    <div class="group zoom">
      <button type="button" class="icon" title="Zoom out (Ctrl+-)" aria-label="Zoom out" onclick={() => zoomTo(view.s / 1.25)}
        >−</button
      >
      <button type="button" class="chip" title="Actual size (Ctrl+1)" onclick={() => zoomTo(1)}>{zoomLabel}</button>
      <button type="button" class="icon" title="Zoom in (Ctrl++)" aria-label="Zoom in" onclick={() => zoomTo(view.s * 1.25)}
        >+</button
      >
      <button type="button" class="icon" title="Fit in the window (Ctrl+0)" aria-label="Fit" onclick={fit}
        ><Icon name="fit" /></button
      >
    </div>
    </div>
    <span class="title" title={path}>{path.split(/[\\/]/).pop()}<small>{image ? `${image.width}×${image.height}` : ""}</small></span>
    <div class="tb-side right">
    <button type="button" class="action" onclick={copy}><Icon name="copy" /> Copy <Kbd keys="Ctrl+C" /></button>
    <button type="button" class="action" onclick={saveAs}
      ><Icon name="saveCopy" /> Save as… <Kbd keys="Ctrl+Shift+S" /></button
    >
    <button type="button" class="action primary" onclick={save}><Icon name="save" /> Save <Kbd keys="Ctrl+S" /></button>
    </div>
  </header>

  <div class="body">
    <nav class="tools" aria-label="Tools">
      {#each GROUPS as group (group.title)}
        <div class="tool-group">
          <span class="heading">{group.title}</span>
          <div class="tool-grid">
            {#each group.tools as t (t.tool)}
              <button
                type="button"
                class="tool"
                class:active={tool === t.tool}
                title="{t.label} ({t.key})"
                aria-label={t.label}
                onclick={() => pick(t.tool)}
              >
                <Icon name={t.icon} />
                <span class="key">{t.key}</span>
              </button>
            {/each}
          </div>
        </div>
      {/each}
    </nav>

    <div class="stage" bind:this={stage}>
      <canvas
        bind:this={canvas}
        style="cursor:{cursor}"
        onpointerdown={pointerDown}
        onpointermove={pointerMove}
        onpointerup={pointerUp}
        ondblclick={doubleClick}
        onwheel={wheel}
      ></canvas>
      {#if editing}
        <textarea
          bind:value={editing.text}
          use:focusNow
          class="text-input"
          style={textStyle}
          rows="1"
          spellcheck="false"
          onkeydown={textKey}
          onblur={commitText}
        ></textarea>
      {/if}
      {#if error}
        <p class="message">{error}</p>
      {:else if !image}
        <p class="message">Loading…</p>
      {/if}
      {#if toast}
        <div class="toast" role="status">
          {toast}
          {#if copyPath && toast.startsWith("Saved as")}
            <button type="button" class="link" onclick={() => copyPath && showInFolder(copyPath)}>Show</button>
          {/if}
        </div>
      {/if}
      {#if tool === "crop"}
        <div class="hint">Drag to choose the area to keep · <Kbd keys="Enter" /> confirm · <Kbd keys="Esc" /> leave</div>
      {/if}
    </div>

    <aside class="panel" aria-label="Properties">
      {#if tool === "frame"}
        <section>
          <h3>Frame</h3>
          <label class="check"
            ><input type="checkbox" checked={frame.enabled} onchange={(e) => setFrame({ enabled: e.currentTarget.checked })} />
            Add a background</label
          >
          <div class="swatches">
            {#each BACKGROUNDS as b, i (b.name)}
              <button
                type="button"
                class="swatch wide"
                class:active={frame.enabled && frame.background === i}
                style="--swatch:linear-gradient(135deg,{b.stops.join(',')})"
                title={b.name}
                aria-label={b.name}
                onclick={() => setFrame({ background: i, enabled: true })}
              ></button>
            {/each}
          </div>
          <label class="field"
            >Margin <input
              type="range"
              min="16"
              max="200"
              value={frame.padding}
              onchange={(e) => setFrame({ padding: Number(e.currentTarget.value), enabled: true })}
            /></label
          >
          <label class="field"
            >Corners <input
              type="range"
              min="0"
              max="40"
              value={frame.radius}
              onchange={(e) => setFrame({ radius: Number(e.currentTarget.value), enabled: true })}
            /></label
          >
          <label class="check"
            ><input
              type="checkbox"
              checked={frame.shadow}
              onchange={(e) => setFrame({ shadow: e.currentTarget.checked, enabled: true })}
            /> Shadow</label
          >
        </section>
      {:else if tool === "crop"}
        <section>
          <h3>Crop</h3>
          <p class="note">Drag over the image to keep only that area.</p>
          {#if crop}
            <p class="note">{crop.w} × {crop.h} px</p>
            <button type="button" onclick={() => commit(() => (crop = null))}>Reset crop</button>
          {/if}
        </section>
      {:else}
        <section>
          <h3>Colour</h3>
          <div class="swatches">
            {#each COLORS as c (c)}
              <button
                type="button"
                class="swatch"
                class:active={color === c}
                style="--swatch:{c}"
                aria-label="Colour {c}"
                onclick={() => setColor(c)}
              ></button>
            {/each}
            <label class="custom" title="Any colour">
              <input type="color" value={color} onchange={(e) => setColor(e.currentTarget.value)} />
            </label>
          </div>
        </section>
        <section>
          <h3>Thickness</h3>
          <div class="row">
            {#each SIZES as sz (sz.level)}
              <button
                type="button"
                class="chip"
                class:active={level === sz.level}
                title={sz.label}
                onclick={() => setLevel(sz.level)}><span class="dot" style="--d:{3 + sz.level * 2}px"></span> {sz.label}</button
              >
            {/each}
          </div>
        </section>
        {#if showFill}
          <section>
            <h3>{tool === "text" || selectedShape?.kind === "text" ? "Background" : "Fill"}</h3>
            <div class="row">
              {#each FILLS as fl (fl.fill)}
                <button type="button" class="chip" class:active={fill === fl.fill} onclick={() => setFill(fl.fill)}
                  >{fl.label}</button
                >
              {/each}
            </div>
          </section>
        {/if}
        {#if tool === "text" || selectedShape?.kind === "text"}
          {@const look = selectedShape?.kind === "text" ? { font: selectedShape.font ?? "sans", bold: selectedShape.bold !== false, italic: !!selectedShape.italic } : textLook}
          <section>
            <h3>Text style</h3>
            <div class="fonts">
              {#each TEXT_FONTS as tf (tf.value)}
                <button type="button" class="chip" class:active={look.font === tf.value} style="font-family:{tf.family}" onclick={() => setTextLook({ font: tf.value })}
                  >{tf.label}</button
                >
              {/each}
            </div>
            <div class="row">
              <button type="button" class="chip" class:active={look.bold} style="font-weight:700" onclick={() => setTextLook({ bold: !look.bold })}>Bold</button>
              <button type="button" class="chip" class:active={look.italic} style="font-style:italic" onclick={() => setTextLook({ italic: !look.italic })}
                >Italic</button
              >
            </div>
          </section>
        {/if}
        {#if selectedShape}
          <section>
            <h3>Selection</h3>
            <div class="column">
              <button type="button" onclick={duplicate}>Duplicate <Kbd keys="Ctrl+D" /></button>
              <button type="button" onclick={removeSelected}>Delete <Kbd keys="Del" /></button>
            </div>
            <p class="note">Drag to move · handles to resize · arrows nudge (<Kbd keys="Shift" /> ×10)</p>
          </section>
        {:else}
          <section class="tips">
            <h3>Tips</h3>
            <p class="note"><Kbd keys="Shift" /> while drawing: straight angles, squares, circles.</p>
            <p class="note"><Kbd keys="Ctrl+wheel" /> zoom · <Kbd keys="Space" /> + drag to move around.</p>
            <p class="note">Double-click a text to edit it.</p>
          </section>
        {/if}
      {/if}
    </aside>
  </div>
</div>

<style>
  /* Same layout as the video editor: top bar, tool rail, floating panel,
     picture (docs/UI_DESIGN.md). */
  .editor {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg);
  }
  .topbar {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 16px;
    height: 60px;
    padding: 0 16px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .tb-side {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .tb-side.right {
    justify-content: flex-end;
    gap: 8px;
  }
  .title {
    display: flex;
    align-items: baseline;
    gap: 10px;
    max-width: 40vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 15px;
    font-weight: 650;
  }
  .title small {
    color: var(--text-faint);
    font-size: 13px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 72px clamp(240px, 22vw, 300px) minmax(0, 1fr);
    grid-template-areas: "rail panel stage";
  }
  .tools {
    grid-area: rail;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 14px 0;
    overflow-y: auto;
  }
  .tool-group {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .tool-group + .tool-group {
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .heading {
    display: none;
  }
  .tool-grid {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tool {
    position: relative;
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    padding: 0;
    border-radius: 14px;
    background: transparent;
    color: var(--text-muted);
  }
  .tool.active {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent-text);
  }
  .key {
    position: absolute;
    right: 4px;
    bottom: 2px;
    font-size: 9px;
    font-weight: 700;
    opacity: 0.45;
  }
  .panel {
    grid-area: panel;
    margin: 14px 0;
    padding: 22px 20px;
    overflow-y: auto;
    border-radius: 20px;
    background: var(--surface);
    box-shadow: var(--shadow-soft);
  }
  .panel section {
    margin-bottom: 26px;
  }
  .panel button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .heading,
  h3 {
    margin: 0 0 12px;
    font-size: 12px;
    font-weight: 650;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .row,
  .swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .fonts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    margin-bottom: 8px;
  }
  .fonts .chip {
    justify-content: center;
    font-size: 15px;
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .column button {
    justify-content: space-between;
    border-radius: 10px;
  }
  .note {
    margin: 8px 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  button.icon {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border-radius: 10px;
    background: transparent;
    font-size: 18px;
  }
  button.chip {
    padding: 6px 12px;
    border-radius: 10px;
  }
  button.chip.active {
    background: var(--accent);
    color: var(--on-accent);
  }
  button.action {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 38px;
    padding: 0 14px;
    border-radius: 10px;
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  button.action.primary {
    min-width: 120px;
    height: 42px;
    justify-content: center;
    border-radius: 12px;
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--accent) 35%, transparent);
    font-size: 15px;
  }
  button.action.primary:hover {
    background: var(--accent-hover);
  }
  button.link {
    border: none;
    background: none;
    color: var(--accent-text);
    padding: 0 0 0 8px;
  }
  button.danger {
    color: var(--danger);
  }
  .swatch {
    width: 26px;
    height: 26px;
    padding: 0;
    border-radius: 50%;
    background: var(--swatch);
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.45);
  }
  .swatch:hover:not(:disabled) {
    background: var(--swatch);
  }
  .swatch.wide {
    width: 44px;
    height: 44px;
    border-radius: 12px;
  }
  .swatch.active {
    outline: 2.5px solid var(--accent);
    outline-offset: 2px;
  }
  .custom input {
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
  }
  .dot {
    width: var(--d);
    height: var(--d);
    border-radius: 50%;
    background: currentColor;
  }
  .check,
  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 0;
    font-size: 13px;
  }
  .field input {
    flex: 1;
  }
  .zoom .chip {
    min-width: 60px;
    justify-content: center;
    font-variant-numeric: tabular-nums;
    background: transparent;
  }
  .stage {
    grid-area: stage;
    position: relative;
    min-width: 0;
    margin: 14px 18px 18px;
    overflow: hidden;
    border-radius: 18px;
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 96%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 20px
      20px;
  }
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    touch-action: none;
  }
  .text-input {
    position: absolute;
    min-width: 40px;
    padding: 0;
    margin: 0;
    border: 1px dashed var(--accent);
    background: transparent;
    line-height: 1.25;
    resize: none;
    overflow: hidden;
    field-sizing: content;
    outline: none;
  }
  .message {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    margin: 0;
    color: var(--text-muted);
  }
  .toast,
  .hint {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    padding: 8px 16px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow-soft);
    font-weight: 550;
  }
  .toast {
    bottom: 20px;
  }
  .hint {
    top: 14px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.45);
  }
  .dialog {
    width: min(440px, calc(100vw - 32px));
    padding: 26px 28px;
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow);
  }
  .dialog h2 {
    margin: 0 0 8px;
    font-size: 20px;
  }
  .dialog p {
    margin: 0 0 22px;
    color: var(--text-muted);
  }
  .dialog-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
