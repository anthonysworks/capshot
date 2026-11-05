<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import {
    type Composition,
    type Config,
    type CursorTrack,
    type JobProgress,
    type VideoEdit,
    GIF_FPS_PRESETS,
    GIF_WIDTHS,
    editorCopyFile,
    editorExport,
    editorVideoSources,
    getConfig,
    onJobProgress,
    showInFolder,
  } from "../lib/api";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import Kbd from "./Kbd.svelte";
  import { CLICK_MARK, CURSOR_STYLES, type CursorStyle, cursorAt, drawCursor, loadShapes } from "./cursors";
  import {
    BACKGROUNDS,
    FREEHAND,
    LINEAR,
    type Point,
    type Shape,
    type ShapeKind,
    bounds,
    fontSize,
    hit,
    renderAnnotations,
  } from "./shapes";
  import {
    SPEEDS,
    type Segment,
    type Trim,
    format,
    indexAt,
    label,
    nextKept,
    outputLength,
    partAt,
    parts,
    rulerStep,
    setSpeed,
    snap,
    speedLabel,
    split,
    toggle,
    whole,
  } from "./timeline";

  let { path }: { path: string } = $props();

  type Tool = "select" | "text" | "arrow" | "line" | "rect" | "ellipse" | "highlight" | "pen" | "number";
  const TOOLS: { tool: Tool; icon: IconName; label: string; key: string }[] = [
    { tool: "select", icon: "select", label: "Select, move — click the video to play", key: "V" },
    { tool: "text", icon: "text", label: "Text", key: "T" },
    { tool: "arrow", icon: "arrow", label: "Arrow", key: "A" },
    { tool: "line", icon: "line", label: "Line", key: "L" },
    { tool: "rect", icon: "rect", label: "Rectangle", key: "R" },
    { tool: "ellipse", icon: "ellipse", label: "Ellipse", key: "E" },
    { tool: "pen", icon: "pen", label: "Pencil", key: "P" },
    { tool: "highlight", icon: "highlight", label: "Highlighter", key: "H" },
    { tool: "number", icon: "number", label: "Numbered step", key: "N" },
  ];
  const COLORS = ["#e5484d", "#f5a524", "#ffd60a", "#30a46c", "#3b82f6", "#ffffff", "#111111"];
  const LEVELS = [
    { level: 1, label: "Thin" },
    { level: 2, label: "Medium" },
    { level: 4, label: "Thick" },
  ];
  const QUALITIES = [
    { value: 18, label: "High" },
    { value: 23, label: "Balanced" },
    { value: 28, label: "Small" },
  ];
  const RATIOS: { label: string; ratio: [number, number] | null }[] = [
    { label: "Original", ratio: null },
    { label: "16:9", ratio: [16, 9] },
    { label: "9:16", ratio: [9, 16] },
    { label: "1:1", ratio: [1, 1] },
    { label: "4:5", ratio: [4, 5] },
    { label: "4:3", ratio: [4, 3] },
  ];
  /** Thumbnails per 100 px of timeline, and the most ever made. */
  const FILMSTRIP_MAX = 40;
  /** How long a new text or shape stays on screen by default, in seconds. */
  const OVERLAY_LENGTH = 3;
  /** Distance under which handles snap, in pixels. */
  const SNAP_PX = 8;
  const MAX_ZOOM = 40;

  interface Overlay {
    id: number;
    shape: Shape;
    start: number;
    end: number;
  }

  let video: HTMLVideoElement;
  let player: HTMLDivElement;
  let preview: HTMLCanvasElement;
  let scroller: HTMLDivElement;
  let content: HTMLDivElement;

  let duration = $state(0);
  let time = $state(0);
  let playing = $state(false);
  let segments = $state<Segment[]>([]);
  let trim = $state<Trim>({ start: 0, end: 0 });
  let selectedSegment = $state<number | null>(null);
  let audio = $state(true);
  let quality = $state(18);
  let crop = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  let cropping = $state(false);
  let cropDraft = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  let ratio = $state<[number, number] | null>(null);
  let background = $state(0);
  let margin = $state(0.05);
  let cursorVisible = $state(true);
  let cursorScale = $state(1);
  let cursorStyle = $state<CursorStyle>("original");
  let cursorColor = $state("#e5484d");
  let halo = $state(false);
  let haloColor = $state("#ffd60a");
  let clickRing = $state(false);
  let clickColor = $state("#ffd60a");
  let clickSound = $state(false);
  let overlays = $state<Overlay[]>([]);
  let selectedOverlay = $state<number | null>(null);
  let tool = $state<Tool>("select");
  let color = $state("#e5484d");
  let level = $state(2);
  let frames = $state<string[]>([]);
  let error = $state("");
  let toast = $state("");
  let config = $state<Config | null>(null);
  let gifFps = $state(15);
  let gifWidth = $state(0);
  let jobs = $state<(JobProgress & { kind: string })[]>([]);
  let past = $state<string[]>([]);
  let future = $state<string[]>([]);
  let cleanSource = $state<string | null>(null);
  let cursorTrack = $state<CursorTrack | null>(null);
  let shapes: { image: HTMLImageElement; hotspot: [number, number] }[] = [];
  let editing = $state<{ x: number; y: number; text: string; id: number | null } | null>(null);
  /** Displayed picture inside the player, in CSS pixels. */
  let box = $state({ left: 0, top: 0, width: 0, height: 0, scale: 1 });
  let draft = $state<Shape | null>(null);
  let zoom = $state(1);
  let viewWidth = $state(800);
  let menu = $state<"export" | "speed" | null>(null);
  let nextId = 1;
  let drag:
    | { kind: "time" }
    | { kind: "trim"; edge: "start" | "end" }
    | { kind: "crop"; start: Point }
    | { kind: "draw"; start: Point; moved: boolean }
    | { kind: "move"; last: Point; moved: boolean }
    | { kind: "bar"; id: number; edge: "start" | "end" | "both"; from: number; start: number; end: number }
    | null = null;

  const source = $derived(convertFileSrc(cleanSource ?? path));
  const kept = $derived(parts(segments, trim));
  const output = $derived(outputLength(kept));
  /** The cursor can only be changed with the recording's copy without cursor. */
  const cursorEditable = $derived(!!cleanSource && !!cursorTrack);
  /** Annotation sizes follow the video size, like in the image editor. */
  let longSide = $state(0);
  const unit = $derived(Math.max(1, Math.round(longSide / 1400)));
  const contentWidth = $derived(viewWidth * zoom);
  const pxPerSecond = $derived(duration ? contentWidth / duration : 1);
  const step = $derived(rulerStep(pxPerSecond));
  const ticks = $derived.by(() => {
    if (!duration) return [];
    const out: number[] = [];
    for (let t = 0; t <= duration + 1e-9 && out.length < 2000; t += step) out.push(t);
    return out;
  });
  const selected = $derived(selectedSegment === null ? null : (segments[selectedSegment] ?? null));
  const selectedItem = $derived(overlays.find((o) => o.id === selectedOverlay) ?? null);

  onMount(() => {
    getConfig().then((c) => {
      config = c;
      gifFps = c.gif.fps;
      gifWidth = c.gif.max_width;
    });
    editorVideoSources(path)
      .then(async (s) => {
        cursorTrack = s.track;
        if (s.track) shapes = await loadShapes(s.track);
        cleanSource = s.clean;
      })
      .catch(() => {});
    const unlisten = onJobProgress((p) => {
      const i = jobs.findIndex((j) => j.job === p.job);
      const old = jobs[i];
      if (old) jobs[i] = { ...p, kind: old.kind };
      if (old && p.output && p.output !== old.output) notify(old.kind === "copy" ? "Copied to the clipboard" : "Exported");
    });
    const observer = new ResizeObserver(() => {
      measure();
      viewWidth = scroller?.clientWidth || viewWidth;
    });
    observer.observe(player);
    // The format (aspect ratio) resizes the video inside the player.
    observer.observe(video);
    observer.observe(scroller);
    let frame = requestAnimationFrame(function loop() {
      follow();
      paint();
      frame = requestAnimationFrame(loop);
    });
    const closeMenu = (e: PointerEvent) => {
      if (!(e.target as HTMLElement)?.closest?.(".popover, .opens-menu")) menu = null;
    };
    window.addEventListener("pointerdown", closeMenu);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("pointerdown", closeMenu);
      unlisten.then((f) => f());
    };
  });

  function loaded() {
    longSide = Math.max(video.videoWidth, video.videoHeight);
    if (!segments.length) {
      duration = video.duration;
      segments = whole(duration);
      trim = { start: 0, end: duration };
      filmstrip();
    }
    measure();
  }

  /** Where the picture is drawn inside the video element (letterboxed). */
  function measure() {
    if (!video?.videoWidth) return;
    const r = video.getBoundingClientRect();
    const p = player.getBoundingClientRect();
    const scale = Math.min(r.width / video.videoWidth, r.height / video.videoHeight);
    const width = video.videoWidth * scale;
    const height = video.videoHeight * scale;
    box = {
      left: r.left - p.left + (r.width - width) / 2,
      top: r.top - p.top + (r.height - height) / 2,
      width,
      height,
      scale,
    };
  }

  /** Thumbnails along the timeline, from a second, hidden player. */
  async function filmstrip() {
    const probe = document.createElement("video");
    probe.src = convertFileSrc(path);
    probe.muted = true;
    probe.preload = "auto";
    await new Promise((resolve) => probe.addEventListener("loadeddata", resolve, { once: true }));
    const canvas = document.createElement("canvas");
    canvas.height = 56;
    canvas.width = Math.round((56 * probe.videoWidth) / Math.max(1, probe.videoHeight));
    const ctx = canvas.getContext("2d");
    const count = Math.min(FILMSTRIP_MAX, Math.max(8, Math.ceil(duration)));
    const out: string[] = [];
    for (let i = 0; i < count && ctx; i++) {
      probe.currentTime = ((i + 0.5) * duration) / count;
      await new Promise((resolve) => probe.addEventListener("seeked", resolve, { once: true }));
      ctx.drawImage(probe, 0, 0, canvas.width, canvas.height);
      out.push(canvas.toDataURL("image/jpeg", 0.7));
      frames = [...out];
    }
    probe.removeAttribute("src");
    probe.load();
  }

  // ── Preview ────────────────────────────────────────────────────────────

  function hexRgb(hex: string): [number, number, number] {
    const h = hex.replace("#", "");
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) || 0) as [number, number, number];
  }

  /** Overlays, cursor and click marks drawn over the playing video. */
  function paint() {
    if (!preview || !video) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(box.width * dpr);
    const h = Math.round(box.height * dpr);
    if (preview.width !== w || preview.height !== h) {
      preview.width = w;
      preview.height = h;
    }
    const ctx = preview.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const s = box.scale * dpr;
    ctx.setTransform(s, 0, 0, s, 0, 0);
    const t = video.currentTime;

    const active = overlays.filter((o) => t >= o.start && t < o.end && o.id !== editing?.id).map((o) => o.shape);
    renderAnnotations(ctx, draft ? [...active, draft] : active);
    if (selectedItem && t >= selectedItem.start && t < selectedItem.end) outline(ctx, bounds(ctx, selectedItem.shape), s);

    if (cursorTrack && (clickRing || cursorEditable)) {
      const scale = cursorScale;
      if (clickRing) {
        const [r, g, b] = hexRgb(clickColor);
        for (const c of cursorTrack.clicks) {
          if (c.t > t || t - c.t >= CLICK_MARK) continue;
          const p = (t - c.t) / CLICK_MARK;
          const eased = 1 - (1 - p) ** 3;
          ctx.strokeStyle = `rgba(${r},${g},${b},${(1 - p) * 0.9})`;
          ctx.lineWidth = 3 * Math.max(1, scale);
          ctx.beginPath();
          ctx.arc(c.x, c.y, (6 + 26 * eased) * Math.max(1, scale), 0, Math.PI * 2);
          ctx.stroke();
        }
      }
      // With the copy without cursor playing, the cursor is drawn here.
      const at = cursorEditable && cursorVisible ? cursorAt(cursorTrack, t) : null;
      if (at) {
        if (halo) {
          const [r, g, b] = hexRgb(haloColor);
          ctx.fillStyle = `rgba(${r},${g},${b},0.35)`;
          ctx.beginPath();
          ctx.arc(at.x, at.y, 22 * Math.max(1, scale), 0, Math.PI * 2);
          ctx.fill();
        }
        const custom = cursorStyle !== "original" ? drawCursor(cursorStyle, cursorColor) : null;
        const img = custom?.canvas ?? shapes[at.shape]?.image;
        const hot = custom?.hotspot ?? shapes[at.shape]?.hotspot ?? [0, 0];
        if (img && (custom || (img as HTMLImageElement).complete)) {
          const iw = (img as HTMLImageElement).naturalWidth || img.width;
          const ih = (img as HTMLImageElement).naturalHeight || img.height;
          ctx.drawImage(img, at.x - hot[0] * scale, at.y - hot[1] * scale, iw * scale, ih * scale);
        }
      }
    }

    const area = cropDraft ?? crop;
    if (area) {
      ctx.fillStyle = cropping ? "rgba(0,0,0,0.55)" : "rgba(0,0,0,0.65)";
      const vw = video.videoWidth;
      const vh = video.videoHeight;
      ctx.fillRect(0, 0, vw, area.y);
      ctx.fillRect(0, area.y + area.h, vw, vh - area.y - area.h);
      ctx.fillRect(0, area.y, area.x, area.h);
      ctx.fillRect(area.x + area.w, area.y, vw - area.x - area.w, area.h);
      outline(ctx, area, s);
    }
  }

  function outline(ctx: CanvasRenderingContext2D, r: { x: number; y: number; w: number; h: number }, s: number) {
    const px = 1 / s;
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

  // ── Playback ───────────────────────────────────────────────────────────

  /** Every frame while playing: skip removed parts, stop at the out point,
   * play each part at its speed, keep the playhead in view. */
  function follow() {
    if (!video) return;
    time = video.currentTime;
    if (!playing) return;
    const next = nextKept(kept, time);
    if (next === null) {
      video.pause();
      video.currentTime = trim.end;
      return;
    }
    if (next > time + 0.01) video.currentTime = next;
    const rate = partAt(kept, next)?.speed ?? 1;
    if (video.playbackRate !== rate) video.playbackRate = rate;
    if (scroller && zoom > 1) {
      const x = time * pxPerSecond;
      if (x < scroller.scrollLeft || x > scroller.scrollLeft + scroller.clientWidth - 40) {
        scroller.scrollLeft = x - scroller.clientWidth * 0.2;
      }
    }
  }

  function togglePlay() {
    if (video.paused) {
      if (nextKept(kept, video.currentTime) === null || video.currentTime >= trim.end - 0.02) {
        video.currentTime = kept[0]?.start ?? 0;
      }
      video.play();
    } else {
      video.pause();
    }
  }

  function seek(t: number) {
    video.currentTime = Math.max(0, Math.min(duration, t));
    time = video.currentTime;
  }

  // Swapping to the copy without cursor keeps the position.
  $effect(() => {
    void source;
    const at = time;
    if (video && cleanSource) {
      video.addEventListener("loadedmetadata", () => (video.currentTime = at), { once: true });
    }
  });

  // ── History ────────────────────────────────────────────────────────────

  const snapshot = () =>
    JSON.stringify({
      segments: $state.snapshot(segments),
      trim: $state.snapshot(trim),
      crop: $state.snapshot(crop),
      overlays: $state.snapshot(overlays),
    });

  function commit(change: () => void) {
    past.push(snapshot());
    future = [];
    change();
  }

  function restore(from: string[], to: string[]) {
    const state = from.pop();
    if (state === undefined) return;
    to.push(snapshot());
    const parsed = JSON.parse(state);
    segments = parsed.segments;
    trim = parsed.trim;
    crop = parsed.crop;
    overlays = parsed.overlays;
    selectedOverlay = null;
    selectedSegment = null;
  }

  const undo = () => restore(past, future);
  const redo = () => restore(future, past);

  // ── Cutting ────────────────────────────────────────────────────────────

  function splitHere() {
    commit(() => (segments = split(segments, time)));
    selectedSegment = indexAt(segments, time);
  }

  function removePart(i = selectedSegment ?? indexAt(segments, time)) {
    commit(() => (segments = toggle(segments, i)));
    selectedSegment = i;
  }

  function speedPart(speed: number, i = selectedSegment ?? indexAt(segments, time)) {
    menu = null;
    if (segments[i]?.speed === speed) return;
    commit(() => (segments = setSpeed(segments, i, speed)));
    selectedSegment = i;
  }

  const allSpeeds = (speed: number) => {
    menu = null;
    commit(() => (segments = segments.map((s) => ({ ...s, speed }))));
  };

  function trimTo(edge: "start" | "end", t: number) {
    if (edge === "start") trim.start = Math.max(0, Math.min(t, trim.end - 0.1));
    else trim.end = Math.min(duration, Math.max(t, trim.start + 0.1));
  }

  /** Cut points, trim and playhead: where dragged things snap to. */
  function snapTargets(exclude?: number) {
    const out = [0, duration, trim.start, trim.end, ...segments.map((s) => s.start)];
    if (!drag || drag.kind !== "time") out.push(time);
    for (const o of overlays) if (o.id !== exclude) out.push(o.start, o.end);
    return out;
  }

  /** Time under the pointer on the timeline. */
  function timeAt(e: PointerEvent | WheelEvent | MouseEvent) {
    const r = content.getBoundingClientRect();
    return (Math.max(0, Math.min(r.width, e.clientX - r.left)) / r.width) * duration;
  }

  const reach = $derived(SNAP_PX / Math.max(1e-6, pxPerSecond));

  function scrubDown(e: PointerEvent) {
    if (e.button !== 0) return;
    drag = { kind: "time" };
    content.setPointerCapture(e.pointerId);
    const t = timeAt(e);
    seek(t);
    selectedOverlay = null;
    const i = indexAt(segments, t);
    selectedSegment = (e.target as HTMLElement).closest(".track") ? i : selectedSegment;
  }

  function trimDown(e: PointerEvent, edge: "start" | "end") {
    e.stopPropagation();
    if (e.button !== 0) return;
    content.setPointerCapture(e.pointerId);
    past.push(snapshot());
    future = [];
    video.pause();
    drag = { kind: "trim", edge };
  }

  /** Dragging a bar of the overlay lane changes when it shows. */
  function barDown(e: PointerEvent, o: Overlay, edge: "start" | "end" | "both") {
    e.stopPropagation();
    if (e.button !== 0) return;
    content.setPointerCapture(e.pointerId);
    selectedOverlay = o.id;
    selectedSegment = null;
    past.push(snapshot());
    future = [];
    drag = { kind: "bar", id: o.id, edge, from: timeAt(e), start: o.start, end: o.end };
  }

  function timelineMove(e: PointerEvent) {
    if (!drag) return;
    const free = e.altKey;
    const at = (t: number, exclude?: number) => (free ? t : snap(t, snapTargets(exclude), reach));
    if (drag.kind === "time") {
      seek(timeAt(e));
    } else if (drag.kind === "trim") {
      trimTo(drag.edge, at(timeAt(e)));
      seek(drag.edge === "start" ? trim.start : trim.end - 0.001);
    } else if (drag.kind === "bar") {
      const d = drag;
      const o = overlays.find((x) => x.id === d.id);
      if (!o) return;
      const delta = timeAt(e) - d.from;
      if (d.edge === "start") o.start = Math.max(0, Math.min(d.end - 0.1, at(d.start + delta, o.id)));
      else if (d.edge === "end") o.end = Math.min(duration, Math.max(d.start + 0.1, at(d.end + delta, o.id)));
      else {
        const length = d.end - d.start;
        const start = at(d.start + delta, o.id);
        // Snap either end.
        const end = at(start + length, o.id);
        o.start = Math.max(0, Math.min(duration - length, end !== start + length ? end - length : start));
        o.end = o.start + length;
      }
      seek(d.edge === "end" ? o.end - 0.01 : o.start);
    }
  }

  function timelineUp() {
    if (drag?.kind === "time" || drag?.kind === "trim" || drag?.kind === "bar") drag = null;
  }

  /** Ctrl+wheel zooms around the pointer; the wheel alone scrolls. */
  function wheel(e: WheelEvent) {
    if (!duration) return;
    if (e.ctrlKey) {
      e.preventDefault();
      zoomBy(e.deltaY < 0 ? 1.25 : 0.8, e);
    } else if (zoom > 1 && Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
      e.preventDefault();
      scroller.scrollLeft += e.deltaY;
    }
  }

  function zoomBy(factor: number, around?: MouseEvent) {
    const anchor = around ? timeAt(around) : time;
    const offset = around ? around.clientX - scroller.getBoundingClientRect().left : scroller.clientWidth / 2;
    zoom = Math.max(1, Math.min(MAX_ZOOM, zoom * factor));
    requestAnimationFrame(() => {
      scroller.scrollLeft = anchor * (contentWidth / duration) - offset;
    });
  }

  // ── Overlays ───────────────────────────────────────────────────────────

  function toVideo(e: { clientX: number; clientY: number }): Point {
    const r = player.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(video.videoWidth, (e.clientX - r.left - box.left) / box.scale)),
      y: Math.max(0, Math.min(video.videoHeight, (e.clientY - r.top - box.top) / box.scale)),
    };
  }

  function make(kind: ShapeKind, p: Point, extra: Partial<Shape> = {}): Shape {
    return {
      id: nextId++,
      kind,
      color,
      size: kind === "text" ? (level + 1) * unit : (level + 2) * unit,
      fill: kind === "text" ? "soft" : "none",
      x1: p.x,
      y1: p.y,
      x2: p.x,
      y2: p.y,
      ...extra,
    };
  }

  function addOverlay(shape: Shape) {
    const start = Math.min(time, Math.max(0, duration - 0.1));
    const overlay = { id: shape.id, shape, start, end: Math.min(duration, start + OVERLAY_LENGTH) };
    commit(() => overlays.push(overlay));
    selectedOverlay = overlay.id;
    selectedSegment = null;
  }

  function topmost(p: Point): Overlay | undefined {
    const ctx = preview.getContext("2d");
    if (!ctx) return undefined;
    return [...overlays]
      .reverse()
      .find((o) => time >= o.start && time < o.end && hit(ctx, o.shape, p, 8 / box.scale));
  }

  function chooseTool(t: Tool) {
    tool = t;
    cropping = false;
    if (t !== "select") video?.pause();
  }

  function stageDown(e: PointerEvent) {
    if (!video?.videoWidth || e.button !== 0) return;
    e.preventDefault();
    menu = null;
    if (editing) return commitText();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const p = toVideo(e);
    if (cropping) {
      drag = { kind: "crop", start: p };
      cropDraft = { ...p, w: 0, h: 0 };
      return;
    }
    if (tool === "select") {
      const found = topmost(p);
      selectedOverlay = found?.id ?? null;
      if (found) {
        video.pause();
        selectedSegment = null;
        past.push(snapshot());
        drag = { kind: "move", last: p, moved: false };
      } else {
        togglePlay();
      }
      return;
    }
    video.pause();
    if (tool === "text") {
      const found = topmost(p);
      editing =
        found?.shape.kind === "text"
          ? { x: found.shape.x1, y: found.shape.y1, text: found.shape.text ?? "", id: found.id }
          : { x: p.x, y: p.y, text: "", id: null };
      return;
    }
    if (tool === "number") {
      const n = Math.max(0, ...overlays.map((o) => o.shape.n ?? 0)) + 1;
      addOverlay(make("number", p, { n }));
      return;
    }
    drag = { kind: "draw", start: p, moved: false };
    draft = make(tool, p, FREEHAND.includes(tool) ? { points: [p] } : {});
  }

  function stageMove(e: PointerEvent) {
    if (!drag) return;
    const p = toVideo(e);
    if (drag.kind === "crop") {
      const s = drag.start;
      cropDraft = { x: Math.min(p.x, s.x), y: Math.min(p.y, s.y), w: Math.abs(p.x - s.x), h: Math.abs(p.y - s.y) };
    } else if (drag.kind === "move") {
      const o = selectedItem;
      if (!o) return;
      const dx = p.x - drag.last.x;
      const dy = p.y - drag.last.y;
      drag.moved ||= dx !== 0 || dy !== 0;
      o.shape.x1 += dx;
      o.shape.x2 += dx;
      o.shape.y1 += dy;
      o.shape.y2 += dy;
      if (o.shape.points) o.shape.points = o.shape.points.map((q) => ({ x: q.x + dx, y: q.y + dy }));
      drag.last = p;
    } else if (drag.kind === "draw" && draft) {
      drag.moved ||= Math.hypot(p.x - drag.start.x, p.y - drag.start.y) * box.scale > 3;
      if (FREEHAND.includes(draft.kind)) draft.points = [...(draft.points ?? []), p];
      else if (e.shiftKey && LINEAR.includes(draft.kind)) {
        const angle = Math.round(Math.atan2(p.y - draft.y1, p.x - draft.x1) / (Math.PI / 4)) * (Math.PI / 4);
        const length = Math.hypot(p.x - draft.x1, p.y - draft.y1);
        draft.x2 = draft.x1 + Math.cos(angle) * length;
        draft.y2 = draft.y1 + Math.sin(angle) * length;
      } else {
        draft.x2 = p.x;
        draft.y2 = p.y;
      }
    }
  }

  function stageUp() {
    const current = drag;
    if (!current || current.kind === "time" || current.kind === "trim" || current.kind === "bar") return;
    drag = null;
    if (current.kind === "crop") {
      const area = cropDraft;
      cropDraft = null;
      if (area && area.w > 8 && area.h > 8) {
        commit(() => (crop = { x: Math.round(area.x), y: Math.round(area.y), w: Math.round(area.w), h: Math.round(area.h) }));
        cropping = false;
      }
    } else if (current.kind === "move") {
      if (!current.moved) past.pop();
      else future = [];
    } else if (current.kind === "draw" && draft) {
      const shape = draft;
      draft = null;
      if (current.moved) addOverlay(shape);
    }
  }

  function stageDouble(e: MouseEvent) {
    const found = topmost(toVideo(e));
    if (found?.shape.kind === "text") {
      video.pause();
      selectedOverlay = found.id;
      editing = { x: found.shape.x1, y: found.shape.y1, text: found.shape.text ?? "", id: found.id };
    }
  }

  function focusNow(node: HTMLTextAreaElement) {
    node.focus();
  }

  function commitText() {
    if (!editing) return;
    const { x, y, text, id } = editing;
    editing = null;
    const trimmed = text.replace(/\s+$/, "");
    const existing = overlays.find((o) => o.id === id);
    if (existing) {
      if (existing.shape.text === trimmed) return;
      commit(() => {
        if (trimmed) existing.shape.text = trimmed;
        else overlays = overlays.filter((o) => o.id !== id);
      });
    } else if (trimmed) {
      addOverlay(make("text", { x, y }, { text: trimmed }));
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

  const textStyle = $derived.by(() => {
    if (!editing) return "";
    const size = fontSize((level + 1) * unit) * box.scale;
    return `left:${box.left + editing.x * box.scale}px;top:${box.top + editing.y * box.scale}px;font-size:${size}px;color:${color}`;
  });

  function removeOverlay() {
    const id = selectedOverlay;
    if (id === null) return;
    selectedOverlay = null;
    commit(() => (overlays = overlays.filter((o) => o.id !== id)));
  }

  function setOverlayTime(edge: "start" | "end") {
    const o = selectedItem;
    if (!o) return;
    commit(() => {
      if (edge === "start") o.start = Math.min(time, o.end - 0.1);
      else o.end = Math.max(time, o.start + 0.1);
    });
  }

  /** Toolbox colour and thickness also apply to the selected item. */
  function setColor(c: string) {
    color = c;
    const o = selectedItem;
    if (o && o.shape.color !== c) commit(() => (o.shape.color = c));
  }

  function setLevel(l: number) {
    level = l;
    const o = selectedItem;
    if (!o) return;
    const size = o.shape.kind === "text" ? (l + 1) * unit : (l + 2) * unit;
    if (o.shape.size !== size) commit(() => (o.shape.size = size));
  }

  // ── Export ─────────────────────────────────────────────────────────────

  /** Each overlay as a PNG of its own area, in video pixels. */
  function overlayImages(): Composition["overlays"] {
    const out: Composition["overlays"] = [];
    const scratch = document.createElement("canvas");
    const sctx = scratch.getContext("2d");
    if (!sctx) return out;
    for (const o of overlays) {
      const b = bounds(sctx, o.shape);
      const pad = 8 + o.shape.size * 3;
      const x = Math.floor(b.x - pad);
      const y = Math.floor(b.y - pad);
      const canvas = document.createElement("canvas");
      canvas.width = Math.ceil(b.w + 2 * pad);
      canvas.height = Math.ceil(b.h + 2 * pad);
      const ctx = canvas.getContext("2d");
      if (!ctx) continue;
      ctx.translate(-x, -y);
      renderAnnotations(ctx, [$state.snapshot(o.shape) as Shape]);
      out.push({ png: canvas.toDataURL("image/png"), x, y, start: o.start, end: o.end });
    }
    return out;
  }

  function composition(): Composition {
    const customCursor = cursorEditable && (cursorStyle !== "original" || cursorScale !== 1 || !cursorVisible || halo);
    let image: { png: string; hotspot: [number, number] } | null = null;
    if (customCursor && cursorStyle !== "original") {
      const drawn = drawCursor(cursorStyle, cursorColor);
      image = { png: drawn.canvas.toDataURL("image/png"), hotspot: drawn.hotspot };
    }
    const bg = BACKGROUNDS[background]?.stops ?? ["#222", "#111"];
    return {
      overlays: overlayImages(),
      cursor: customCursor
        ? { visible: cursorVisible, scale: cursorScale, image, halo: halo ? [...hexRgb(haloColor), 90] : null }
        : null,
      clicks: clickRing || clickSound ? { ring: clickRing, color: hexRgb(clickColor), sound: clickSound } : null,
      canvas: ratio ? { ratio: [ratio[0], ratio[1]], colors: bg.map(hexRgb), padding: margin } : null,
    };
  }

  function edit(): VideoEdit {
    return {
      segments: kept.map((p) => [p.start, p.end]),
      speed: 1,
      speeds: kept.map((p) => p.speed),
      crop: crop ? [crop.x, crop.y, crop.w, crop.h] : null,
      composition: composition(),
    };
  }

  /** Nothing changed: the file can be copied as it is. */
  const untouched = $derived(
    kept.length === 1 &&
      kept[0]!.speed === 1 &&
      kept[0]!.end - kept[0]!.start >= duration - 0.01 &&
      !crop &&
      !ratio &&
      !overlays.length &&
      !clickRing &&
      !clickSound &&
      cursorVisible &&
      cursorScale === 1 &&
      cursorStyle === "original" &&
      !halo,
  );

  async function exportAs(kind: "mp4" | "gif", copy = false) {
    menu = null;
    if (!config || !kept.length) return;
    try {
      const job = await editorExport(
        path,
        kind === "mp4"
          ? { kind, edit: edit(), audio, quality }
          : { kind, gif: { ...$state.snapshot(config.gif), fps: gifFps, max_width: gifWidth }, edit: edit() },
        copy,
      );
      jobs.unshift({ job, kind: copy ? "copy" : kind, input: path, done: 0, output: null, bytes: null, error: null });
    } catch (e) {
      error = String(e);
    }
  }

  async function copyVideo() {
    if (untouched) {
      await editorCopyFile(path);
      notify("Copied to the clipboard");
    } else {
      notify("Preparing the edited video… it will be copied when ready");
      await exportAs("mp4", true);
    }
  }

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function notify(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 3000);
  }

  // ── Keyboard ───────────────────────────────────────────────────────────

  function keyDown(e: KeyboardEvent) {
    if (editing) return;
    const target = e.target as HTMLElement;
    if (target?.tagName === "SELECT" || (target?.tagName === "INPUT" && (target as HTMLInputElement).type !== "checkbox"))
      return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (ctrl) {
      if (key === "z") (e.shiftKey ? redo : undo)();
      else if (key === "y") redo();
      else if (key === "c") copyVideo();
      else if (key === "e") exportAs("mp4");
      else if (key === "=" || key === "+") zoomBy(1.5);
      else if (key === "-") zoomBy(1 / 1.5);
      else if (key === "0") zoom = 1;
      else return;
      e.preventDefault();
      return;
    }
    const step = e.shiftKey ? 1 : 1 / 30;
    const toolKey = TOOLS.find((t) => t.key.toLowerCase() === key);
    if (toolKey) {
      chooseTool(toolKey.tool);
      return;
    }
    switch (key) {
      case " ":
        e.preventDefault();
        togglePlay();
        break;
      case "arrowleft":
        seek(time - step);
        break;
      case "arrowright":
        seek(time + step);
        break;
      case "home":
        seek(trim.start);
        break;
      case "end":
        seek(trim.end - 0.001);
        break;
      case "s":
        splitHere();
        break;
      case "delete":
      case "backspace":
        if (selectedOverlay !== null) removeOverlay();
        else removePart();
        break;
      case "i":
        commit(() => trimTo("start", time));
        break;
      case "o":
        commit(() => trimTo("end", time));
        break;
      case "c":
        cropping = !cropping;
        if (cropping) video.pause();
        break;
      case "escape":
        cropping = false;
        selectedOverlay = null;
        selectedSegment = null;
        draft = null;
        menu = null;
        break;
    }
  }

  const percent = (t: number) => (duration ? (t / duration) * 100 : 0);
  const size = (bytes: number) =>
    bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const stageCursor = $derived(
    cropping ? "crosshair" : tool === "select" ? "default" : tool === "text" ? "text" : "crosshair",
  );
  const frameStyle = $derived.by(() => {
    if (!ratio) return "";
    const stops = BACKGROUNDS[background]?.stops ?? ["#222", "#111"];
    return `aspect-ratio:${ratio[0]}/${ratio[1]};background:linear-gradient(135deg,${stops.join(",")});padding:${margin * 100}%`;
  });
  const resultSize = $derived.by(() => {
    if (!video?.videoWidth && !crop) return "";
    const w = crop?.w ?? video?.videoWidth ?? 0;
    const h = crop?.h ?? video?.videoHeight ?? 0;
    return w && h ? `${w & ~1}×${h & ~1}` : "";
  });
  const activeJobs = $derived(jobs.slice(0, 3));
</script>

<svelte:window onkeydown={keyDown} />

<div class="editor">
  <header class="topbar">
    <span class="title" title={path}>{name(path)}</span>
    <div class="group">
      <button type="button" class="icon" title="Undo (Ctrl+Z)" aria-label="Undo" disabled={!past.length} onclick={undo}
        ><Icon name="undo" /></button
      >
      <button type="button" class="icon" title="Redo (Ctrl+Y)" aria-label="Redo" disabled={!future.length} onclick={redo}
        ><Icon name="redo" /></button
      >
    </div>
    <span class="summary"
      >Result <strong>{format(output)}</strong>{resultSize ? ` · ${resultSize}` : ""}{ratio ? ` · ${ratio[0]}:${ratio[1]}` : ""}</span
    >
    <div class="spacer"></div>
    <button type="button" class="action" onclick={copyVideo} disabled={!duration} title="Copy the result to the clipboard (Ctrl+C)"
      ><Icon name="copy" /> Copy</button
    >
    <div class="split-button">
      <button type="button" class="action primary" onclick={() => exportAs("mp4")} disabled={!duration} title="Export MP4 (Ctrl+E)"
        ><Icon name="export" /> Export</button
      >
      <button
        type="button"
        class="action primary caret opens-menu"
        aria-label="Export options"
        aria-expanded={menu === "export"}
        onclick={() => (menu = menu === "export" ? null : "export")}><Icon name="chevron" /></button
      >
      {#if menu === "export"}
        <div class="popover export-menu" role="dialog" aria-label="Export options">
          <section>
            <h3>MP4</h3>
            <label class="field"
              >Quality
              <select bind:value={quality}>{#each QUALITIES as q (q.value)}<option value={q.value}>{q.label}</option>{/each}</select>
            </label>
            <label class="check"><input type="checkbox" bind:checked={audio} /> Keep the sound</label>
            <button type="button" class="primary wide" onclick={() => exportAs("mp4")}>Export MP4 <Kbd keys="Ctrl+E" /></button>
          </section>
          <section>
            <h3>GIF</h3>
            <div class="pair">
              <label class="field"
                >Frame rate
                <select bind:value={gifFps}>{#each GIF_FPS_PRESETS as fv (fv)}<option value={fv}>{fv} fps</option>{/each}</select>
              </label>
              <label class="field"
                >Width
                <select bind:value={gifWidth}
                  >{#each GIF_WIDTHS as w (w)}<option value={w}>{w === 0 ? "Original" : `${w} px`}</option>{/each}</select
                >
              </label>
            </div>
            <button type="button" class="wide" onclick={() => exportAs("gif")} disabled={!config}>Export GIF</button>
          </section>
        </div>
      {/if}
    </div>
  </header>

  <nav class="toolbox" aria-label="Tools">
    <div class="tools" role="toolbar">
      {#each TOOLS as t (t.tool)}
        <button
          type="button"
          class="tool"
          class:active={tool === t.tool && !cropping}
          title="{t.label} ({t.key})"
          aria-label={t.label}
          aria-pressed={tool === t.tool && !cropping}
          onclick={() => chooseTool(t.tool)}
        >
          <Icon name={t.icon} />
          <span class="key">{t.key}</span>
        </button>
      {/each}
      <span class="sep"></span>
      <button
        type="button"
        class="tool"
        class:active={cropping}
        title="Crop: draw the area to keep (C)"
        aria-label="Crop"
        aria-pressed={cropping}
        onclick={() => {
          cropping = !cropping;
          if (cropping) video.pause();
        }}
      >
        <Icon name="crop" />
        <span class="key">C</span>
      </button>
    </div>
    <span class="sep"></span>
    <div class="colors" role="group" aria-label="Colour">
      {#each COLORS as c (c)}
        <button type="button" class="swatch" class:active={color === c} style="--swatch:{c}" aria-label="Colour {c}" onclick={() => setColor(c)}
        ></button>
      {/each}
      <label class="custom" title="Any colour" style="--swatch:{color}"
        ><input type="color" value={color} onchange={(e) => setColor(e.currentTarget.value)} /></label
      >
    </div>
    <span class="sep"></span>
    <div class="levels" role="group" aria-label="Thickness">
      {#each LEVELS as l (l.level)}
        <button type="button" class="tool" class:active={level === l.level} title={l.label} aria-label={l.label} onclick={() => setLevel(l.level)}
          ><span class="dot" style="--d:{3 + l.level * 2}px"></span></button
        >
      {/each}
    </div>
    <div class="spacer"></div>
    {#if selectedItem}
      <span class="context">
        {selectedItem.shape.kind === "text" ? "Text" : selectedItem.shape.kind} · {format(selectedItem.start)} → {format(selectedItem.end)}
      </span>
      <button type="button" class="tool" title="Delete the selected item (Del)" aria-label="Delete the selected item" onclick={removeOverlay}
        ><Icon name="trash" /></button
      >
    {:else if cropping}
      <span class="context">Drag over the video to keep only that area · <Kbd keys="Esc" /> cancel</span>
    {:else if tool !== "select"}
      <span class="context">Click or drag on the video · shows for {OVERLAY_LENGTH} s, adjust it on the timeline</span>
    {:else}
      <span class="context">Click the video to play · click an item to select it</span>
    {/if}
  </nav>

  <div class="body">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="player"
      class:framed={!!ratio}
      bind:this={player}
      onpointerdown={stageDown}
      onpointermove={stageMove}
      onpointerup={stageUp}
      ondblclick={stageDouble}
      style="cursor:{stageCursor}"
    >
      <div class="frame" style={frameStyle}>
        <!-- svelte-ignore a11y_media_has_caption -->
        <video
          bind:this={video}
          src={source}
          preload="auto"
          onloadedmetadata={loaded}
          onresize={measure}
          onplay={() => (playing = true)}
          onpause={() => (playing = false)}
          onerror={() => (error = "This video cannot be played here, but it can still be exported.")}
        ></video>
      </div>
      <canvas
        bind:this={preview}
        class="preview"
        style="left:{box.left}px;top:{box.top}px;width:{box.width}px;height:{box.height}px"
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
          onpointerdown={(e) => e.stopPropagation()}
        ></textarea>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      {#if toast}<div class="toast" role="status">{toast}</div>{/if}
      {#if activeJobs.length}
        <div class="jobs" onpointerdown={(e) => e.stopPropagation()} role="status">
          {#each activeJobs as j (j.job)}
            <div class="job">
              <span class="kind">{j.kind === "copy" ? "Copy" : j.kind.toUpperCase()}</span>
              {#if j.error}
                <span class="failed" title={j.error}>{j.error}</span>
              {:else if j.output}
                <span class="name" title={j.output}>{name(j.output)} · {size(j.bytes ?? 0)}</span>
                <button type="button" class="icon small" title="Show in folder" aria-label="Show in folder" onclick={() => j.output && showInFolder(j.output)}
                  ><Icon name="folder" /></button
                >
              {:else}
                <progress max="1" value={j.done}></progress>
                <span class="pct">{Math.round(j.done * 100)}%</span>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <aside class="inspector">
      {#if selectedItem}
        <details open>
          <summary>Selected item</summary>
          <p class="note">Shows {format(selectedItem.start)} → {format(selectedItem.end)}. Drag its bar on the timeline to move it in time.</p>
          <div class="pair">
            <button type="button" onclick={() => setOverlayTime("start")}>From playhead</button>
            <button type="button" onclick={() => setOverlayTime("end")}>Until playhead</button>
          </div>
          <button type="button" class="wide" onclick={removeOverlay}><Icon name="trash" /> Delete <Kbd keys="Del" /></button>
        </details>
      {/if}
      <details open>
        <summary>Format</summary>
        <div class="chips">
          {#each RATIOS as r (r.label)}
            <button
              type="button"
              class="chip"
              class:active={(ratio?.join(":") ?? null) === (r.ratio?.join(":") ?? null)}
              onclick={() => (ratio = r.ratio)}>{r.label}</button
            >
          {/each}
        </div>
        {#if ratio}
          <div class="chips">
            {#each BACKGROUNDS as b, i (b.name)}
              <button
                type="button"
                class="swatch wide-swatch"
                class:active={background === i}
                style="--swatch:linear-gradient(135deg,{b.stops.join(',')})"
                title={b.name}
                aria-label={b.name}
                onclick={() => (background = i)}
              ></button>
            {/each}
          </div>
          <label class="field">Margin <input type="range" min="0" max="0.2" step="0.01" bind:value={margin} /></label>
        {/if}
        <div class="pair">
          <button type="button" class:active={cropping} onclick={() => (cropping = !cropping)}><Icon name="crop" /> Crop</button>
          <button type="button" disabled={!crop} onclick={() => commit(() => (crop = null))}>Reset crop</button>
        </div>
      </details>
      <details open>
        <summary>Cursor</summary>
        {#if !cursorTrack}
          <p class="note">Needs a Kadra recording made with “editable cursor” on (Settings › Recording).</p>
        {:else if cursorEditable}
          <label class="check"><input type="checkbox" bind:checked={cursorVisible} /> Show the pointer</label>
          <label class="field"
            >Size <input type="range" min="0.5" max="4" step="0.25" bind:value={cursorScale} /> <span class="value">{cursorScale}×</span></label
          >
          <label class="field"
            >Style
            <select bind:value={cursorStyle}>{#each CURSOR_STYLES as cs (cs.value)}<option value={cs.value}>{cs.label}</option>{/each}</select>
            {#if cursorStyle !== "original"}<input type="color" bind:value={cursorColor} class="mini" title="Pointer colour" />{/if}
          </label>
          <label class="check"
            ><input type="checkbox" bind:checked={halo} /> Halo <input type="color" bind:value={haloColor} class="mini" title="Halo colour" /></label
          >
        {:else}
          <p class="note">The copy without cursor of this recording is gone: only clicks can be marked.</p>
        {/if}
      </details>
      {#if cursorTrack}
        <details open>
          <summary>Clicks ({cursorTrack.clicks.length})</summary>
          <label class="check"
            ><input type="checkbox" bind:checked={clickRing} /> Ring <input type="color" bind:value={clickColor} class="mini" title="Ring colour" /></label
          >
          <label class="check"><input type="checkbox" bind:checked={clickSound} /> Click sound</label>
        </details>
      {/if}
    </aside>
  </div>

  <footer class="timeline">
    <div class="transport">
      <button type="button" class="icon" title="Go to the start (Home)" aria-label="Go to the start" onclick={() => seek(trim.start)}
        ><Icon name="start" /></button
      >
      <button type="button" class="icon play" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onclick={togglePlay}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <button type="button" class="icon" title="Go to the end (End)" aria-label="Go to the end" onclick={() => seek(trim.end - 0.001)}
        ><Icon name="end" /></button
      >
      <span class="time">{format(time)} <span class="muted">/ {format(duration)}</span></span>
      <span class="sep"></span>
      <button type="button" class="tbtn" onclick={splitHere} disabled={!duration} title="Split at the playhead (S)"
        ><Icon name="split" /> Split <Kbd keys="S" /></button
      >
      <button
        type="button"
        class="tbtn"
        onclick={() => removePart()}
        disabled={!duration}
        title={selected?.kept === false ? "Restore the selected part (Del)" : "Delete the selected part (Del)"}
        ><Icon name={selected?.kept === false ? "restore" : "trash"} />
        {selected?.kept === false ? "Restore" : "Delete"} <Kbd keys="Del" /></button
      >
      <div class="speed-menu">
        <button
          type="button"
          class="tbtn opens-menu"
          disabled={!duration}
          aria-expanded={menu === "speed"}
          title="Speed of the selected part"
          onclick={() => (menu = menu === "speed" ? null : "speed")}
          ><Icon name="speed" /> Speed {speedLabel((selected ?? segments[indexAt(segments, time)])?.speed ?? 1)}
          <Icon name="chevron" /></button
        >
        {#if menu === "speed"}
          <div class="popover speeds" role="menu">
            {#each SPEEDS as sp (sp)}
              <button
                type="button"
                role="menuitem"
                class:active={((selected ?? segments[indexAt(segments, time)])?.speed ?? 1) === sp}
                onclick={() => speedPart(sp)}>{speedLabel(sp)}</button
              >
            {/each}
            <hr />
            <div class="all">
              All parts:
              {#each [1, 1.5, 2] as sp (sp)}<button type="button" class="chip" onclick={() => allSpeeds(sp)}>{speedLabel(sp)}</button>{/each}
            </div>
          </div>
        {/if}
      </div>
      <div class="spacer"></div>
      <span class="hint-keys"
        ><Kbd keys="I" />/<Kbd keys="O" /> trim · <Kbd keys="Alt" /> no snapping · <Kbd keys="Ctrl" /> + wheel zoom</span
      >
      <div class="zoom">
        <button type="button" class="icon small" aria-label="Zoom out" title="Zoom out (Ctrl+-)" disabled={zoom <= 1} onclick={() => zoomBy(1 / 1.5)}
          ><Icon name="minus" /></button
        >
        <input
          type="range"
          min="0"
          max={Math.log(MAX_ZOOM)}
          step="0.01"
          value={Math.log(zoom)}
          aria-label="Timeline zoom"
          oninput={(e) => (zoom = Math.exp(Number(e.currentTarget.value)))}
        />
        <button type="button" class="icon small" aria-label="Zoom in" title="Zoom in (Ctrl++)" disabled={zoom >= MAX_ZOOM} onclick={() => zoomBy(1.5)}
          ><Icon name="plus" /></button
        >
      </div>
    </div>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="scroller" bind:this={scroller} onwheel={wheel}>
      <div
        class="content"
        bind:this={content}
        style="width:{zoom * 100}%"
        onpointerdown={scrubDown}
        onpointermove={timelineMove}
        onpointerup={timelineUp}
        onpointercancel={timelineUp}
      >
        <div class="ruler">
          {#each ticks as t (t)}
            <span class="tick" style="left:{percent(t)}%">{label(t, step)}</span>
          {/each}
        </div>

        <div class="track">
          <div class="filmstrip">
            {#each frames as src, i (i)}<img {src} alt="" draggable="false" />{/each}
          </div>
          {#each segments as sg, i (sg.start)}
            <div
              class="segment"
              class:removed={!sg.kept}
              class:selected={selectedSegment === i}
              style="left:{percent(sg.start)}%;width:{percent(sg.end) - percent(sg.start)}%"
            >
              {#if sg.kept && sg.speed !== 1}<span class="badge">{speedLabel(sg.speed)}</span>{/if}
              <div class="part-tools">
                <button
                  type="button"
                  title={sg.kept ? "Delete this part" : "Restore this part"}
                  aria-label={sg.kept ? "Delete this part" : "Restore this part"}
                  onpointerdown={(e) => e.stopPropagation()}
                  onclick={() => removePart(i)}><Icon name={sg.kept ? "trash" : "restore"} /></button
                >
                {#if sg.kept}
                  <button
                    type="button"
                    class="opens-menu"
                    title="Speed of this part"
                    onpointerdown={(e) => e.stopPropagation()}
                    onclick={() => {
                      selectedSegment = i;
                      menu = "speed";
                    }}>{speedLabel(sg.speed)}</button
                  >
                {/if}
              </div>
            </div>
          {/each}
          <!-- Outside the trim: dimmed. -->
          <div class="outside" style="left:0;width:{percent(trim.start)}%"></div>
          <div class="outside" style="left:{percent(trim.end)}%;right:0"></div>
          <div class="trim-frame" style="left:{percent(trim.start)}%;width:{percent(trim.end) - percent(trim.start)}%"></div>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="handle start"
            style="left:{percent(trim.start)}%"
            title="Drag to trim the start (I)"
            onpointerdown={(e) => trimDown(e, "start")}
          ></div>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="handle end"
            style="left:{percent(trim.end)}%"
            title="Drag to trim the end (O)"
            onpointerdown={(e) => trimDown(e, "end")}
          ></div>
        </div>

        <div class="lane">
          {#if !overlays.length}
            <span class="lane-hint">Texts and shapes appear here — drag a bar to choose when it shows</span>
          {/if}
          {#each overlays as o (o.id)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="bar"
              class:selected={selectedOverlay === o.id}
              style="left:{percent(o.start)}%;width:{percent(o.end) - percent(o.start)}%;--bar:{o.shape.color}"
              onpointerdown={(e) => barDown(e, o, "both")}
            >
              <span class="edge left" onpointerdown={(e) => barDown(e, o, "start")}></span>
              <span class="label">{o.shape.kind === "text" ? o.shape.text : o.shape.kind}</span>
              <span class="edge right" onpointerdown={(e) => barDown(e, o, "end")}></span>
            </div>
          {/each}
        </div>

        <div class="playhead" style="left:{percent(time)}%"><span class="knob"></span></div>
      </div>
    </div>
  </footer>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .topbar,
  .toolbox {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .toolbox {
    gap: 8px;
    padding: 4px 12px;
  }
  .title {
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .group {
    display: flex;
    gap: 2px;
    padding-left: 10px;
    border-left: 1px solid var(--border);
  }
  .summary,
  .time {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  .summary strong,
  .time {
    color: var(--text);
  }
  .muted {
    color: var(--text-muted);
  }
  .spacer {
    flex: 1;
  }
  .sep {
    width: 1px;
    align-self: stretch;
    margin: 4px 2px;
    background: var(--border);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  button.icon {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border-color: transparent;
    background: transparent;
  }
  button.icon.small {
    width: 26px;
    height: 26px;
  }
  button.icon:hover:not(:disabled),
  .tbtn:hover:not(:disabled) {
    background: var(--control);
    border-color: var(--border);
  }
  button.active,
  button.chip.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  button.chip {
    padding: 3px 9px;
  }
  button.wide {
    width: 100%;
    justify-content: center;
    margin-top: 6px;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }

  /* Export split button and menus */
  .split-button,
  .speed-menu {
    position: relative;
    display: flex;
  }
  .split-button .action.primary:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }
  .split-button .caret {
    padding: 5px 4px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid color-mix(in srgb, var(--on-accent) 30%, transparent);
  }
  .popover {
    position: absolute;
    z-index: 20;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.3);
  }
  .export-menu {
    top: calc(100% + 6px);
    right: 0;
    width: 280px;
  }
  .speeds {
    bottom: calc(100% + 6px);
    left: 0;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    width: 220px;
  }
  .speeds hr {
    grid-column: 1 / -1;
    width: 100%;
    border: none;
    border-top: 1px solid var(--border);
    margin: 4px 0;
  }
  .speeds .all {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-muted);
  }
  .speeds > button {
    justify-content: center;
    font-family: var(--font-mono);
  }

  /* Toolbox */
  .tools,
  .colors,
  .levels {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .colors {
    gap: 5px;
  }
  .tool {
    position: relative;
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border-color: transparent;
    background: transparent;
  }
  .tool:hover:not(.active) {
    background: var(--control);
    border-color: var(--border);
  }
  .tool .key {
    position: absolute;
    right: 2px;
    bottom: 0;
    font-size: 8px;
    font-weight: 700;
    font-family: var(--font-mono);
    opacity: 0.55;
  }
  .context {
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .swatch,
  .custom {
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 50%;
    background: var(--swatch);
    border: 2px solid var(--border);
  }
  .swatch.active {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .custom {
    position: relative;
    overflow: hidden;
    background: conic-gradient(red, yellow, lime, cyan, blue, magenta, red);
  }
  .custom input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }
  .dot {
    width: var(--d);
    height: var(--d);
    border-radius: 50%;
    background: currentColor;
  }

  /* Player */
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 260px;
  }
  .player {
    position: relative;
    min-width: 0;
    overflow: hidden;
    background: #000;
    display: grid;
    place-items: center;
    touch-action: none;
  }
  .frame {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    min-height: 0;
  }
  .player.framed .frame {
    width: auto;
    height: 100%;
    max-width: 100%;
  }
  video {
    width: 100%;
    height: 100%;
    min-height: 0;
    object-fit: contain;
    pointer-events: none;
  }
  .preview {
    position: absolute;
    pointer-events: none;
  }
  .text-input {
    position: absolute;
    min-width: 40px;
    padding: 0;
    margin: 0;
    border: 1px dashed var(--accent);
    background: transparent;
    font-family: "Segoe UI Variable Text", "Segoe UI", sans-serif;
    font-weight: 600;
    line-height: 1.25;
    resize: none;
    overflow: hidden;
    field-sizing: content;
    outline: none;
  }
  .toast,
  .error {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 12px;
    border-radius: 6px;
    background: var(--surface);
    border: 1px solid var(--border);
    font-size: 13px;
    pointer-events: none;
  }
  .toast {
    bottom: 14px;
  }
  .error {
    bottom: 50px;
    color: var(--danger);
  }
  .jobs {
    position: absolute;
    right: 10px;
    bottom: 10px;
    width: 280px;
    padding: 6px 8px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--surface) 92%, transparent);
    border: 1px solid var(--border);
    cursor: default;
  }
  .job {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 26px;
    font-size: 12px;
  }
  .job .kind {
    font-weight: 600;
    min-width: 36px;
  }
  .job .name,
  .failed {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .job progress {
    flex: 1;
  }
  .pct {
    font-family: var(--font-mono);
    color: var(--text-muted);
  }
  .failed {
    color: var(--danger);
  }

  /* Inspector */
  .inspector {
    overflow-y: auto;
    background: var(--surface);
    border-left: 1px solid var(--border);
  }
  details {
    padding: 8px 12px 12px;
    border-bottom: 1px solid var(--border);
  }
  summary {
    margin: 0 -4px 6px;
    padding: 2px 4px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
  }
  summary:hover {
    color: var(--text);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin: 4px 0 8px;
  }
  .wide-swatch {
    width: 34px;
    border-radius: 6px;
  }
  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .pair button {
    justify-content: center;
  }
  section + section {
    margin-top: 12px;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
  }
  .note {
    font-size: 12px;
    color: var(--text-muted);
    margin: 4px 0 8px;
  }
  .check,
  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
    font-size: 13px;
  }
  .field input[type="range"],
  .field select {
    flex: 1;
    min-width: 0;
  }
  .value {
    font-family: var(--font-mono);
    font-size: 12px;
    min-width: 32px;
  }
  .mini {
    width: 26px;
    height: 22px;
    padding: 0;
    border: none;
    background: none;
    margin-left: auto;
  }

  /* Timeline */
  .timeline {
    padding: 6px 12px 10px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .transport {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
  }
  .transport .play {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
  }
  .transport .play:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.1);
  }
  .time {
    min-width: 128px;
    margin-left: 4px;
  }
  .tbtn {
    border-color: transparent;
    background: transparent;
    padding: 4px 8px;
  }
  .hint-keys {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .zoom {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .zoom input {
    width: 110px;
    padding: 0;
  }
  .scroller {
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0 10px;
    scrollbar-width: thin;
  }
  .content {
    position: relative;
    min-width: 100%;
    touch-action: none;
    padding-bottom: 4px;
  }
  .ruler {
    position: relative;
    height: 18px;
    cursor: ew-resize;
  }
  .tick {
    position: absolute;
    top: 0;
    padding-left: 3px;
    border-left: 1px solid color-mix(in srgb, var(--text) 30%, transparent);
    height: 10px;
    font-size: 10px;
    line-height: 10px;
    font-family: var(--font-mono);
    color: var(--text-muted);
    white-space: nowrap;
    pointer-events: none;
  }
  .track,
  .lane {
    position: relative;
    border-radius: 6px;
  }
  .track {
    height: 58px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    cursor: pointer;
  }
  .lane {
    height: 28px;
    margin-top: 4px;
    background: color-mix(in srgb, var(--text) 5%, transparent);
    overflow: hidden;
  }
  .lane-hint {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 11px;
    color: var(--text-muted);
    pointer-events: none;
    white-space: nowrap;
  }
  .filmstrip {
    position: absolute;
    inset: 0;
    display: flex;
    overflow: hidden;
    border-radius: 6px;
    opacity: 0.9;
    pointer-events: none;
  }
  .filmstrip img {
    flex: 1;
    min-width: 0;
    height: 100%;
    object-fit: cover;
  }
  .segment {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 2px solid color-mix(in srgb, var(--surface) 80%, transparent);
  }
  .segment.removed {
    background: repeating-linear-gradient(135deg, rgb(0 0 0 / 0.7) 0 6px, rgb(0 0 0 / 0.5) 6px 12px);
  }
  .segment.selected {
    box-shadow: inset 0 0 0 2px var(--accent);
    background-color: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .badge {
    position: absolute;
    left: 6px;
    bottom: 4px;
    padding: 0 5px;
    border-radius: 4px;
    background: rgb(0 0 0 / 0.7);
    color: #fff;
    font: 600 11px/16px var(--font-mono);
    pointer-events: none;
  }
  .part-tools {
    position: absolute;
    top: 4px;
    right: 4px;
    display: none;
    gap: 2px;
    z-index: 3;
  }
  .segment:hover .part-tools,
  .segment.selected .part-tools {
    display: flex;
  }
  .part-tools button {
    height: 22px;
    min-width: 22px;
    padding: 0 4px;
    justify-content: center;
    border: none;
    border-radius: 4px;
    background: rgb(0 0 0 / 0.72);
    color: #fff;
    font: 600 11px var(--font-mono);
  }
  .part-tools button:hover {
    background: var(--accent);
    color: var(--on-accent);
  }
  .part-tools :global(svg) {
    width: 14px;
    height: 14px;
  }
  .outside {
    position: absolute;
    top: 0;
    bottom: 0;
    background: rgb(0 0 0 / 0.62);
    pointer-events: none;
  }
  .trim-frame {
    position: absolute;
    top: 0;
    bottom: 0;
    border-top: 3px solid #ffc531;
    border-bottom: 3px solid #ffc531;
    pointer-events: none;
  }
  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 14px;
    background: #ffc531;
    cursor: ew-resize;
    z-index: 4;
  }
  .handle.start {
    margin-left: -2px;
    border-radius: 6px 0 0 6px;
    transform: translateX(-12px);
  }
  .handle.end {
    margin-left: 2px;
    border-radius: 0 6px 6px 0;
  }
  .handle::after {
    content: "";
    position: absolute;
    left: 5px;
    top: 50%;
    width: 4px;
    height: 18px;
    margin-top: -9px;
    border-left: 1.5px solid #6b4e00;
    border-right: 1.5px solid #6b4e00;
  }
  .handle:hover {
    filter: brightness(1.08);
  }
  .bar {
    position: absolute;
    top: 3px;
    bottom: 3px;
    display: flex;
    align-items: center;
    border-radius: 4px;
    background: color-mix(in srgb, var(--bar) 55%, transparent);
    border: 1px solid var(--bar);
    cursor: grab;
    overflow: hidden;
  }
  .bar.selected {
    outline: 2px solid var(--accent);
  }
  .bar .label {
    flex: 1;
    padding: 0 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    color: #fff;
    text-shadow: 0 0 2px #000;
    pointer-events: none;
  }
  .edge {
    width: 6px;
    align-self: stretch;
    cursor: ew-resize;
    background: rgb(255 255 255 / 0.5);
  }
  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: #fff;
    box-shadow: 0 0 2px #000;
    pointer-events: none;
    z-index: 5;
  }
  .knob {
    position: absolute;
    top: 0;
    left: -5px;
    width: 12px;
    height: 12px;
    border-radius: 2px 2px 6px 6px;
    background: #fff;
    box-shadow: 0 0 2px #000;
  }
</style>
