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
    type Segment,
    format,
    indexAt,
    keptLength,
    keptRanges,
    nextKept,
    split,
    toggle,
    trimAt,
    whole,
  } from "./timeline";

  let { path }: { path: string } = $props();

  type Tab = "cut" | "annotate" | "cursor" | "frame" | "export";
  const TABS: { tab: Tab; icon: IconName; label: string; key: string }[] = [
    { tab: "cut", icon: "scissors", label: "Cut & speed", key: "1" },
    { tab: "annotate", icon: "text", label: "Text & shapes", key: "2" },
    { tab: "cursor", icon: "cursor", label: "Cursor & clicks", key: "3" },
    { tab: "frame", icon: "aspect", label: "Crop & format", key: "4" },
    { tab: "export", icon: "export", label: "Export", key: "5" },
  ];
  type OverlayTool = "select" | "text" | "arrow" | "line" | "rect" | "ellipse" | "highlight" | "pen" | "number";
  const OVERLAY_TOOLS: { tool: OverlayTool; icon: IconName; label: string; key: string }[] = [
    { tool: "select", icon: "select", label: "Select and move", key: "V" },
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
  const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3];
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
  const FILMSTRIP = 12;
  /** How long a new text or shape stays on screen by default, in seconds. */
  const OVERLAY_LENGTH = 3;

  interface Overlay {
    id: number;
    shape: Shape;
    start: number;
    end: number;
  }

  let video: HTMLVideoElement;
  let player: HTMLDivElement;
  let preview: HTMLCanvasElement;
  let track: HTMLDivElement;
  let lane: HTMLDivElement;

  let duration = $state(0);
  let time = $state(0);
  let playing = $state(false);
  let segments = $state<Segment[]>([]);
  let selectedSegment = $state(0);
  let speed = $state(1);
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
  let halo = $state(false);
  let haloColor = $state("#ffd60a");
  let clickRing = $state(false);
  let clickColor = $state("#ffd60a");
  let clickSound = $state(false);
  let overlays = $state<Overlay[]>([]);
  let selectedOverlay = $state<number | null>(null);
  let tool = $state<OverlayTool>("text");
  let color = $state("#e5484d");
  let level = $state(2);
  let tab = $state<Tab>("cut");
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
  let nextId = 1;
  let drag:
    | { kind: "time" }
    | { kind: "crop"; start: Point }
    | { kind: "draw"; start: Point; moved: boolean }
    | { kind: "move"; last: Point; moved: boolean }
    | { kind: "bar"; id: number; edge: "start" | "end" | "both"; from: number; start: number; end: number }
    | null = null;

  const source = $derived(convertFileSrc(cleanSource ?? path));
  const output = $derived(keptLength(segments) / speed);
  /** The cursor can only be changed with the recording's copy without cursor. */
  const cursorEditable = $derived(!!cleanSource && !!cursorTrack);
  /** Annotation sizes follow the video size, like in the image editor. */
  let longSide = $state(0);
  const unit = $derived(Math.max(1, Math.round(longSide / 1400)));

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
    const observer = new ResizeObserver(measure);
    observer.observe(player);
    // The format (aspect ratio) resizes the video inside the player.
    observer.observe(video);
    let frame = requestAnimationFrame(function loop() {
      paint();
      frame = requestAnimationFrame(loop);
    });
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      unlisten.then((f) => f());
    };
  });

  function loaded() {
    longSide = Math.max(video.videoWidth, video.videoHeight);
    if (!segments.length) {
      duration = video.duration;
      segments = whole(duration);
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
    canvas.height = 48;
    canvas.width = Math.round((48 * probe.videoWidth) / Math.max(1, probe.videoHeight));
    const ctx = canvas.getContext("2d");
    const out: string[] = [];
    for (let i = 0; i < FILMSTRIP && ctx; i++) {
      probe.currentTime = ((i + 0.5) * duration) / FILMSTRIP;
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
    const chosen = overlays.find((o) => o.id === selectedOverlay);
    if (chosen && tab === "annotate") outline(ctx, bounds(ctx, chosen.shape), s);

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
        const custom = cursorStyle !== "original" ? drawCursor(cursorStyle, color) : null;
        const img = custom?.canvas ?? shapes[at.shape]?.image;
        const hot = custom?.hotspot ?? shapes[at.shape]?.hotspot ?? [0, 0];
        if (img && (custom || (img as HTMLImageElement).complete)) {
          const iw = (img as HTMLImageElement).naturalWidth || img.width;
          const ih = (img as HTMLImageElement).naturalHeight || img.height;
          ctx.drawImage(img, at.x - hot[0] * scale, at.y - hot[1] * scale, iw * scale, ih * scale);
        }
      }
    }

    const area = cropDraft ?? (cropping ? crop : crop);
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

  /** Playback skips removed parts and loops over the kept ones. */
  function timeUpdate() {
    time = video.currentTime;
    if (!playing) return;
    const next = nextKept(segments, time);
    if (next === null) video.currentTime = keptRanges(segments)[0]?.[0] ?? 0;
    else if (next > time + 0.01) video.currentTime = next;
  }

  function togglePlay() {
    if (video.paused) {
      if (nextKept(segments, video.currentTime) === null) video.currentTime = keptRanges(segments)[0]?.[0] ?? 0;
      video.playbackRate = speed;
      video.play();
    } else {
      video.pause();
    }
  }

  function seek(t: number) {
    video.currentTime = Math.max(0, Math.min(duration, t));
    time = video.currentTime;
    selectedSegment = indexAt(segments, time);
  }

  $effect(() => {
    if (video) video.playbackRate = speed;
  });

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
    crop = parsed.crop;
    overlays = parsed.overlays;
    selectedOverlay = null;
  }

  const undo = () => restore(past, future);
  const redo = () => restore(future, past);

  // ── Cutting ────────────────────────────────────────────────────────────

  const splitHere = () => commit(() => (segments = split(segments, time)));
  const removeSegment = () => commit(() => (segments = toggle(segments, selectedSegment)));
  const startHere = () => commit(() => (segments = trimAt(segments, time, true)));
  const endHere = () => commit(() => (segments = trimAt(segments, time, false)));

  function trackPos(e: PointerEvent, el: HTMLElement) {
    const r = el.getBoundingClientRect();
    return (Math.max(0, Math.min(r.width, e.clientX - r.left)) / r.width) * duration;
  }

  function trackDown(e: PointerEvent) {
    drag = { kind: "time" };
    track.setPointerCapture(e.pointerId);
    seek(trackPos(e, track));
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
  }

  function topmost(p: Point): Overlay | undefined {
    const ctx = preview.getContext("2d");
    if (!ctx) return undefined;
    return [...overlays]
      .reverse()
      .find((o) => time >= o.start && time < o.end && hit(ctx, o.shape, p, 8 / box.scale));
  }

  function stageDown(e: PointerEvent) {
    if (!video?.videoWidth || e.button !== 0) return;
    e.preventDefault();
    if (editing) return commitText();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const p = toVideo(e);
    if (cropping) {
      drag = { kind: "crop", start: p };
      cropDraft = { ...p, w: 0, h: 0 };
      return;
    }
    if (tab !== "annotate") {
      togglePlay();
      return;
    }
    video.pause();
    if (tool === "select") {
      const found = topmost(p);
      selectedOverlay = found?.id ?? null;
      if (found) {
        past.push(snapshot());
        drag = { kind: "move", last: p, moved: false };
      }
      return;
    }
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
      const o = overlays.find((x) => x.id === selectedOverlay);
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
    drag = null;
    if (!current) return;
    if (current.kind === "crop") {
      const area = cropDraft;
      cropDraft = null;
      if (area && area.w > 8 && area.h > 8) {
        commit(() => (crop = { x: Math.round(area.x), y: Math.round(area.y), w: Math.round(area.w), h: Math.round(area.h) }));
        cropping = false;
      }
    } else if (current.kind === "move") {
      if (!current.moved) past.pop();
    } else if (current.kind === "draw" && draft) {
      const shape = draft;
      draft = null;
      if (current.moved) addOverlay(shape);
    }
  }

  function stageDouble(e: MouseEvent) {
    if (tab !== "annotate") return;
    const found = topmost(toVideo(e));
    if (found?.shape.kind === "text") {
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
    const o = overlays.find((x) => x.id === selectedOverlay);
    if (!o) return;
    commit(() => {
      if (edge === "start") o.start = Math.min(time, o.end - 0.1);
      else o.end = Math.max(time, o.start + 0.1);
    });
  }

  function setColor(c: string) {
    color = c;
    const o = overlays.find((x) => x.id === selectedOverlay);
    if (o) commit(() => (o.shape.color = c));
  }

  /** Dragging a bar of the overlay lane changes when it shows. */
  function barDown(e: PointerEvent, o: Overlay, edge: "start" | "end" | "both") {
    e.stopPropagation();
    lane.setPointerCapture(e.pointerId);
    selectedOverlay = o.id;
    tab = "annotate";
    past.push(snapshot());
    drag = { kind: "bar", id: o.id, edge, from: trackPos(e, lane), start: o.start, end: o.end };
  }

  function laneMove(e: PointerEvent) {
    if (drag?.kind === "time") return seek(trackPos(e, track));
    if (drag?.kind !== "bar") return;
    const d = drag;
    const o = overlays.find((x) => x.id === d.id);
    if (!o) return;
    const delta = trackPos(e, lane) - d.from;
    if (d.edge === "start") o.start = Math.max(0, Math.min(d.end - 0.1, d.start + delta));
    else if (d.edge === "end") o.end = Math.min(duration, Math.max(d.start + 0.1, d.end + delta));
    else {
      const length = d.end - d.start;
      o.start = Math.max(0, Math.min(duration - length, d.start + delta));
      o.end = o.start + length;
    }
    seek(d.edge === "end" ? o.end - 0.01 : o.start);
  }

  function laneUp() {
    drag = null;
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
      const drawn = drawCursor(cursorStyle, color);
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
      segments: keptRanges(segments),
      speed,
      crop: crop ? [crop.x, crop.y, crop.w, crop.h] : null,
      composition: composition(),
    };
  }

  /** Nothing changed: the file can be copied as it is. */
  const untouched = $derived(
    keptRanges(segments).length === 1 &&
      keptLength(segments) >= duration - 0.01 &&
      speed === 1 &&
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
    if (!config || !keptRanges(segments).length) return;
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
      else return;
      e.preventDefault();
      return;
    }
    const step = e.shiftKey ? 1 : 1 / 30;
    const tabKey = TABS.find((t) => t.key === key);
    if (tabKey) {
      tab = tabKey.tab;
      return;
    }
    const overlayKey = OVERLAY_TOOLS.find((t) => t.key.toLowerCase() === key);
    if (overlayKey && key !== "s") {
      tab = "annotate";
      tool = overlayKey.tool;
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
      case "s":
        splitHere();
        break;
      case "delete":
      case "backspace":
        if (selectedOverlay !== null) removeOverlay();
        else removeSegment();
        break;
      case "i":
        startHere();
        break;
      case "o":
        endHere();
        break;
      case "c":
        tab = "frame";
        cropping = !cropping;
        break;
      case "escape":
        cropping = false;
        selectedOverlay = null;
        draft = null;
        break;
    }
  }

  const percent = (t: number) => (duration ? (t / duration) * 100 : 0);
  const size = (bytes: number) =>
    bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const stageCursor = $derived(
    cropping ? "crosshair" : tab === "annotate" ? (tool === "select" ? "default" : tool === "text" ? "text" : "crosshair") : "pointer",
  );
  const frameStyle = $derived.by(() => {
    if (!ratio) return "";
    const stops = BACKGROUNDS[background]?.stops ?? ["#222", "#111"];
    return `aspect-ratio:${ratio[0]}/${ratio[1]};background:linear-gradient(135deg,${stops.join(",")});padding:${margin * 100}%`;
  });
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
    <span class="summary">Result {format(output)}{crop ? ` · ${crop.w}×${crop.h}` : ""}{ratio ? ` · ${ratio[0]}:${ratio[1]}` : ""}</span>
    <div class="spacer"></div>
    <button type="button" class="action" onclick={copyVideo} disabled={!duration}
      ><Icon name="copy" /> Copy <Kbd keys="Ctrl+C" /></button
    >
    <button type="button" class="action" onclick={() => exportAs("gif")} disabled={!config || !duration}>GIF</button>
    <button type="button" class="action primary" onclick={() => exportAs("mp4")} disabled={!duration}
      ><Icon name="export" /> Export MP4 <Kbd keys="Ctrl+E" /></button
    >
  </header>

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
          ontimeupdate={timeUpdate}
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
      {#if cropping}
        <div class="hint">Drag over the video to keep only that area · <Kbd keys="Esc" /> leave</div>
      {:else if tab === "annotate" && tool !== "select"}
        <div class="hint">Click or drag on the video · shows for {OVERLAY_LENGTH} s from the playhead, adjust it on the timeline</div>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      {#if toast}<div class="toast" role="status">{toast}</div>{/if}
    </div>

    <aside class="panel">
      <nav class="tabs" aria-label="Editing">
        {#each TABS as t (t.tab)}
          <button type="button" class:active={tab === t.tab} title="{t.label} ({t.key})" onclick={() => (tab = t.tab)}>
            <Icon name={t.icon} />
            <span>{t.label}</span>
            <span class="key">{t.key}</span>
          </button>
        {/each}
      </nav>

      <div class="content">
        {#if tab === "cut"}
          <section>
            <h3>Cut</h3>
            <div class="column">
              <button type="button" onclick={splitHere}><Icon name="split" /> Split at the playhead <Kbd keys="S" /></button>
              <button type="button" onclick={removeSegment}
                ><Icon name="trash" />
                {segments[selectedSegment]?.kept === false ? "Restore the selected part" : "Remove the selected part"}
                <Kbd keys="Del" /></button
              >
              <button type="button" onclick={startHere}>Start here <Kbd keys="I" /></button>
              <button type="button" onclick={endHere}>End here <Kbd keys="O" /></button>
              <button type="button" class="link" onclick={() => commit(() => (segments = whole(duration)))}>Keep everything</button>
            </div>
            <p class="note">Click the timeline to place the playhead and select a part.</p>
          </section>
          <section>
            <h3>Speed</h3>
            <div class="row">
              {#each SPEEDS as sp (sp)}
                <button type="button" class="chip" class:active={speed === sp} onclick={() => (speed = sp)}>{sp}×</button>
              {/each}
            </div>
            <p class="note">The sound follows the speed without changing pitch.</p>
          </section>
        {:else if tab === "annotate"}
          <section>
            <h3>Add</h3>
            <div class="tool-grid">
              {#each OVERLAY_TOOLS as t (t.tool)}
                <button type="button" class="tool" class:active={tool === t.tool} title="{t.label} ({t.key})" onclick={() => (tool = t.tool)}>
                  <Icon name={t.icon} />
                  <span class="key">{t.key}</span>
                </button>
              {/each}
            </div>
          </section>
          <section>
            <h3>Colour</h3>
            <div class="row">
              {#each COLORS as c (c)}
                <button type="button" class="swatch" class:active={color === c} style="--swatch:{c}" aria-label="Colour {c}" onclick={() => setColor(c)}></button>
              {/each}
              <label class="custom" title="Any colour"><input type="color" value={color} onchange={(e) => setColor(e.currentTarget.value)} /></label>
            </div>
            <div class="row">
              {#each [1, 2, 4] as l (l)}
                <button type="button" class="chip" class:active={level === l} onclick={() => (level = l)}
                  ><span class="dot" style="--d:{3 + l * 2}px"></span> {l === 1 ? "Thin" : l === 2 ? "Medium" : "Thick"}</button
                >
              {/each}
            </div>
          </section>
          {#if selectedOverlay !== null}
            {@const o = overlays.find((x) => x.id === selectedOverlay)}
            {#if o}
              <section>
                <h3>Selected · {format(o.start)} → {format(o.end)}</h3>
                <div class="column">
                  <button type="button" onclick={() => setOverlayTime("start")}>Show from the playhead</button>
                  <button type="button" onclick={() => setOverlayTime("end")}>Hide at the playhead</button>
                  <button type="button" onclick={removeOverlay}><Icon name="trash" /> Delete <Kbd keys="Del" /></button>
                </div>
              </section>
            {/if}
          {/if}
          <p class="note">Each item has a bar under the timeline: drag it, or its ends, to choose when it shows. Double-click a text to edit it.</p>
        {:else if tab === "cursor"}
          {#if !cursorTrack}
            <p class="note">
              Cursor effects need a recording made with Kadra with “editable cursor” on (Settings › Recording). This video
              has no cursor track.
            </p>
          {:else}
            <section>
              <h3>Pointer</h3>
              {#if cursorEditable}
                <label class="check"><input type="checkbox" bind:checked={cursorVisible} /> Show the pointer</label>
                <label class="field">Size <input type="range" min="0.5" max="4" step="0.25" bind:value={cursorScale} /> <span class="value">{cursorScale}×</span></label>
                <div class="column">
                  {#each CURSOR_STYLES as cs (cs.value)}
                    <label class="check"><input type="radio" name="cursor" value={cs.value} bind:group={cursorStyle} /> {cs.label}</label>
                  {/each}
                </div>
                <label class="check"><input type="checkbox" bind:checked={halo} /> Halo around the pointer
                  <input type="color" bind:value={haloColor} class="mini" /></label
                >
              {:else}
                <p class="note">The copy without cursor of this recording is gone: only clicks can be marked.</p>
              {/if}
            </section>
            <section>
              <h3>Clicks ({cursorTrack.clicks.length})</h3>
              <label class="check"><input type="checkbox" bind:checked={clickRing} /> Mark clicks with a ring
                <input type="color" bind:value={clickColor} class="mini" /></label
              >
              <label class="check"><input type="checkbox" bind:checked={clickSound} /> Click sound</label>
            </section>
          {/if}
        {:else if tab === "frame"}
          <section>
            <h3>Crop</h3>
            <div class="column">
              <button type="button" class:active={cropping} onclick={() => (cropping = !cropping)}><Icon name="crop" /> Draw the area to keep <Kbd keys="C" /></button>
              {#if crop}
                <button type="button" class="link" onclick={() => commit(() => (crop = null))}>Reset crop ({crop.w}×{crop.h})</button>
              {/if}
            </div>
          </section>
          <section>
            <h3>Format</h3>
            <div class="row">
              {#each RATIOS as r (r.label)}
                <button type="button" class="chip" class:active={(ratio?.join(":") ?? null) === (r.ratio?.join(":") ?? null)} onclick={() => (ratio = r.ratio)}
                  >{r.label}</button
                >
              {/each}
            </div>
            {#if ratio}
              <div class="row">
                {#each BACKGROUNDS as b, i (b.name)}
                  <button type="button" class="swatch wide" class:active={background === i} style="--swatch:linear-gradient(135deg,{b.stops.join(',')})" title={b.name} aria-label={b.name} onclick={() => (background = i)}></button>
                {/each}
              </div>
              <label class="field">Margin <input type="range" min="0" max="0.2" step="0.01" bind:value={margin} /></label>
            {/if}
            <p class="note">For Shorts, Reels and TikTok use 9:16; the video is centred on the background.</p>
          </section>
        {:else}
          <section>
            <h3>MP4</h3>
            <label class="field">Quality
              <select bind:value={quality}>{#each QUALITIES as q (q.value)}<option value={q.value}>{q.label}</option>{/each}</select>
            </label>
            <label class="check"><input type="checkbox" bind:checked={audio} /> Keep the sound</label>
            <button type="button" class="primary" onclick={() => exportAs("mp4")} disabled={!duration}>Export MP4 <Kbd keys="Ctrl+E" /></button>
          </section>
          <section>
            <h3>GIF</h3>
            <label class="field">Frame rate
              <select bind:value={gifFps}>{#each GIF_FPS_PRESETS as fv (fv)}<option value={fv}>{fv} fps</option>{/each}</select>
            </label>
            <label class="field">Width
              <select bind:value={gifWidth}>{#each GIF_WIDTHS as w (w)}<option value={w}>{w === 0 ? "Original" : `${w} px`}</option>{/each}</select>
            </label>
            <button type="button" onclick={() => exportAs("gif")} disabled={!config || !duration}>Export GIF</button>
          </section>
        {/if}

        {#if jobs.length}
          <section class="jobs">
            <h3>Exports</h3>
            {#each jobs as j (j.job)}
              <div class="job">
                <span class="kind">{j.kind === "copy" ? "Copy" : j.kind.toUpperCase()}</span>
                {#if j.error}
                  <span class="failed">{j.error}</span>
                {:else if j.output}
                  <span class="name" title={j.output}>{name(j.output)} · {size(j.bytes ?? 0)}</span>
                  <button type="button" class="icon" title="Show in folder" aria-label="Show in folder" onclick={() => j.output && showInFolder(j.output)}><Icon name="folder" /></button>
                {:else}
                  <progress max="1" value={j.done}></progress>
                {/if}
              </div>
            {/each}
          </section>
        {/if}
      </div>
    </aside>
  </div>

  <footer class="timeline">
    <div class="transport">
      <button type="button" class="icon" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onclick={togglePlay}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <span class="time">{format(time)} / {format(duration)}</span>
      <span class="note">
        <Kbd keys="Space" /> play · <Kbd keys="←" /><Kbd keys="→" /> frame · <Kbd keys="S" /> split · <Kbd keys="Del" /> remove ·
        <Kbd keys="I" />/<Kbd keys="O" /> start/end
      </span>
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="tracks" onpointermove={laneMove} onpointerup={laneUp}>
      <div class="track" bind:this={track} onpointerdown={trackDown}>
        <div class="filmstrip">
          {#each frames as src, i (i)}<img {src} alt="" />{/each}
        </div>
        {#each segments as sg, i (sg.start)}
          <div
            class="segment"
            class:removed={!sg.kept}
            class:selected={selectedSegment === i}
            style="left:{percent(sg.start)}%;width:{percent(sg.end) - percent(sg.start)}%"
          ></div>
        {/each}
        <div class="playhead" style="left:{percent(time)}%"></div>
      </div>
      <div class="lane" bind:this={lane}>
        {#each overlays as o (o.id)}
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
        <div class="playhead" style="left:{percent(time)}%"></div>
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
  .topbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
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
  .spacer {
    flex: 1;
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
  button.icon:hover:not(:disabled) {
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
  button.link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 2px 0;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 300px;
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
    box-sizing: border-box;
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
  .hint,
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
  .hint {
    top: 10px;
    color: var(--text-muted);
  }
  .toast {
    bottom: 14px;
  }
  .error {
    bottom: 50px;
    color: var(--danger);
  }
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--surface);
    border-left: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    flex-direction: column;
    padding: 6px;
    gap: 2px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    position: relative;
    justify-content: flex-start;
    border-color: transparent;
    background: transparent;
    padding: 6px 10px;
  }
  .tabs button:hover {
    background: var(--control);
  }
  .tabs button.active {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--text);
    border-color: var(--accent);
  }
  .key {
    margin-left: auto;
    font-size: 10px;
    font-family: var(--font-mono);
    opacity: 0.6;
  }
  .content {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }
  section {
    margin-bottom: 16px;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 4px 0;
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .column button {
    justify-content: flex-start;
  }
  .note {
    font-size: 12px;
    color: var(--text-muted);
    margin: 6px 0;
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
  }
  .mini {
    width: 26px;
    height: 22px;
    padding: 0;
    border: none;
    background: none;
  }
  .tool-grid {
    display: grid;
    grid-template-columns: repeat(5, 40px);
    gap: 4px;
  }
  .tool {
    position: relative;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    padding: 0;
  }
  .tool .key {
    position: absolute;
    right: 3px;
    bottom: 1px;
    font-size: 9px;
    font-weight: 700;
  }
  .swatch {
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    background: var(--swatch);
    border: 2px solid var(--border);
  }
  .swatch.wide {
    width: 38px;
    border-radius: 6px;
  }
  .swatch.active {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .custom input {
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    background: none;
  }
  .dot {
    width: var(--d);
    height: var(--d);
    border-radius: 50%;
    background: currentColor;
  }
  .jobs .job {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .job .kind {
    font-weight: 600;
    min-width: 36px;
  }
  .job .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .job progress {
    flex: 1;
  }
  .failed {
    color: var(--danger);
  }
  .timeline {
    padding: 8px 12px 12px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .transport {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 6px;
  }
  .transport .note {
    margin: 0 0 0 auto;
  }
  .tracks {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .track,
  .lane {
    position: relative;
    border-radius: 6px;
    overflow: hidden;
    touch-action: none;
  }
  .track {
    height: 52px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    cursor: pointer;
  }
  .lane {
    height: 26px;
    background: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .filmstrip {
    position: absolute;
    inset: 0;
    display: flex;
    opacity: 0.85;
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
    border: 2px solid var(--accent);
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    pointer-events: none;
  }
  .segment.removed {
    border-color: transparent;
    background: repeating-linear-gradient(135deg, rgb(0 0 0 / 0.65) 0 6px, rgb(0 0 0 / 0.45) 6px 12px);
  }
  .segment.selected {
    box-shadow: inset 0 0 0 2px #fff;
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
    outline: 2px solid #fff;
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
  }
</style>
