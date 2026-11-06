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
  import { ditherBand } from "./dither";
  import {
    BACKGROUNDS,
    FREEHAND,
    LINEAR,
    type Point,
    type Rect,
    type Shape,
    type ShapeKind,
    bounds,
    fontSize,
    hit,
    renderAnnotations,
  } from "./shapes";
  import {
    type Layout,
    clampRect,
    corners,
    dragCorner,
    fitLayout,
    packRows,
    plainLayout,
    playClick,
    ratioCrop,
    resizeShape,
    shapeHandles,
  } from "./stage";
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
    { tool: "select", icon: "select", label: "Select, move, resize — click the video to play", key: "V" },
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
  /** How long a new text or shape stays on screen by default, in seconds. */
  const OVERLAY_LENGTH = 3;
  /** Distance under which handles snap, in pixels. */
  const SNAP_PX = 8;
  const MAX_ZOOM = 40;
  /** Height of the filmstrip and of one row of time bars, in pixels. */
  const TRACK_H = 56;
  const ROW_H = 26;
  /** Space around the picture in the player, in pixels. */
  const STAGE_MARGIN = 28;

  interface Overlay {
    id: number;
    shape: Shape;
    start: number;
    end: number;
  }

  let video: HTMLVideoElement;
  let player: HTMLDivElement;
  let stage: HTMLCanvasElement;
  let scroller: HTMLDivElement;
  let content: HTMLDivElement;

  let duration = $state(0);
  let time = $state(0);
  let playing = $state(false);
  let vw = $state(0);
  let vh = $state(0);
  let segments = $state<Segment[]>([]);
  let trim = $state<Trim>({ start: 0, end: 0 });
  let selectedSegment = $state<number | null>(null);
  let audio = $state(true);
  let quality = $state(18);
  let crop = $state<Rect | null>(null);
  /** Editing the crop: the whole frame shows, with the area kept. */
  let cropping = $state(false);
  let cropDraft = $state<Rect | null>(null);
  let ratio = $state<[number, number] | null>(null);
  /** With a ratio: crop to it (fill) or show all on a background (fit). */
  let fit = $state<"fill" | "fit">("fill");
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
  /** Filmstrip frames, evenly spread over the video; empty until made. */
  let frames = $state<string[]>([]);
  let error = $state("");
  let toast = $state("");
  let config = $state<Config | null>(null);
  let gifFps = $state(15);
  let gifWidth = $state(0);
  let gifOptions = $state(false);
  let jobs = $state<(JobProgress & { kind: string })[]>([]);
  let past = $state<string[]>([]);
  let future = $state<string[]>([]);
  let cleanSource = $state<string | null>(null);
  let cursorTrack = $state<CursorTrack | null>(null);
  let shapes: { image: HTMLImageElement; hotspot: [number, number] }[] = [];
  let editing = $state<{ x: number; y: number; text: string; id: number | null } | null>(null);
  let playerSize = $state({ w: 0, h: 0 });
  let draft = $state<Shape | null>(null);
  let zoom = $state(1);
  let viewWidth = $state(800);
  let menu = $state<"speed" | null>(null);
  /** Resize handle under the pointer (select tool), or -1. */
  let hover = $state(-1);
  let nextId = 1;
  let audioContext: AudioContext | null = null;
  let lastClickCheck = 0;
  let drag:
    | { kind: "time" }
    | { kind: "trim"; edge: "start" | "end" }
    | { kind: "cropCorner"; anchor: Point }
    | { kind: "cropMove"; from: Point; rect: Rect }
    | { kind: "draw"; start: Point; moved: boolean }
    | { kind: "move"; last: Point; moved: boolean }
    | { kind: "resize"; handle: number; original: Shape; moved: boolean }
    | { kind: "bar"; id: number; edge: "start" | "end" | "both"; from: number; start: number; end: number }
    | null = null;

  const source = $derived(convertFileSrc(cleanSource ?? path));
  const kept = $derived(parts(segments, trim));
  const output = $derived(outputLength(kept));
  /** The cursor can only be changed with the recording's copy without cursor. */
  const cursorEditable = $derived(!!cleanSource && !!cursorTrack);
  /** Annotation sizes follow the video size, like in the image editor. */
  const unit = $derived(Math.max(1, Math.round(Math.max(vw, vh) / 1400)));
  const full = $derived<Rect>({ x: 0, y: 0, w: vw || 1, h: vh || 1 });

  /** What the export looks like (while cropping: the whole frame). */
  const layout = $derived<Layout>(
    cropping || !vw
      ? plainLayout(full)
      : ratio && fit === "fit"
        ? fitLayout(crop ?? full, ratio, margin)
        : plainLayout(crop ?? full),
  );
  /** CSS pixels per output pixel, and per source pixel. */
  const k = $derived(
    Math.max(
      0.01,
      Math.min((playerSize.w - 2 * STAGE_MARGIN) / layout.out.w, (playerSize.h - 2 * STAGE_MARGIN) / layout.out.h),
    ),
  );
  const sourceScale = $derived(k * (layout.area.w / layout.crop.w));
  const stageBox = $derived({
    width: layout.out.w * k,
    height: layout.out.h * k,
    left: (playerSize.w - layout.out.w * k) / 2,
    top: (playerSize.h - layout.out.h * k) / 2,
  });
  const lockedRatio = $derived(ratio && fit === "fill" ? ratio[0] / ratio[1] : null);

  const contentWidth = $derived(viewWidth * zoom);
  const pxPerSecond = $derived(duration ? contentWidth / duration : 1);
  const step = $derived(rulerStep(pxPerSecond));
  const ticks = $derived.by(() => {
    if (!duration) return [];
    const out: number[] = [];
    for (let t = 0; t <= duration + 1e-9 && out.length < 2000; t += step) out.push(t);
    return out;
  });
  /** Filmstrip tiles at the video's own proportions, each showing the
   * frame under its middle (the nearest one made so far). */
  const tiles = $derived.by(() => {
    if (!frames.some(Boolean) || !duration || !vw) return [];
    const width = Math.max(24, (TRACK_H * vw) / vh);
    const count = Math.min(600, Math.ceil(contentWidth / width));
    return Array.from({ length: count }, (_, i) => {
      const t = ((i + 0.5) * width) / pxPerSecond;
      const index = Math.min(frames.length - 1, Math.floor((t / duration) * frames.length));
      let src = "";
      for (let d = 0; !src && d < frames.length; d++) src = frames[index - d] || frames[index + d] || "";
      return { left: i * width, width, src };
    });
  });
  const rows = $derived(packRows(overlays));
  const selected = $derived(selectedSegment === null ? null : (segments[selectedSegment] ?? null));
  const current = $derived(selected ?? segments[indexAt(segments, time)] ?? null);
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
      playerSize = { w: player.clientWidth, h: player.clientHeight };
      viewWidth = scroller?.clientWidth || viewWidth;
    });
    observer.observe(player);
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
      audioContext?.close();
    };
  });

  function loaded() {
    vw = video.videoWidth;
    vh = video.videoHeight;
    if (!segments.length) {
      duration = video.duration;
      segments = whole(duration);
      trim = { start: 0, end: duration };
      filmstrip();
    }
  }

  /** Frames for the timeline, from a second, hidden player: every 8th
   * first so the strip looks complete quickly, then the ones between. */
  async function filmstrip() {
    const probe = document.createElement("video");
    probe.src = convertFileSrc(path);
    probe.muted = true;
    probe.preload = "auto";
    await new Promise((resolve) => probe.addEventListener("loadeddata", resolve, { once: true }));
    const canvas = document.createElement("canvas");
    canvas.height = 2 * TRACK_H;
    canvas.width = Math.round((canvas.height * probe.videoWidth) / Math.max(1, probe.videoHeight));
    const ctx = canvas.getContext("2d");
    const count = Math.min(120, Math.max(16, Math.ceil(duration * 3)));
    frames = new Array(count).fill("");
    const order = [...Array(count).keys()].sort((a, b) => (a % 8 === 0 ? 0 : 1) - (b % 8 === 0 ? 0 : 1) || a - b);
    for (const i of order) {
      if (!ctx) break;
      probe.currentTime = ((i + 0.5) * duration) / count;
      await new Promise((resolve) => probe.addEventListener("seeked", resolve, { once: true }));
      ctx.drawImage(probe, 0, 0, canvas.width, canvas.height);
      frames[i] = canvas.toDataURL("image/jpeg", 0.75);
    }
    probe.removeAttribute("src");
    probe.load();
  }

  // ── Stage ──────────────────────────────────────────────────────────────

  function hexRgb(hex: string): [number, number, number] {
    const h = hex.replace("#", "");
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) || 0) as [number, number, number];
  }

  /** Pointer position in source pixels. */
  function toSource(e: { clientX: number; clientY: number }): Point {
    const r = stage.getBoundingClientRect();
    const ox = (e.clientX - r.left) / k;
    const oy = (e.clientY - r.top) / k;
    const s = layout.area.w / layout.crop.w;
    return {
      x: Math.max(0, Math.min(vw, (ox - layout.area.x) / s + layout.crop.x)),
      y: Math.max(0, Math.min(vh, (oy - layout.area.y) / s + layout.crop.y)),
    };
  }

  /** Source pixels → CSS pixels in the player. */
  function toPlayer(p: Point) {
    const s = layout.area.w / layout.crop.w;
    return {
      x: stageBox.left + (layout.area.x + (p.x - layout.crop.x) * s) * k,
      y: stageBox.top + (layout.area.y + (p.y - layout.crop.y) * s) * k,
    };
  }

  /** The picture as it will be exported: crop, ratio, background, then
   * overlays, cursor and click marks drawn in source pixels. */
  function paint() {
    if (!stage || !video) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(stageBox.width * dpr);
    const h = Math.round(stageBox.height * dpr);
    if (stage.width !== w || stage.height !== h) {
      stage.width = w;
      stage.height = h;
    }
    const ctx = stage.getContext("2d");
    if (!ctx || !vw) return;
    const { out, crop: c, area } = layout;
    ctx.setTransform(dpr * k, 0, 0, dpr * k, 0, 0);
    ctx.clearRect(0, 0, out.w, out.h);
    if (layout.background) {
      const stops = BACKGROUNDS[background]?.stops ?? ["#222", "#111"];
      const gradient = ctx.createLinearGradient(0, 0, out.w, out.h);
      stops.forEach((stop, i) => gradient.addColorStop(i / Math.max(1, stops.length - 1), stop));
      ctx.fillStyle = gradient;
      ctx.fillRect(0, 0, out.w, out.h);
    }
    ctx.save();
    ctx.beginPath();
    ctx.rect(area.x, area.y, area.w, area.h);
    ctx.clip();
    ctx.imageSmoothingQuality = "high";
    if (video.readyState >= 2) ctx.drawImage(video, c.x, c.y, c.w, c.h, area.x, area.y, area.w, area.h);
    const s = area.w / c.w;
    ctx.translate(area.x - c.x * s, area.y - c.y * s);
    ctx.scale(s, s);
    /** One CSS pixel, in source pixels. */
    const px = 1 / (k * s);
    const t = video.currentTime;

    const active = overlays.filter((o) => t >= o.start && t < o.end && o.id !== editing?.id).map((o) => o.shape);
    renderAnnotations(ctx, draft ? [...active, draft] : active);

    if (cursorTrack && (clickRing || cursorEditable)) {
      const scale = cursorScale;
      if (clickRing) {
        const [r, g, b] = hexRgb(clickColor);
        for (const click of cursorTrack.clicks) {
          if (click.t > t || t - click.t >= CLICK_MARK) continue;
          const p = (t - click.t) / CLICK_MARK;
          const eased = 1 - (1 - p) ** 3;
          ctx.strokeStyle = `rgba(${r},${g},${b},${(1 - p) * 0.9})`;
          ctx.lineWidth = 3 * Math.max(1, scale);
          ctx.beginPath();
          ctx.arc(click.x, click.y, (6 + 26 * eased) * Math.max(1, scale), 0, Math.PI * 2);
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

    // Selection: outline and resize handles.
    if (selectedItem && !cropping && !editing && t >= selectedItem.start && t < selectedItem.end) {
      dashed(ctx, bounds(ctx, selectedItem.shape), px);
      for (const hp of shapeHandles(ctx, selectedItem.shape)) handleDot(ctx, hp, px);
    }
    ctx.restore();

    // Crop frame: the rest dimmed, thirds, corner brackets.
    if (cropping && cropDraft) {
      const r = cropDraft;
      ctx.fillStyle = "rgba(8,8,10,0.62)";
      ctx.fillRect(0, 0, vw, r.y);
      ctx.fillRect(0, r.y + r.h, vw, vh - r.y - r.h);
      ctx.fillRect(0, r.y, r.x, r.h);
      ctx.fillRect(r.x + r.w, r.y, vw - r.x - r.w, r.h);
      const line = 1 / k;
      ctx.strokeStyle = "rgba(255,255,255,0.35)";
      ctx.lineWidth = line;
      ctx.beginPath();
      for (const f of [1 / 3, 2 / 3]) {
        ctx.moveTo(r.x + r.w * f, r.y);
        ctx.lineTo(r.x + r.w * f, r.y + r.h);
        ctx.moveTo(r.x, r.y + r.h * f);
        ctx.lineTo(r.x + r.w, r.y + r.h * f);
      }
      ctx.stroke();
      ctx.strokeStyle = "#fff";
      ctx.lineWidth = 2 * line;
      ctx.strokeRect(r.x, r.y, r.w, r.h);
      const arm = Math.min(24 / k, r.w / 3, r.h / 3);
      ctx.lineWidth = 5 * line;
      ctx.beginPath();
      for (const [i, p] of corners(r).entries()) {
        const sx = i === 0 || i === 3 ? 1 : -1;
        const sy = i < 2 ? 1 : -1;
        ctx.moveTo(p.x + sx * arm, p.y);
        ctx.lineTo(p.x, p.y);
        ctx.lineTo(p.x, p.y + sy * arm);
      }
      ctx.stroke();
    }
  }

  function dashed(ctx: CanvasRenderingContext2D, r: Rect, px: number) {
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

  function handleDot(ctx: CanvasRenderingContext2D, p: Point, px: number) {
    const size = 10 * px;
    ctx.fillStyle = "#fff";
    ctx.strokeStyle = "#8b7cff";
    ctx.lineWidth = 2 * px;
    ctx.beginPath();
    ctx.roundRect(p.x - size / 2, p.y - size / 2, size, size, 3 * px);
    ctx.fill();
    ctx.stroke();
  }

  // ── Playback ───────────────────────────────────────────────────────────

  /** Every frame while playing: skip removed parts, stop at the out point,
   * play each part at its speed, sound the clicks, keep the playhead in
   * view. */
  function follow() {
    if (!video) return;
    time = video.currentTime;
    if (!playing) {
      lastClickCheck = time;
      return;
    }
    const next = nextKept(kept, time);
    if (next === null) {
      video.pause();
      video.currentTime = trim.end;
      return;
    }
    if (next > time + 0.01) video.currentTime = next;
    const rate = partAt(kept, next)?.speed ?? 1;
    if (video.playbackRate !== rate) video.playbackRate = rate;
    // Clicks passed since the last frame (not across a jump).
    if (clickSound && cursorTrack && time > lastClickCheck && time - lastClickCheck < 0.5) {
      for (const c of cursorTrack.clicks) {
        if (c.t > lastClickCheck && c.t <= time) {
          audioContext ??= new AudioContext();
          playClick(audioContext);
        }
      }
    }
    lastClickCheck = time;
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
      ratio: $state.snapshot(ratio),
      fit,
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
    ratio = parsed.ratio;
    fit = parsed.fit;
    overlays = parsed.overlays;
    selectedOverlay = null;
    selectedSegment = null;
    cropping = false;
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
    selectedSegment = (e.target as HTMLElement).closest(".track") ? indexAt(segments, t) : null;
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

  /** Dragging a bar of the overlay lanes changes when it shows. */
  function barDown(e: PointerEvent, o: Overlay, edge: "start" | "end" | "both") {
    e.stopPropagation();
    if (e.button !== 0) return;
    content.setPointerCapture(e.pointerId);
    selectedOverlay = o.id;
    selectedSegment = null;
    tool = "select";
    past.push(snapshot());
    future = [];
    drag = { kind: "bar", id: o.id, edge, from: timeAt(e), start: o.start, end: o.end };
  }

  function timelineMove(e: PointerEvent) {
    if (!drag) return;
    const at = (t: number, exclude?: number) => (e.altKey ? t : snap(t, snapTargets(exclude), reach));
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

  // ── Format and crop ────────────────────────────────────────────────────

  function startCrop() {
    video?.pause();
    editing = null;
    cropDraft = crop ? { ...crop } : lockedRatio && ratio ? ratioCrop(full, ratio) : { ...full };
    cropping = true;
  }

  /** Keeps the frame being edited; the whole frame means no crop. */
  function applyCrop() {
    const r = cropDraft;
    cropping = false;
    cropDraft = null;
    if (!r) return;
    const even = (v: number) => Math.round(v) & ~1;
    const next = { x: even(r.x), y: even(r.y), w: even(r.w), h: even(r.h) };
    const entire = next.x === 0 && next.y === 0 && next.w >= (vw & ~1) && next.h >= (vh & ~1);
    const value = entire && !lockedRatio ? null : next;
    if (JSON.stringify(value) !== JSON.stringify(crop)) commit(() => (crop = value));
  }

  function cancelCrop() {
    cropping = false;
    cropDraft = null;
  }

  function chooseRatio(r: [number, number] | null) {
    if (JSON.stringify(r) === JSON.stringify(ratio)) return;
    commit(() => {
      const wasFill = ratio && fit === "fill";
      ratio = r;
      if (!r) {
        if (wasFill) crop = null;
        cropping = false;
        return;
      }
      if (fit === "fill") {
        const centre = crop ? { x: crop.x + crop.w / 2, y: crop.y + crop.h / 2 } : undefined;
        crop = ratioCrop(full, r, centre);
      }
    });
    if (r && fit === "fill") startCrop();
  }

  function chooseFit(mode: "fill" | "fit") {
    if (mode === fit || !ratio) return;
    commit(() => {
      fit = mode;
      crop = mode === "fill" && ratio ? ratioCrop(full, ratio) : null;
    });
    if (mode === "fill") startCrop();
    else cancelCrop();
  }

  // ── Overlays ───────────────────────────────────────────────────────────

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

  const measureCtx = () => stage.getContext("2d");

  function topmost(p: Point): Overlay | undefined {
    const ctx = measureCtx();
    if (!ctx) return undefined;
    return [...overlays]
      .reverse()
      .find((o) => time >= o.start && time < o.end && hit(ctx, o.shape, p, 8 / sourceScale));
  }

  /** Resize handle of the selected item under `p`, or -1. */
  function handleAt(p: Point): number {
    const ctx = measureCtx();
    const o = selectedItem;
    if (!ctx || !o || time < o.start || time >= o.end) return -1;
    return shapeHandles(ctx, o.shape).findIndex((h) => Math.hypot(h.x - p.x, h.y - p.y) * sourceScale <= 10);
  }

  function chooseTool(t: Tool) {
    tool = t;
    if (cropping) applyCrop();
    if (t !== "select") video?.pause();
  }

  function stageDown(e: PointerEvent) {
    if (!vw || e.button !== 0) return;
    e.preventDefault();
    menu = null;
    if (editing) return commitText();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const p = toSource(e);
    if (cropping && cropDraft) {
      const r = cropDraft;
      const cs = corners(r);
      const near = cs.findIndex((c) => Math.hypot(c.x - p.x, c.y - p.y) * k <= 16);
      if (near >= 0) drag = { kind: "cropCorner", anchor: cs[(near + 2) % 4] as Point };
      else if (p.x > r.x && p.x < r.x + r.w && p.y > r.y && p.y < r.y + r.h)
        drag = { kind: "cropMove", from: p, rect: { ...r } };
      else {
        drag = { kind: "cropCorner", anchor: p };
        cropDraft = { ...p, w: 1, h: 1 };
      }
      return;
    }
    if (tool === "select") {
      const handle = handleAt(p);
      if (handle >= 0 && selectedItem) {
        video.pause();
        past.push(snapshot());
        drag = { kind: "resize", handle, original: $state.snapshot(selectedItem.shape) as Shape, moved: false };
        return;
      }
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
    if (!vw) return;
    const p = toSource(e);
    if (!drag) {
      hover = !cropping && tool === "select" ? handleAt(p) : -1;
      return;
    }
    if (drag.kind === "cropCorner") {
      cropDraft = dragCorner(drag.anchor, p, full, lockedRatio);
    } else if (drag.kind === "cropMove") {
      const r = drag.rect;
      cropDraft = clampRect({ ...r, x: r.x + p.x - drag.from.x, y: r.y + p.y - drag.from.y }, full);
    } else if (drag.kind === "resize") {
      const ctx = measureCtx();
      const o = selectedItem;
      if (!ctx || !o) return;
      drag.moved = true;
      o.shape = resizeShape(ctx, drag.original, drag.handle, p);
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
      drag.moved ||= Math.hypot(p.x - drag.start.x, p.y - drag.start.y) * sourceScale > 3;
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
    if (current.kind === "cropCorner" && cropDraft && (cropDraft.w < 16 || cropDraft.h < 16)) {
      cropDraft = crop ? { ...crop } : { ...full };
    } else if (current.kind === "move" || current.kind === "resize") {
      if (!current.moved) past.pop();
      else future = [];
    } else if (current.kind === "draw" && draft) {
      const shape = draft;
      draft = null;
      if (current.moved) addOverlay(shape);
    }
  }

  function stageDouble(e: MouseEvent) {
    if (cropping) return applyCrop();
    const found = topmost(toSource(e));
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
    const existing = overlays.find((o) => o.id === editing?.id);
    const size = fontSize(existing?.shape.size ?? (level + 1) * unit) * sourceScale;
    const at = toPlayer(editing);
    return `left:${at.x}px;top:${at.y}px;font-size:${size}px;color:${existing?.shape.color ?? color}`;
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
      canvas: ratio && fit === "fit" ? { ratio: [ratio[0], ratio[1]], colors: bg.map(hexRgb), padding: margin } : null,
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
      !(ratio && fit === "fit") &&
      !overlays.length &&
      !clickRing &&
      !clickSound &&
      cursorVisible &&
      cursorScale === 1 &&
      cursorStyle === "original" &&
      !halo,
  );

  async function exportAs(kind: "mp4" | "gif", copy = false) {
    if (cropping) applyCrop();
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

  function listen() {
    audioContext ??= new AudioContext();
    playClick(audioContext);
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
        if (cropping) applyCrop();
        else startCrop();
        break;
      case "enter":
        if (cropping) applyCrop();
        break;
      case "escape":
        if (cropping) cancelCrop();
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
    cropping
      ? "crosshair"
      : tool === "select"
        ? hover === 0 || hover === 2
          ? "nwse-resize"
          : hover === 1 || hover === 3
            ? "nesw-resize"
            : "default"
        : tool === "text"
          ? "text"
          : "crosshair",
  );
  const outputSize = $derived.by(() => {
    if (!vw) return "";
    const fitted = ratio && fit === "fit" ? fitLayout(crop ?? full, ratio, margin).out : null;
    const w = fitted?.w ?? crop?.w ?? vw;
    const h = fitted?.h ?? crop?.h ?? vh;
    return `${Math.round(w) & ~1}×${Math.round(h) & ~1}`;
  });
  const barIcon = (kind: ShapeKind): IconName =>
    (["text", "arrow", "line", "rect", "ellipse", "pen", "highlight", "number"] as const).find((k) => k === kind) ?? "rect";
  /** A short tip over the video for what the current tool does. */
  const hint = $derived(
    cropping
      ? `Drag the frame or its corners${lockedRatio ? ` · ${ratio?.join(":")} kept` : ""} · Enter to apply`
      : selectedItem
        ? "Drag to move · corners to resize · double-click a text to edit"
        : tool !== "select"
          ? `Click or drag on the video · shows ${OVERLAY_LENGTH} s`
          : "",
  );
  const DITHER_TOP = ditherBand("top");
  const DITHER_BOTTOM = ditherBand("bottom");
</script>

<svelte:window onkeydown={keyDown} />

<div class="editor" style="--dither-top:{DITHER_TOP};--dither-bottom:{DITHER_BOTTOM}">
  <div class="main">
    <nav class="toolbox" aria-label="Tools">
      <div class="cluster">
        <button type="button" class="tool" title="Undo (Ctrl+Z)" aria-label="Undo" disabled={!past.length} onclick={undo}
          ><Icon name="undo" /></button
        >
        <button type="button" class="tool" title="Redo (Ctrl+Y)" aria-label="Redo" disabled={!future.length} onclick={redo}
          ><Icon name="redo" /></button
        >
      </div>
      <span class="sep"></span>
      <div class="cluster" role="toolbar">
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
        <button
          type="button"
          class="tool"
          class:active={cropping}
          title="Crop: choose the area to keep (C)"
          aria-label="Crop"
          aria-pressed={cropping}
          onclick={() => (cropping ? applyCrop() : startCrop())}
        >
          <Icon name="crop" />
          <span class="key">C</span>
        </button>
      </div>
      <span class="sep"></span>
      <div class="cluster colors" role="group" aria-label="Colour">
        {#each COLORS as c (c)}
          <button type="button" class="swatch" class:active={color === c} style="--swatch:{c}" aria-label="Colour {c}" onclick={() => setColor(c)}
          ></button>
        {/each}
        <label class="custom" title="Any colour"
          ><input type="color" value={color} onchange={(e) => setColor(e.currentTarget.value)} /></label
        >
      </div>
      <span class="sep"></span>
      <div class="cluster" role="group" aria-label="Thickness">
        {#each LEVELS as l (l.level)}
          <button type="button" class="tool" class:active={level === l.level} title={l.label} aria-label={l.label} onclick={() => setLevel(l.level)}
            ><span class="dot" style="--d:{3 + l.level * 2}px"></span></button
          >
        {/each}
      </div>
    </nav>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="player"
      bind:this={player}
      onpointerdown={stageDown}
      onpointermove={stageMove}
      onpointerup={stageUp}
      ondblclick={stageDouble}
      style="cursor:{stageCursor}"
    >
      <!-- Plays the sound and feeds the stage; never shown itself. -->
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        bind:this={video}
        src={source}
        preload="auto"
        onloadedmetadata={loaded}
        onplay={() => (playing = true)}
        onpause={() => (playing = false)}
        onerror={() => (error = "This video cannot be played here, but it can still be exported.")}
      ></video>
      <canvas
        bind:this={stage}
        class="stage"
        class:framed={layout.background}
        style="left:{stageBox.left}px;top:{stageBox.top}px;width:{stageBox.width}px;height:{stageBox.height}px"
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
        <div class="crop-bar" onpointerdown={(e) => e.stopPropagation()} role="toolbar" aria-label="Crop" tabindex="-1">
          <span>{cropDraft ? `${Math.round(cropDraft.w) & ~1} × ${Math.round(cropDraft.h) & ~1}` : ""}</span>
          <button type="button" class="ghost" onclick={() => (cropDraft = lockedRatio && ratio ? ratioCrop(full, ratio) : { ...full })}
            >Reset</button
          >
          <button type="button" class="ghost" onclick={cancelCrop}>Cancel <Kbd keys="Esc" /></button>
          <button type="button" class="pill" onclick={applyCrop}><Icon name="check" /> Done <Kbd keys="Enter" /></button>
        </div>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      {#if toast}<div class="toast" role="status">{toast}</div>{/if}
      {#if hint}<div class="hint">{hint}</div>{/if}
    </div>

    <footer class="dock">
      <div class="controls">
        <div class="side left">
          <button type="button" class="act" onclick={splitHere} disabled={!duration} title="Split at the playhead (S)"
            ><Icon name="split" /> <span class="lbl">Split</span> <Kbd keys="S" /></button
          >
          <button
            type="button"
            class="act"
            class:danger={current?.kept !== false}
            onclick={() => removePart()}
            disabled={!duration}
            title={current?.kept === false ? "Restore this part (Del)" : "Delete this part (Del)"}
            ><Icon name={current?.kept === false ? "restore" : "trash"} />
            <span class="lbl">{current?.kept === false ? "Restore" : "Delete"}</span> <Kbd keys="Del" /></button
          >
          <div class="anchor">
            <button
              type="button"
              class="act opens-menu"
              class:on={(current?.speed ?? 1) !== 1}
              disabled={!duration}
              aria-expanded={menu === "speed"}
              title="Speed of this part"
              onclick={() => (menu = menu === "speed" ? null : "speed")}
              ><Icon name="speed" /> {speedLabel(current?.speed ?? 1)} <Icon name="chevron" /></button
            >
            {#if menu === "speed"}
              <div class="popover speeds" role="menu">
                {#each SPEEDS as sp (sp)}
                  <button type="button" role="menuitem" class:active={(current?.speed ?? 1) === sp} onclick={() => speedPart(sp)}
                    >{speedLabel(sp)}</button
                  >
                {/each}
                <div class="all">
                  All parts
                  {#each [1, 1.5, 2] as sp (sp)}<button type="button" class="chip" onclick={() => allSpeeds(sp)}>{speedLabel(sp)}</button>{/each}
                </div>
              </div>
            {/if}
          </div>
        </div>

        <div class="center">
          <button type="button" class="round skip" title="Go to the start (Home)" aria-label="Go to the start" onclick={() => seek(trim.start)}
            ><Icon name="start" /></button
          >
          <button type="button" class="round play" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onclick={togglePlay}>
            <Icon name={playing ? "pause" : "play"} />
          </button>
          <button type="button" class="round skip" title="Go to the end (End)" aria-label="Go to the end" onclick={() => seek(trim.end - 0.001)}
            ><Icon name="end" /></button
          >
          <span class="time">{format(time)} <span class="muted">/ {format(output)}</span></span>
        </div>

        <div class="side right">
          <span class="hint-keys"><Kbd keys="I" /><Kbd keys="O" /> trim · <Kbd keys="Alt" /> free</span>
          <div class="zoom">
            <button type="button" class="round small" aria-label="Zoom out" title="Zoom out (Ctrl+-)" disabled={zoom <= 1} onclick={() => zoomBy(1 / 1.5)}
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
            <button type="button" class="round small" aria-label="Zoom in" title="Zoom in (Ctrl++)" disabled={zoom >= MAX_ZOOM} onclick={() => zoomBy(1.5)}
              ><Icon name="plus" /></button
            >
          </div>
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

          <div class="track" style="height:{TRACK_H}px">
            <div class="filmstrip">
              {#each tiles as tile, i (i)}
                {#if tile.src}<img src={tile.src} alt="" draggable="false" style="left:{tile.left}px;width:{tile.width}px" />{/if}
              {/each}
            </div>
            {#each segments as sg, i (sg.start)}
              <div
                class="segment"
                class:removed={!sg.kept}
                class:selected={selectedSegment === i}
                class:cut={i > 0}
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
            <div class="outside" style="left:0;width:{percent(trim.start)}%"></div>
            <div class="outside" style="left:{percent(trim.end)}%;right:0"></div>
            <div class="trim-frame" style="left:{percent(trim.start)}%;width:{percent(trim.end) - percent(trim.start)}%"></div>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="handle start" style="left:{percent(trim.start)}%" title="Drag to trim the start (I)" onpointerdown={(e) => trimDown(e, "start")}></div>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="handle end" style="left:{percent(trim.end)}%" title="Drag to trim the end (O)" onpointerdown={(e) => trimDown(e, "end")}></div>
          </div>

          <div class="lanes" style="height:{rows.count * ROW_H + 6}px">
            {#if !overlays.length}
              <span class="lane-hint">Texts and shapes appear here — drag a bar to choose when it shows</span>
            {/if}
            {#each overlays as o (o.id)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="bar"
                class:selected={selectedOverlay === o.id}
                style="left:{percent(o.start)}%;width:{percent(o.end) - percent(o.start)}%;top:{3 + (rows.row.get(o.id) ?? 0) * ROW_H}px;--bar:{o.shape.color}"
                onpointerdown={(e) => barDown(e, o, "both")}
              >
                <span class="edge left" onpointerdown={(e) => barDown(e, o, "start")}></span>
                <span class="label"
                  ><Icon name={barIcon(o.shape.kind)} />{o.shape.kind === "text"
                    ? o.shape.text
                    : o.shape.kind === "number"
                      ? `Step ${o.shape.n}`
                      : o.shape.kind}</span
                >
                <span class="edge right" onpointerdown={(e) => barDown(e, o, "end")}></span>
              </div>
            {/each}
          </div>

          <div class="playhead" style="left:{percent(time)}%"><span class="knob"></span></div>
        </div>
      </div>
    </footer>
  </div>

  <aside class="side-panel">
    <header class="panel-head">
      <div class="file" title={path}><span class="glyph"><Icon name="play" /></span>{name(path)}</div>
      <div class="sub">{format(output)}{outputSize ? ` · ${outputSize}` : ""}{kept.length > 1 ? ` · ${kept.length} parts` : ""}</div>
    </header>

    <div class="panel-scroll">
      {#if selectedItem}
        <details open>
          <summary>Selected item</summary>
          <p class="note">Shows {format(selectedItem.start)} → {format(selectedItem.end)}.</p>
          <div class="pair">
            <button type="button" onclick={() => setOverlayTime("start")}>From playhead</button>
            <button type="button" onclick={() => setOverlayTime("end")}>Until playhead</button>
          </div>
          <button type="button" class="wide danger-text" onclick={removeOverlay}><Icon name="trash" /> Delete <Kbd keys="Del" /></button>
        </details>
      {/if}
      <details open>
        <summary>Format</summary>
        <div class="ratios">
          {#each RATIOS as r (r.label)}
            {@const active = (ratio?.join(":") ?? null) === (r.ratio?.join(":") ?? null)}
            <button type="button" class="ratio" class:active onclick={() => chooseRatio(r.ratio)} aria-pressed={active}>
              <span
                class="shape"
                style={r.ratio
                  ? `aspect-ratio:${r.ratio[0]}/${r.ratio[1]};${r.ratio[0] >= r.ratio[1] ? "width:22px" : "height:22px"}`
                  : "width:22px;height:15px;border-style:dashed"}
              ></span>
              {r.label}
            </button>
          {/each}
        </div>
        {#if ratio}
          <div class="segmented" role="radiogroup" aria-label="How the video fills the format">
            <button type="button" class:active={fit === "fill"} role="radio" aria-checked={fit === "fill"} onclick={() => chooseFit("fill")}
              >Fill · crop</button
            >
            <button type="button" class:active={fit === "fit"} role="radio" aria-checked={fit === "fit"} onclick={() => chooseFit("fit")}
              >Fit · background</button
            >
          </div>
          {#if fit === "fit"}
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
          {:else}
            <p class="note">The video fills the {ratio.join(":")} frame, no borders: drag it to choose what stays visible.</p>
          {/if}
        {/if}
        <div class="pair">
          <button type="button" class:active={cropping} onclick={() => (cropping ? applyCrop() : startCrop())}
            ><Icon name="crop" /> {ratio && fit === "fill" ? "Reframe" : "Crop"}</button
          >
          <button type="button" disabled={!crop || !!(ratio && fit === "fill")} onclick={() => commit(() => (crop = null))}>Reset</button>
        </div>
      </details>
      <details open>
        <summary>Cursor</summary>
        {#if !cursorTrack}
          <p class="note">Needs a Kadra recording made with “editable cursor” on (Settings › Recording).</p>
        {:else if cursorEditable}
          <label class="check"><input type="checkbox" bind:checked={cursorVisible} /> Show the pointer</label>
          <label class="field"
            >Size <input type="range" min="0.5" max="4" step="0.25" bind:value={cursorScale} />
            <span class="value">{cursorScale}×</span></label
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
          <summary>Clicks</summary>
          <label class="check"
            ><input type="checkbox" bind:checked={clickRing} /> Ring
            <input type="color" bind:value={clickColor} class="mini" title="Ring colour" /></label
          >
          <div class="check">
            <label><input type="checkbox" bind:checked={clickSound} /> Soft click sound</label>
            <button type="button" class="ghost tiny" onclick={listen}>Listen</button>
          </div>
        </details>
      {/if}
    </div>

    <div class="export-card">
      <div class="card-title">Export</div>
      <div class="segmented" role="radiogroup" aria-label="Quality">
        {#each QUALITIES as q (q.value)}
          <button type="button" role="radio" aria-checked={quality === q.value} class:active={quality === q.value} onclick={() => (quality = q.value)}
            >{q.label}</button
          >
        {/each}
      </div>
      <label class="check"><input type="checkbox" bind:checked={audio} /> Keep the sound</label>
      <button type="button" class="pill big" onclick={() => exportAs("mp4")} disabled={!duration}
        ><Icon name="check" /> Export MP4 <Kbd keys="Ctrl+E" /></button
      >
      <div class="pair">
        <button type="button" class="ghost" onclick={copyVideo} disabled={!duration} title="Copy the result to the clipboard (Ctrl+C)"
          ><Icon name="copy" /> Copy</button
        >
        <button type="button" class="ghost" onclick={() => exportAs("gif")} disabled={!config || !duration}><Icon name="export" /> GIF</button>
      </div>
      <button type="button" class="link" aria-expanded={gifOptions} onclick={() => (gifOptions = !gifOptions)}
        >GIF settings <Icon name="chevron" /></button
      >
      {#if gifOptions}
        <div class="pair">
          <select bind:value={gifFps} aria-label="GIF frame rate">{#each GIF_FPS_PRESETS as fv (fv)}<option value={fv}>{fv} fps</option>{/each}</select>
          <select bind:value={gifWidth} aria-label="GIF width"
            >{#each GIF_WIDTHS as w (w)}<option value={w}>{w === 0 ? "Original size" : `${w} px wide`}</option>{/each}</select
          >
        </div>
      {/if}
      {#each jobs.slice(0, 3) as j (j.job)}
        <div class="job">
          <span class="kind">{j.kind === "copy" ? "Copy" : j.kind.toUpperCase()}</span>
          {#if j.error}
            <span class="failed" title={j.error}>{j.error}</span>
          {:else if j.output}
            <span class="name" title={j.output}>{name(j.output)} · {size(j.bytes ?? 0)}</span>
            <button type="button" class="round small" title="Show in folder" aria-label="Show in folder" onclick={() => j.output && showInFolder(j.output)}
              ><Icon name="folder" /></button
            >
          {:else}
            <span class="bar-progress"><span style="width:{Math.round(j.done * 100)}%"></span></span>
            <span class="pct">{Math.round(j.done * 100)}%</span>
          {/if}
        </div>
      {/each}
    </div>
  </aside>
</div>

<style>
  /* Dark, soft and rounded whatever the system theme: an editing room. */
  .editor {
    --bg: #0c0d11;
    --surface: #181816;
    --raised: #1f1f1c;
    --control: #232320;
    --hover: #2b2b27;
    --border: #262622;
    --border-strong: #35352f;
    --text: #ecece8;
    --text-muted: #8f8f88;
    --accent: #8b7cff;
    --accent-soft: rgb(139 124 255 / 0.18);
    --on-accent: #ffffff;
    --trim: #f5c542;
    --danger: #ff8a8a;
    color-scheme: dark;
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(250px, 24vw, 300px);
    height: 100vh;
    min-width: 640px;
    overflow: hidden;
    background: var(--bg);
    color: var(--text);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 10px 0 10px 10px;
    gap: 10px;
  }
  .editor :global(button),
  .editor :global(select),
  .editor :global(input) {
    color: var(--text);
    background: var(--control);
    border-color: var(--border);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border-radius: 8px;
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button.active {
    background: var(--accent-soft) !important;
    border-color: var(--accent) !important;
    color: #fff;
  }
  .sep {
    width: 1px;
    align-self: stretch;
    margin: 6px 4px;
    background: var(--border);
  }
  .muted {
    color: var(--text-muted);
  }

  /* Toolbox */
  .toolbox {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--dither-top) repeat-x left top / auto 32px, var(--surface);
    container-type: inline-size;
  }
  .cluster {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .colors {
    gap: 6px;
    padding: 0 4px;
  }
  .tool {
    position: relative;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border: 1px solid transparent !important;
    background: transparent !important;
    border-radius: 10px;
  }
  .tool:hover:not(:disabled):not(.active) {
    background: var(--hover) !important;
  }
  .tool.active {
    background: var(--accent-soft) !important;
    border-color: color-mix(in srgb, var(--accent) 60%, transparent) !important;
  }
  .tool .key {
    position: absolute;
    right: 3px;
    bottom: 1px;
    font-size: 8px;
    font-weight: 700;
    font-family: var(--font-mono);
    opacity: 0.5;
  }
  .swatch {
    width: 20px;
    height: 20px;
    padding: 0;
    border-radius: 50%;
    background: var(--swatch) !important;
    border: 2px solid rgb(255 255 255 / 0.12) !important;
  }
  .swatch.active {
    outline: 2px solid #fff;
    outline-offset: 2px;
  }
  .custom {
    position: relative;
    width: 20px;
    height: 20px;
    border-radius: 50%;
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
  .player {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: radial-gradient(circle at 50% 40%, #1d1d22, #101013 70%);
    touch-action: none;
  }
  video {
    position: absolute;
    width: 2px;
    height: 2px;
    opacity: 0;
    pointer-events: none;
  }
  .stage {
    position: absolute;
    border-radius: 4px;
    box-shadow: 0 10px 40px rgb(0 0 0 / 0.45);
  }
  .stage.framed {
    border-radius: 8px;
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
  .crop-bar {
    position: absolute;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 6px 6px 14px;
    border: 1px solid var(--border-strong);
    border-radius: 999px;
    background: rgb(26 26 30 / 0.92);
    backdrop-filter: blur(8px);
    cursor: default;
  }
  .crop-bar span {
    font: 12px var(--font-mono);
    color: var(--text-muted);
    margin-right: 6px;
  }
  .toast,
  .hint,
  .error {
    position: absolute;
    left: 50%;
    top: 14px;
    transform: translateX(-50%);
    padding: 7px 14px;
    border-radius: 999px;
    background: var(--raised);
    border: 1px solid var(--border-strong);
    font-size: 13px;
    pointer-events: none;
  }
  .toast {
    top: auto;
    bottom: 14px;
  }
  .hint {
    top: 12px;
    max-width: calc(100% - 24px);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: 12px;
    background: rgb(24 24 22 / 0.88);
    backdrop-filter: blur(6px);
  }
  .error {
    color: var(--danger);
  }

  .pill {
    justify-content: center;
    padding: 6px 14px;
    border: none !important;
    border-radius: 999px;
    background: #f4f4f6 !important;
    color: #141416 !important;
    font-weight: 600;
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.3);
  }
  .pill:hover:not(:disabled) {
    background: #ffffff !important;
  }
  .pill.big {
    width: 100%;
    padding: 9px 14px;
    margin: 4px 0 8px;
  }
  .ghost {
    justify-content: center;
    padding: 6px 12px;
    border: 1px solid var(--border-strong) !important;
    border-radius: 999px;
    background: transparent !important;
  }
  .ghost:hover:not(:disabled) {
    background: var(--hover) !important;
  }
  .ghost.tiny {
    padding: 1px 10px;
    margin-left: auto;
    font-size: 12px;
  }
  .link {
    border: none !important;
    background: none !important;
    color: var(--text-muted);
    padding: 2px 0;
    margin-bottom: 4px;
    font-size: 12px;
  }
  .link:hover {
    color: var(--text);
  }

  /* Dock: transport and timeline */
  .dock {
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--dither-bottom) repeat-x left bottom / auto 32px, var(--surface);
    container-type: inline-size;
  }
  .controls {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }
  .side {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .side.right {
    justify-content: flex-end;
  }
  .act {
    height: 34px;
    padding: 0 12px;
    border: 1px solid var(--border-strong) !important;
    background: var(--raised) !important;
    border-radius: 10px;
    font-weight: 500;
    white-space: nowrap;
  }
  .act:hover:not(:disabled) {
    background: var(--hover) !important;
  }
  .act.danger:hover:not(:disabled) {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 50%, transparent) !important;
  }
  .act.on {
    color: var(--trim);
    border-color: color-mix(in srgb, var(--trim) 50%, transparent) !important;
  }
  .center {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .round {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border-radius: 50% !important;
    border: 1px solid var(--border-strong) !important;
    background: var(--raised) !important;
  }
  .round.small {
    width: 26px;
    height: 26px;
  }
  .round:hover:not(:disabled) {
    background: var(--hover) !important;
  }
  .round.play {
    width: 44px;
    height: 44px;
    border: none !important;
    background: #f4f4f6 !important;
    color: #141416 !important;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.35);
  }
  .round.play:hover {
    background: #fff !important;
  }
  .time {
    min-width: 140px;
    font: 500 13px var(--font-mono);
  }
  .hint-keys {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
  }
  .zoom {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .zoom input {
    width: 100px;
    padding: 0;
    accent-color: var(--accent);
    background: transparent !important;
  }
  .anchor {
    position: relative;
  }
  .popover {
    position: absolute;
    z-index: 20;
    padding: 8px;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--raised);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.5);
  }
  .speeds {
    bottom: calc(100% + 8px);
    left: 0;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    width: 230px;
  }
  .speeds > button {
    justify-content: center;
    font-family: var(--font-mono);
    border-color: transparent;
  }
  .speeds .all {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 4px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-muted);
  }
  .chip {
    padding: 2px 9px;
    border-radius: 999px;
  }

  /* Timeline */
  .scroller {
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0 12px;
    scrollbar-width: thin;
    scrollbar-color: var(--border-strong) transparent;
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
    padding-left: 4px;
    border-left: 1px solid var(--border-strong);
    height: 10px;
    font-size: 10px;
    line-height: 10px;
    font-family: var(--font-mono);
    color: var(--text-muted);
    white-space: nowrap;
    pointer-events: none;
  }
  .track {
    position: relative;
    border-radius: 10px;
    background: #0d0d10;
    cursor: pointer;
  }
  .filmstrip {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: 10px;
    pointer-events: none;
  }
  .filmstrip img {
    position: absolute;
    top: 0;
    height: 100%;
    object-fit: cover;
    border-right: 1px solid rgb(0 0 0 / 0.35);
  }
  .segment {
    position: absolute;
    top: 0;
    bottom: 0;
  }
  .segment.cut::before {
    content: "";
    position: absolute;
    left: -1.5px;
    top: 0;
    bottom: 0;
    width: 3px;
    background: var(--surface);
  }
  .segment.removed {
    background: repeating-linear-gradient(135deg, rgb(10 10 12 / 0.82) 0 6px, rgb(10 10 12 / 0.62) 6px 12px);
  }
  .segment.selected {
    box-shadow: inset 0 0 0 2px var(--accent);
    background-color: rgb(139 124 255 / 0.12);
  }
  .badge {
    position: absolute;
    left: 8px;
    bottom: 5px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--trim);
    color: #1a1400;
    font: 700 11px/17px var(--font-mono);
    pointer-events: none;
  }
  .part-tools {
    position: absolute;
    top: 5px;
    right: 5px;
    display: none;
    gap: 3px;
    z-index: 3;
  }
  .segment:hover .part-tools,
  .segment.selected .part-tools {
    display: flex;
  }
  .part-tools button {
    height: 24px;
    min-width: 24px;
    padding: 0 6px;
    justify-content: center;
    border: none !important;
    border-radius: 999px;
    background: rgb(15 15 18 / 0.85) !important;
    color: #fff;
    font: 600 11px var(--font-mono);
  }
  .part-tools button:hover {
    background: #f4f4f6 !important;
    color: #141416;
  }
  .part-tools :global(svg) {
    width: 14px;
    height: 14px;
  }
  .outside {
    position: absolute;
    top: 0;
    bottom: 0;
    background: rgb(8 8 10 / 0.72);
    pointer-events: none;
  }
  .trim-frame {
    position: absolute;
    top: 0;
    bottom: 0;
    border-top: 3px solid var(--trim);
    border-bottom: 3px solid var(--trim);
    pointer-events: none;
  }
  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 14px;
    background: var(--trim);
    cursor: ew-resize;
    z-index: 4;
  }
  .handle.start {
    border-radius: 8px 0 0 8px;
    transform: translateX(-12px);
  }
  .handle.end {
    border-radius: 0 8px 8px 0;
    transform: translateX(-2px);
  }
  .handle::after {
    content: "";
    position: absolute;
    left: 5px;
    top: 50%;
    width: 4px;
    height: 18px;
    margin-top: -9px;
    border-left: 1.5px solid #5c4300;
    border-right: 1.5px solid #5c4300;
  }
  .handle:hover {
    filter: brightness(1.08);
  }
  .lanes {
    position: relative;
    margin-top: 6px;
    border-radius: 10px;
    background: rgb(255 255 255 / 0.03);
    transition: height 0.15s;
  }
  .lane-hint {
    position: absolute;
    left: 12px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 11px;
    color: var(--text-muted);
    pointer-events: none;
    white-space: nowrap;
  }
  .bar {
    position: absolute;
    height: 22px;
    display: flex;
    align-items: center;
    border-radius: 7px;
    background: color-mix(in srgb, var(--bar) 30%, #1b1b20);
    border: 1px solid color-mix(in srgb, var(--bar) 70%, transparent);
    cursor: grab;
    overflow: hidden;
  }
  .bar.selected {
    outline: 2px solid #fff;
    outline-offset: 1px;
  }
  .bar .label {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    color: #fff;
    pointer-events: none;
  }
  .bar .label :global(svg) {
    flex: none;
    width: 12px;
    height: 12px;
    opacity: 0.8;
  }
  .edge {
    width: 6px;
    align-self: stretch;
    cursor: ew-resize;
    background: color-mix(in srgb, var(--bar) 80%, white);
    opacity: 0.6;
  }
  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: #fff;
    box-shadow: 0 0 3px rgb(0 0 0 / 0.8);
    pointer-events: none;
    z-index: 5;
  }
  .knob {
    position: absolute;
    top: 0;
    left: -5px;
    width: 12px;
    height: 12px;
    border-radius: 3px 3px 7px 7px;
    background: #fff;
  }

  /* Right panel */
  .side-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    margin: 10px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--surface);
    overflow: hidden;
  }
  .panel-head {
    padding: 14px 16px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--dither-top) repeat-x left top / auto 48px;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 15px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .glyph {
    flex: none;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 7px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .glyph :global(svg) {
    width: 12px;
    height: 12px;
  }
  .sub {
    margin-top: 4px;
    font: 12px var(--font-mono);
    color: var(--text-muted);
  }
  .panel-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: var(--border-strong) transparent;
  }
  details {
    padding: 10px 16px 14px;
    border-bottom: 1px solid var(--border);
  }
  summary {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    cursor: pointer;
    list-style: none;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary::after {
    content: "";
    margin-left: auto;
    width: 6px;
    height: 6px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: rotate(45deg);
  }
  details:not([open]) summary {
    margin-bottom: 0;
  }
  details:not([open]) summary::after {
    transform: rotate(-45deg);
  }
  summary:hover {
    color: var(--text);
  }
  .ratios {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
    margin-bottom: 10px;
  }
  .ratio {
    flex-direction: column;
    justify-content: center;
    gap: 4px;
    height: 56px;
    padding: 4px;
    font-size: 12px;
    border-radius: 10px;
  }
  .ratio .shape {
    display: block;
    border: 1.5px solid currentColor;
    border-radius: 3px;
    opacity: 0.8;
  }
  .segmented {
    display: flex;
    padding: 3px;
    gap: 3px;
    margin-bottom: 10px;
    border-radius: 10px;
    background: var(--bg);
    border: 1px solid var(--border);
  }
  .segmented button {
    flex: 1;
    justify-content: center;
    padding: 4px 6px;
    font-size: 12px;
    border: 1px solid transparent !important;
    background: transparent !important;
    border-radius: 7px;
  }
  .segmented button.active {
    background: var(--hover) !important;
    border-color: var(--border-strong) !important;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 4px 0 8px;
  }
  .wide-swatch {
    width: 36px;
    border-radius: 8px;
  }
  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .pair button {
    justify-content: center;
  }
  .pair select {
    min-width: 0;
  }
  button.wide {
    width: 100%;
    justify-content: center;
    margin-top: 6px;
  }
  .danger-text:hover {
    color: var(--danger);
  }
  .note {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 8px;
  }
  .check,
  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 7px 0;
    font-size: 13px;
  }
  .check label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .editor input[type="checkbox"] {
    accent-color: var(--accent);
  }
  .field input[type="range"] {
    flex: 1;
    min-width: 0;
    padding: 0;
    accent-color: var(--accent);
    background: transparent !important;
  }
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
    border: none !important;
    background: none !important;
    margin-left: auto;
  }

  /* Export card */
  .export-card {
    padding: 12px 14px 14px;
    border-top: 1px solid var(--border);
    background: var(--dither-bottom) repeat-x left bottom / auto 48px, var(--surface);
  }
  .card-title {
    margin-bottom: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .job {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 28px;
    margin-top: 6px;
    font-size: 12px;
  }
  .job .kind {
    font-weight: 600;
    min-width: 34px;
  }
  .job .name,
  .failed {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar-progress {
    flex: 1;
    height: 4px;
    border-radius: 2px;
    background: var(--border);
    overflow: hidden;
  }
  .bar-progress span {
    display: block;
    height: 100%;
    background: linear-gradient(90deg, #b8860b, var(--trim));
    transition: width 0.2s;
  }
  .pct {
    font-family: var(--font-mono);
    color: var(--text-muted);
  }
  .failed {
    color: var(--danger);
  }

  /* Smaller windows: the transport row gives up hints, shortcuts, then
   * labels, so play stays centred and nothing overlaps. */
  @container (max-width: 980px) {
    .hint-keys {
      display: none;
    }
  }
  @container (max-width: 820px) {
    .act :global(.kbd) {
      display: none;
    }
    .zoom input {
      width: 64px;
    }
    .time {
      min-width: 0;
      font-size: 12px;
    }
  }
  @container (max-width: 660px) {
    .act .lbl,
    .zoom input {
      display: none;
    }
    .act {
      padding: 0 9px;
    }
    .center {
      gap: 5px;
    }
    .round.play {
      width: 38px;
      height: 38px;
    }
    .round.skip {
      display: none;
    }
  }
  /* The tool bar stays on one row as long as it can. */
  @container (max-width: 900px) {
    .tool {
      width: 31px;
      height: 31px;
    }
    .colors {
      gap: 4px;
    }
    .swatch,
    .custom {
      width: 17px;
      height: 17px;
    }
    .sep {
      margin: 6px 2px;
    }
  }
  /* Short windows: a thinner timeline leaves room for the picture. */
  @media (max-height: 720px) {
    .main {
      gap: 6px;
      padding-top: 6px;
      padding-bottom: 6px;
    }
    .track {
      height: 44px !important;
    }
    .ratio {
      height: 46px;
    }
    .side-panel {
      margin: 6px;
    }
  }
</style>
