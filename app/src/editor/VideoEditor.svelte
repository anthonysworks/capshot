<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, untrack } from "svelte";
  import {
    type Composition,
    type Config,
    type CursorTrack,
    type JobProgress,
    type VideoEdit,
    GIF_FPS_PRESETS,
    GIF_WIDTHS,
    editorChooseFolder,
    editorCopyFile,
    editorDiscard,
    editorExport,
    editorSaveAs,
    editorVideoSources,
    getConfig,
    onJobProgress,
  } from "../lib/api";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import Kbd from "./Kbd.svelte";
  import { CLICK_MARK, CURSOR_STYLES, type CursorStyle, cursorAt, drawCursor, loadShapes } from "./cursors";
  import { VIVID } from "./colors";
  import {
    BACKGROUNDS,
    FREEHAND,
    LINEAR,
    type Point,
    type Rect,
    type Shape,
    type ShapeKind,
    drawnBend,
    bounds,
    fontSize,
    hit,
    normalize,
    TEXT_FONTS,
    textFont,
    type TextFont,
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
    CLICK_SOUNDS,
    type ClickSound,
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
    join,
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

  type Tool = "select" | "text" | "arrow" | "curve" | "line" | "rect" | "ellipse" | "highlight" | "pen" | "number" | "pixelate";
  const TOOLS: { tool: Tool; icon: IconName; label: string; key: string }[] = [
    { tool: "select", icon: "select", label: "Select, move, resize — click the video to play", key: "V" },
    { tool: "text", icon: "text", label: "Text", key: "T" },
    { tool: "arrow", icon: "arrow", label: "Arrow", key: "A" },
    { tool: "curve", icon: "curve", label: "Curved arrow (drag its middle to bend it)", key: "Q" },
    { tool: "line", icon: "line", label: "Line", key: "L" },
    { tool: "rect", icon: "rect", label: "Rectangle", key: "R" },
    { tool: "ellipse", icon: "ellipse", label: "Ellipse", key: "E" },
    { tool: "pen", icon: "pen", label: "Pencil", key: "P" },
    { tool: "highlight", icon: "highlight", label: "Highlighter", key: "H" },
    { tool: "number", icon: "number", label: "Numbered step", key: "N" },
    { tool: "pixelate", icon: "pixelate", label: "Pixelate: hides what is under it", key: "B" },
  ];
  const COLORS = VIVID;
  const LEVELS = [
    { level: 1, label: "Thin" },
    { level: 2, label: "Medium" },
    { level: 4, label: "Thick" },
  ];
  /** Smaller sizes and rates offered besides the recording's own. */
  const HEIGHTS = [2160, 1440, 1080, 720, 480];
  const FRAME_RATES = [60, 30, 24];
  const FADES = [0, 0.5, 1, 2];
  /** Text backgrounds. */
  const FILLS: { value: Shape["fill"]; label: string }[] = [
    { value: "none", label: "Outlined text" },
    { value: "soft", label: "On a tinted box" },
    { value: "solid", label: "On a solid box" },
  ];
  /** The arrow pointer, in a 32 px box (as drawn by `drawCursor`). */
  const ARROW = "M2 2L2 24L7.5 19L11.5 28L15 26.5L11 17.5L18.5 17.5Z";
  /** Pixel block of the pixelate tool, by thickness. */
  const BLOCKS: Record<number, number> = { 1: 8, 2: 14, 4: 24 };
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
  /** Sound level (1: as recorded, up to 2) and fades, in seconds. */
  let volume = $state(1);
  let fadeIn = $state(0);
  let fadeOut = $state(0);
  /** What is saved, and how. */
  let exportKind = $state<"mp4" | "gif">("mp4");
  let height = $state<number | null>(null);
  let fps = $state<number | null>(null);
  /** 0 (small) – 100 (as good as it gets); mapped to the encoder quality. */
  let qualityLevel = $state(80);
  let gifQuality = $state(90);
  let gifDither = $state(true);
  let gifLoop = $state(true);
  /** Export folder, and whether the recording is still a draft. */
  let folder = $state("");
  let unsaved = $state(false);
  /** Asking whether to close without saving. */
  let closing = $state(false);
  /** Something was saved (or copied) from this draft. */
  let savedOnce = $state(false);
  let sourceBytes = $state(0);
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
  let clickVoice = $state<ClickSound>("soft");
  let overlays = $state<Overlay[]>([]);
  let selectedOverlay = $state<number | null>(null);
  let tool = $state<Tool>("select");
  let color = $state("#e5484d");
  let level = $state(2);
  /** Filmstrip frames, evenly spread over the video; empty until made. */
  let frames = $state<(ImageBitmap | null)[]>([]);
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
  let playerSize = $state({ w: 0, h: 0 });
  let draft = $state<Shape | null>(null);
  let zoom = $state(1);
  let viewWidth = $state(800);
  let menu = $state<"speed" | "sound" | null>(null);
  /** A new format is being framed: Cancel goes back to the one before. */
  let formatPending = false;
  /** Playback errors in a row (the player reloads on the first ones). */
  let failures = 0;
  /** While drawing a curved arrow: the path followed, to bend it that way. */
  let curvePath: Point[] = [];
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
      let frame: ImageBitmap | null = null;
      for (let d = 0; !frame && d < frames.length; d++) frame = frames[index - d] ?? frames[index + d] ?? null;
      return { left: i * width, width, frame };
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
      gifQuality = c.gif.quality;
      gifDither = c.gif.dither;
      gifLoop = c.gif.repeat;
    });
    editorVideoSources(path)
      .then(async (s) => {
        cursorTrack = s.track;
        if (s.track) shapes = await loadShapes(s.track);
        cleanSource = s.clean;
        folder = s.folder;
        unsaved = s.draft;
        sourceBytes = s.bytes;
      })
      .catch(() => {});
    const unlisten = onJobProgress((p) => {
      const i = jobs.findIndex((j) => j.job === p.job);
      const old = jobs[i];
      if (old) jobs[i] = { ...p, kind: old.kind };
      if (old && p.output && p.output !== old.output) {
        savedOnce = true;
        notify(old.kind === "copy" ? "Copied to the clipboard" : `Saved · ${name(p.output)}`);
      }
      if (old && p.error && !old.error) notify(`Not saved: ${p.error}`);
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
    // Closing deletes a draft: ask first when nothing was saved from it.
    const stopClose = getCurrentWindow().onCloseRequested((e) => {
      if (!mustAsk) return;
      e.preventDefault();
      closing = true;
    });
    return () => {
      cancelAnimationFrame(frame);
      stopClose.then((f) => f());
      observer.disconnect();
      window.removeEventListener("pointerdown", closeMenu);
      unlisten.then((f) => f());
      audioContext?.close();
    };
  });

  /** Nothing was saved from this draft and nothing is being saved. */
  const mustAsk = $derived(unsaved && !savedOnce && !jobs.some((j) => !j.error));

  // "Kadra Editor · name · size · length · rate · weight" in the title bar.
  $effect(() => {
    if (!vw || !duration) return;
    const parts = [name(path), `${vw}×${vh}`, format(duration), `${sourceFps} fps`];
    if (sourceBytes) parts.push(size(sourceBytes));
    getCurrentWindow()
      .setTitle(`Kadra Editor · ${parts.join(" · ")}`)
      .catch(() => {});
  });

  /** Deletes the draft and closes the window. */
  async function closeWithoutSaving() {
    closing = false;
    try {
      await editorDiscard(path);
    } catch (e) {
      error = String(e);
    }
  }

  async function saveThenStay() {
    closing = false;
    await exportAs(exportKind);
  }

  /** The player stopped with an error (the system decoder can fail after
   * a while, e.g. while an export uses the GPU): reload it where it was.
   * Only repeated failures are reported. */
  function playbackFailed() {
    if (failures++ >= 3) {
      error = "This video cannot be played here, but it can still be exported.";
      return;
    }
    const at = time;
    const resume = playing;
    video.addEventListener(
      "loadeddata",
      () => {
        video.currentTime = at;
        if (resume) video.play().catch(() => {});
      },
      { once: true },
    );
    video.load();
  }

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

  /** Draws a filmstrip frame into its tile. */
  function showFrame(frame: ImageBitmap) {
    return (canvas: HTMLCanvasElement) => {
      canvas.width = frame.width;
      canvas.height = frame.height;
      canvas.getContext("2d")?.drawImage(frame, 0, 0);
    };
  }

  /** Frames for the timeline, from a second, hidden player: every 8th
   * first so the strip looks complete quickly, then the ones between. */
  async function filmstrip() {
    const probe = document.createElement("video");
    probe.src = convertFileSrc(path);
    probe.muted = true;
    probe.preload = "auto";
    await new Promise((resolve) => probe.addEventListener("loadeddata", resolve, { once: true }));
    const height = 2 * TRACK_H;
    const width = Math.max(1, Math.round((height * probe.videoWidth) / Math.max(1, probe.videoHeight)));
    const count = Math.min(120, Math.max(16, Math.ceil(duration * 3)));
    frames = new Array(count).fill(null);
    const order = [...Array(count).keys()].sort((a, b) => (a % 8 === 0 ? 0 : 1) - (b % 8 === 0 ? 0 : 1) || a - b);
    for (const i of order) {
      probe.currentTime = ((i + 0.5) * duration) / count;
      await new Promise((resolve) => probe.addEventListener("seeked", resolve, { once: true }));
      // Kept as bitmaps: the video comes from another origin (the asset
      // protocol), so reading pixels back (toDataURL) is not allowed.
      frames[i] = await createImageBitmap(probe, { resizeWidth: width, resizeHeight: height, resizeQuality: "medium" });
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

    const shown = overlays.filter((o) => t >= o.start && t < o.end && o.id !== editing?.id).map((o) => o.shape);
    const all = draft ? [...shown, draft] : shown;
    // Pixelated areas hide the video, under everything else.
    for (const s of all) if (s.kind === "pixelate") pixelateArea(ctx, s);
    renderAnnotations(
      ctx,
      all.filter((s) => s.kind !== "pixelate"),
    );
    if (draft?.kind === "pixelate") dashed(ctx, normalize(draft), px);

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

  const scratch = document.createElement("canvas");

  /** Shows `s` (in source pixels) as blocks of `s.size` pixels. */
  function pixelateArea(ctx: CanvasRenderingContext2D, s: Shape) {
    const r = normalize(s);
    const x = Math.max(0, r.x);
    const y = Math.max(0, r.y);
    const w = Math.min(vw, r.x + r.w) - x;
    const h = Math.min(vh, r.y + r.h) - y;
    if (w < 2 || h < 2 || video.readyState < 2) return;
    const block = Math.max(2, s.size);
    scratch.width = Math.max(1, Math.ceil(w / block));
    scratch.height = Math.max(1, Math.ceil(h / block));
    const sctx = scratch.getContext("2d");
    if (!sctx) return;
    sctx.imageSmoothingQuality = "high";
    sctx.drawImage(video, x, y, w, h, 0, 0, scratch.width, scratch.height);
    ctx.save();
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(scratch, 0, 0, scratch.width, scratch.height, x, y, w, h);
    ctx.restore();
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
  /** Time in the result of source time `t`. */
  function outputTime(t: number) {
    let out = 0;
    for (const p of kept) {
      if (t < p.start) break;
      out += (Math.min(t, p.end) - p.start) / p.speed;
    }
    return out;
  }

  /** Sound level at `t` seconds of the result. */
  function soundLevel(t: number) {
    let g = volume;
    if (fadeIn > 0) g *= Math.max(0, Math.min(1, t / fadeIn));
    if (fadeOut > 0) g *= Math.max(0, Math.min(1, (output - t) / fadeOut));
    return g;
  }

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
    // Volume and fades, as in the export (the preview cannot go above 100%).
    video.muted = !audio;
    const level = soundLevel(outputTime(next));
    if (Math.abs(video.volume - Math.min(1, level)) > 0.01) video.volume = Math.min(1, level);
    // Clicks passed since the last frame (not across a jump).
    if (clickSound && cursorTrack && time > lastClickCheck && time - lastClickCheck < 0.5) {
      for (const c of cursorTrack.clicks) {
        if (c.t > lastClickCheck && c.t <= time) {
          audioContext ??= new AudioContext();
          playClick(audioContext, clickVoice);
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
    const at = untrack(() => time);
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
    formatPending = false;
    apply(state);
  }

  /** Puts back a state saved by `snapshot`. */
  function apply(state: string) {
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

  /** Removes the cut at the start of part `i`. */
  function joinAt(i: number) {
    commit(() => (segments = join(segments, i)));
    selectedSegment = null;
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
    const pending = formatPending;
    formatPending = false;
    cropping = false;
    cropDraft = null;
    if (!r) return;
    const even = (v: number) => Math.round(v) & ~1;
    const next = { x: even(r.x), y: even(r.y), w: even(r.w), h: even(r.h) };
    const entire = next.x === 0 && next.y === 0 && next.w >= (vw & ~1) && next.h >= (vh & ~1);
    const value = entire && !lockedRatio ? null : next;
    if (JSON.stringify(value) === JSON.stringify(crop)) return;
    // Framing a new format is one step with choosing it.
    if (pending) crop = value;
    else commit(() => (crop = value));
  }

  /** Leaves the frame as it was; a format just chosen is undone. */
  function cancelCrop() {
    cropping = false;
    cropDraft = null;
    if (!formatPending) return;
    formatPending = false;
    const before = past.pop();
    if (before !== undefined) apply(before);
  }

  /** A format change, framed next: Cancel goes back to before the first. */
  function changeFormat(change: () => void) {
    if (formatPending) change();
    else commit(change);
  }

  function chooseRatio(r: [number, number] | null) {
    if (JSON.stringify(r) === JSON.stringify(ratio)) return;
    changeFormat(() => {
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
    if (r && fit === "fill") {
      startCrop();
      formatPending = true;
    } else {
      formatPending = false;
    }
  }

  function chooseFit(mode: "fill" | "fit") {
    if (mode === fit || !ratio) return;
    changeFormat(() => {
      fit = mode;
      crop = mode === "fill" && ratio ? ratioCrop(full, ratio) : null;
    });
    if (mode === "fill") {
      startCrop();
      formatPending = true;
    } else {
      formatPending = false;
      cropping = false;
      cropDraft = null;
    }
  }

  // ── Overlays ───────────────────────────────────────────────────────────

  function make(kind: ShapeKind, p: Point, extra: Partial<Shape> = {}): Shape {
    return {
      id: nextId++,
      kind,
      color,
      size:
        kind === "text" ? (level + 1) * unit : kind === "pixelate" ? (BLOCKS[level] ?? 14) * unit : (level + 2) * unit,
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
    curvePath = [p];
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
      if (draft.kind === "curve") {
        curvePath.push(p);
        draft.bend = drawnBend(draft, curvePath);
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
    const font = existing ? textFont(existing.shape, size) : `600 ${size}px "Segoe UI Variable Text", "Segoe UI", sans-serif`;
    return `left:${at.x}px;top:${at.y}px;font:${font};color:${existing?.shape.color ?? color}`;
  });

  function removeOverlay() {
    const id = selectedOverlay;
    if (id === null) return;
    selectedOverlay = null;
    commit(() => (overlays = overlays.filter((o) => o.id !== id)));
  }

  /** Changes the style of the selected text. */
  function styleText(patch: Partial<Pick<Shape, "font" | "bold" | "italic" | "fill">>) {
    const o = selectedItem;
    if (!o || o.shape.kind !== "text") return;
    commit(() => Object.assign(o.shape, patch));
  }

  function duplicateOverlay() {
    const o = selectedItem;
    if (!o) return;
    const shape = $state.snapshot(o.shape) as Shape;
    const shift = 24 * unit;
    shape.id = nextId++;
    shape.x1 += shift;
    shape.x2 += shift;
    shape.y1 += shift;
    shape.y2 += shift;
    if (shape.points) shape.points = shape.points.map((q) => ({ x: q.x + shift, y: q.y + shift }));
    const copy = { id: shape.id, shape, start: o.start, end: o.end };
    commit(() => overlays.push(copy));
    selectedOverlay = copy.id;
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
    const size =
      o.shape.kind === "text" ? (l + 1) * unit : o.shape.kind === "pixelate" ? (BLOCKS[l] ?? 14) * unit : (l + 2) * unit;
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
      if (o.shape.kind === "pixelate") continue;
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
      clicks:
        clickRing || clickSound
          ? { ring: clickRing, color: hexRgb(clickColor), sound: clickSound, voice: clickVoice }
          : null,
      canvas: ratio && fit === "fit" ? { ratio: [ratio[0], ratio[1]], colors: bg.map(hexRgb), padding: margin } : null,
      pixelate: overlays
        .filter((o) => o.shape.kind === "pixelate")
        .map((o) => {
          const r = normalize(o.shape);
          return {
            x: Math.round(r.x),
            y: Math.round(r.y),
            w: Math.round(r.w),
            h: Math.round(r.h),
            block: Math.max(2, Math.round(o.shape.size)),
            start: o.start,
            end: o.end,
          };
        }),
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
      !halo &&
      audio &&
      volume === 1 &&
      !fadeIn &&
      !fadeOut,
  );

  /** Encoder quality (0 best – 51) for the quality level shown. */
  const crf = $derived(Math.round(34 - (qualityLevel / 100) * 20));

  async function exportAs(kind: "mp4" | "gif", copy = false, output: string | null = null) {
    if (cropping) applyCrop();
    if (!config || !kept.length) return;
    try {
      const job = await editorExport(
        path,
        kind === "mp4"
          ? { kind, edit: edit(), audio, quality: crf, fps, height, volume, fade_in: fadeIn, fade_out: fadeOut }
          : {
              kind,
              gif: {
                ...$state.snapshot(config.gif),
                fps: gifFps,
                max_width: gifWidth,
                quality: gifQuality,
                dither: gifDither,
                repeat: gifLoop,
              },
              edit: edit(),
            },
        copy,
        output,
      );
      jobs.unshift({ job, kind: copy ? "copy" : kind, input: path, done: 0, output: null, bytes: null, error: null });
    } catch (e) {
      error = String(e);
    }
  }

  async function copyVideo() {
    if (untouched) {
      await editorCopyFile(path);
      savedOnce = true;
      notify("Copied to the clipboard");
    } else {
      notify("Preparing the edited video… it will be copied when ready");
      await exportAs("mp4", true);
    }
  }

  /** Save with a name and place picked in a dialog. */
  async function saveAs() {
    const output = await editorSaveAs(path, exportKind).catch(() => null);
    if (output) await exportAs(exportKind, false, output);
  }

  async function chooseFolder() {
    const chosen = await editorChooseFolder(path).catch(() => null);
    if (chosen) folder = chosen;
  }

  /** Size of the exported frame, after crop, format and height. */
  /** Size of the exported frame at full resolution. */
  const exportBase = $derived.by(() => {
    if (!vw) return { w: 0, h: 0 };
    const fitted = ratio && fit === "fit" ? fitLayout(crop ?? full, ratio, margin).out : null;
    return { w: Math.round(fitted?.w ?? crop?.w ?? vw) & ~1, h: Math.round(fitted?.h ?? crop?.h ?? vh) & ~1 };
  });
  const exportSize = $derived.by(() => {
    if (!vw) return { w: 0, h: 0 };
    const fitted = ratio && fit === "fit" ? fitLayout(crop ?? full, ratio, margin).out : null;
    let w = fitted?.w ?? crop?.w ?? vw;
    let h = fitted?.h ?? crop?.h ?? vh;
    if (exportKind === "mp4" && height && height < h) {
      w = (w * height) / h;
      h = height;
    }
    if (exportKind === "gif" && gifWidth && gifWidth < w) {
      h = (h * gifWidth) / w;
      w = gifWidth;
    }
    return { w: Math.round(w) & ~1, h: Math.round(h) & ~1 };
  });
  /** Frame rate of the recording, from its cursor track (else 60). */
  const sourceFps = $derived.by(() => {
    const s = cursorTrack?.samples;
    const step = s && s.length > 2 ? (s[2]!.t - s[0]!.t) / 2 : 0;
    return step > 0 ? Math.round(1 / step) : 60;
  });
  /** What the encoder aims for (same rule as the exporter), in bytes. */
  const estimate = $derived.by(() => {
    if (exportKind !== "mp4" || !exportSize.w) return null;
    const rate = Math.min(fps ?? sourceFps, sourceFps);
    const bpp = 0.08 * 2 ** ((23 - crf) / 6);
    const bits = Math.min(2e8, Math.max(1e6, exportSize.w * exportSize.h * rate * bpp));
    return (bits / 8 + (audio ? 24_000 : 0)) * output;
  });
  const qualityName = $derived(
    qualityLevel >= 90 ? "Near lossless" : qualityLevel >= 70 ? "High" : qualityLevel >= 45 ? "Medium" : "Small file",
  );

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function notify(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 3000);
  }

  function listen(voice: ClickSound) {
    clickVoice = voice;
    audioContext ??= new AudioContext();
    playClick(audioContext, voice);
  }

  /** The export being written (shown in the Save button), if any. */
  const running = $derived(jobs.find((j) => !j.output && !j.error && j.kind !== "copy") ?? null);

  /** Where the toolbar of the selected item sits, above it (or below when
   * there is no room), in player pixels. */
  const itemBar = $derived.by(() => {
    void time;
    const o = selectedItem;
    if (!o || cropping || editing || drag || time < o.start || time >= o.end || !stage) return null;
    const ctx = stage.getContext("2d");
    if (!ctx) return null;
    const b = bounds(ctx, o.shape);
    const top = toPlayer({ x: b.x, y: b.y });
    const bottom = toPlayer({ x: b.x + b.w, y: b.y + b.h });
    const x = Math.max(8, Math.min(playerSize.w - 8, (top.x + bottom.x) / 2));
    return top.y > 60 ? { x, y: top.y - 12, below: false } : { x, y: bottom.y + 12, below: true };
  });

  /** Options of the resolution and frame rate menus: the recording's own
   * value first, then smaller ones. */
  const heightOptions = $derived([
    { value: null, label: `${exportBase.h}p` },
    ...HEIGHTS.filter((h) => h < exportBase.h).map((h) => ({ value: h, label: `${h}p` })),
  ]);
  const fpsOptions = $derived([
    { value: null, label: `${sourceFps} fps` },
    ...FRAME_RATES.filter((r) => r < sourceFps).map((r) => ({ value: r, label: `${r} fps` })),
  ]);

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
    (["text", "arrow", "curve", "line", "rect", "ellipse", "pen", "highlight", "number", "pixelate"] as const).find((k) => k === kind) ?? "rect";
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
</script>

<svelte:window onkeydown={keyDown} />

<div class="editor">
  {#if closing}
    <div class="backdrop">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="close-title">
        <h2 id="close-title">Close without saving?</h2>
        <p>This recording was never saved. Closing the editor deletes it.</p>
        <div class="dialog-actions">
          <button type="button" class="pill big" onclick={saveThenStay}><Icon name="save" /> Save {exportKind.toUpperCase()}</button>
          <button type="button" class="ghost danger-fill" onclick={closeWithoutSaving}>Delete and close</button>
          <button type="button" class="ghost" onclick={() => (closing = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}
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
        onerror={playbackFailed}
        onseeked={() => (failures = 0)}
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
      {#if itemBar && selectedItem}
        {@const s = selectedItem.shape}
        <div
          class="item-bar"
          class:below={itemBar.below}
          style="left:{itemBar.x}px;top:{itemBar.y}px"
          onpointerdown={(e) => e.stopPropagation()}
          role="toolbar"
          aria-label="Selected item"
          tabindex="-1"
        >
          {#if s.kind === "text"}
            {#each TEXT_FONTS as f (f.value)}
              <button type="button" class="ib" class:active={(s.font ?? "sans") === f.value} style="font-family:{f.family}" title={f.label} onclick={() => styleText({ font: f.value })}
                >Aa</button
              >
            {/each}
            <span class="ib-sep"></span>
            <button type="button" class="ib" class:active={s.bold !== false} style="font-weight:700" title="Bold" onclick={() => styleText({ bold: s.bold === false })}>B</button>
            <button type="button" class="ib" class:active={!!s.italic} style="font-style:italic;font-family:Georgia,serif" title="Italic" onclick={() => styleText({ italic: !s.italic })}
              >I</button
            >
            <span class="ib-sep"></span>
            {#each FILLS as fl (fl.value)}
              <button type="button" class="ib" class:active={s.fill === fl.value} title={fl.label} onclick={() => styleText({ fill: fl.value })}
                ><span class="fill-demo {fl.value}" style="--c:{s.color}">A</span></button
              >
            {/each}
            <span class="ib-sep"></span>
          {/if}
          <button type="button" class="ib" title="Duplicate" aria-label="Duplicate" onclick={duplicateOverlay}><Icon name="duplicate" /></button>
          <button type="button" class="ib danger" title="Delete (Del)" aria-label="Delete" onclick={removeOverlay}><Icon name="trash" /></button>
        </div>
      {/if}
    </div>

    <footer class="dock">
      <div class="controls">
        <div class="side left">
          <button type="button" class="act" onclick={splitHere} disabled={!duration} title="Split at the playhead (S)"
            ><Icon name="scissors" /> <span class="lbl">Split</span> <Kbd keys="S" /></button
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
          <div class="anchor">
            <button
              type="button"
              class="act opens-menu"
              class:on={!audio || volume !== 1 || fadeIn > 0 || fadeOut > 0}
              disabled={!duration}
              aria-expanded={menu === "sound"}
              title="Sound"
              onclick={() => (menu = menu === "sound" ? null : "sound")}
              ><Icon name={audio ? "volume" : "mute"} /> {audio ? `${Math.round(volume * 100)}%` : "Muted"} <Icon name="chevron" /></button
            >
            {#if menu === "sound"}
              <div class="popover sound" role="dialog" aria-label="Sound">
                <label class="switch-row"><span>Sound</span><input type="checkbox" class="switch" bind:checked={audio} /></label>
                <label class="field" class:off={!audio}
                  >Volume <input type="range" min="0" max="2" step="0.05" bind:value={volume} disabled={!audio} />
                  <span class="value">{Math.round(volume * 100)}%</span></label
                >
                <div class="field" class:off={!audio}>
                  <span>Fade in</span>
                  <div class="segmented tiny" role="radiogroup" aria-label="Fade in">
                    {#each FADES as f (f)}
                      <button type="button" role="radio" aria-checked={fadeIn === f} class:active={fadeIn === f} disabled={!audio} onclick={() => (fadeIn = f)}
                        >{f ? `${f}s` : "Off"}</button
                      >
                    {/each}
                  </div>
                </div>
                <div class="field" class:off={!audio}>
                  <span>Fade out</span>
                  <div class="segmented tiny" role="radiogroup" aria-label="Fade out">
                    {#each FADES as f (f)}
                      <button type="button" role="radio" aria-checked={fadeOut === f} class:active={fadeOut === f} disabled={!audio} onclick={() => (fadeOut = f)}
                        >{f ? `${f}s` : "Off"}</button
                      >
                    {/each}
                  </div>
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
                {#if tile.frame}<canvas {@attach showFrame(tile.frame)} style="left:{tile.left}px;width:{tile.width}px"></canvas>{/if}
              {/each}
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
            <!-- Cuts: a clear line between parts; hovering shows how to join them back. -->
            {#each segments as sg, i (sg.start)}
              {#if i > 0}
                <div class="cut-mark" style="left:{percent(sg.start)}%">
                  <button
                    type="button"
                    class="join"
                    title="Remove this split (join the parts)"
                    aria-label="Remove this split"
                    onpointerdown={(e) => e.stopPropagation()}
                    onclick={() => joinAt(i)}><Icon name="join" /></button
                  >
                </div>
              {/if}
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
                      : o.shape.kind === "pixelate"
                        ? "Pixelate"
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
      <div class="file" title={path}>{name(path)}</div>
      <div class="sub">{format(output)}{outputSize ? ` · ${outputSize}` : ""}{kept.length > 1 ? ` · ${kept.length} parts` : ""}</div>
    </header>

    <div class="panel-scroll">
      <section>
        <h3>Format</h3>
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
              >Fill</button
            >
            <button type="button" class:active={fit === "fit"} role="radio" aria-checked={fit === "fit"} onclick={() => chooseFit("fit")}
              >Fit on background</button
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
          {/if}
        {/if}
        <div class="pair">
          <button type="button" class:active={cropping} onclick={() => (cropping ? applyCrop() : startCrop())}
            ><Icon name="crop" /> {ratio && fit === "fill" ? "Reframe" : "Crop"}</button
          >
          <button type="button" disabled={!crop || !!(ratio && fit === "fill")} onclick={() => commit(() => (crop = null))}>Reset</button>
        </div>
      </section>

      <section>
        <h3>Cursor and clicks</h3>
        {#if !cursorTrack}
          <p class="note">Needs a Kadra recording made with “editable cursor” on (Settings › Recording).</p>
        {:else}
          {#if cursorEditable}
            <div class="looks" role="radiogroup" aria-label="Pointer">
              <button type="button" role="radio" class="look" class:active={!cursorVisible} aria-checked={!cursorVisible} onclick={() => (cursorVisible = false)}>
                <svg viewBox="0 0 32 32" aria-hidden="true"><path d={ARROW} class="ghost-arrow" /><path d="M5 27L27 5" class="slash" /></svg>
                Hidden
              </button>
              {#each CURSOR_STYLES as cs (cs.value)}
                {@const active = cursorVisible && cursorStyle === cs.value}
                <button
                  type="button"
                  role="radio"
                  class="look"
                  class:active
                  aria-checked={active}
                  onclick={() => {
                    cursorVisible = true;
                    cursorStyle = cs.value;
                  }}
                >
                  {#if cs.value === "original" && cursorTrack.shapes[0]}
                    <img src="data:image/png;base64,{cursorTrack.shapes[0].png}" alt="" />
                  {:else if cs.value === "dot"}
                    <svg viewBox="0 0 32 32" aria-hidden="true"><circle cx="16" cy="16" r="9" fill={cursorColor} stroke="#fff" stroke-width="2" /></svg>
                  {:else}
                    <svg viewBox="0 0 32 32" aria-hidden="true"
                      ><path d={ARROW} fill={cs.value === "dark" ? "#111" : "#fff"} stroke={cs.value === "dark" ? "#fff" : "#111"} stroke-width="1.6" stroke-linejoin="round" /></svg
                    >
                  {/if}
                  {cs.label}
                </button>
              {/each}
            </div>
            {#if cursorVisible}
              <label class="field"
                >Size <input type="range" min="0.5" max="4" step="0.25" bind:value={cursorScale} />
                <span class="value">{cursorScale}×</span></label
              >
              {#if cursorStyle === "dot"}
                <div class="field">
                  <span>Colour</span>
                  <div class="mini-swatches">
                    {#each VIVID.slice(0, 6) as c (c)}
                      <button type="button" class="swatch" class:active={cursorColor === c} style="--swatch:{c}" aria-label="Colour {c}" onclick={() => (cursorColor = c)}></button>
                    {/each}
                  </div>
                </div>
              {/if}
            {/if}
          {:else}
            <p class="note">The copy without cursor of this recording is gone: only clicks can be shown.</p>
          {/if}

          <div class="effects">
            {#if cursorEditable}
              <div class="effect" class:active={halo}>
                <button type="button" aria-pressed={halo} onclick={() => (halo = !halo)}>
                  <svg viewBox="0 0 40 40" aria-hidden="true"
                    ><circle cx="16" cy="16" r="13" fill={haloColor} opacity="0.45" /><path d={ARROW} transform="translate(9 9) scale(0.7)" fill="#fff" stroke="#111" stroke-width="2" /></svg
                  >
                  <span>Halo</span>
                </button>
                <input type="color" bind:value={haloColor} class="dot-color" title="Halo colour" />
              </div>
            {/if}
            <div class="effect" class:active={clickRing}>
              <button type="button" aria-pressed={clickRing} onclick={() => (clickRing = !clickRing)}>
                <svg viewBox="0 0 40 40" aria-hidden="true"
                  ><circle cx="16" cy="16" r="12" fill="none" stroke={clickColor} stroke-width="3" /><circle cx="16" cy="16" r="6" fill="none" stroke={clickColor} stroke-width="2" opacity="0.5" /><path d={ARROW} transform="translate(13 13) scale(0.7)" fill="#fff" stroke="#111" stroke-width="2" /></svg
                >
                <span>Click ring</span>
              </button>
              <input type="color" bind:value={clickColor} class="dot-color" title="Ring colour" />
            </div>
          </div>

          <div class="field stacked">
            <span>Click sound</span>
            <div class="segmented wrap" role="radiogroup" aria-label="Click sound">
              <button type="button" role="radio" aria-checked={!clickSound} class:active={!clickSound} onclick={() => (clickSound = false)}>Off</button>
              {#each CLICK_SOUNDS as cs (cs.value)}
                {@const active = clickSound && clickVoice === cs.value}
                <button
                  type="button"
                  role="radio"
                  aria-checked={active}
                  class:active
                  title="Plays it once"
                  onclick={() => {
                    clickSound = true;
                    listen(cs.value);
                  }}>{cs.label}</button
                >
              {/each}
            </div>
          </div>
        {/if}
      </section>
    </div>

    <div class="export-card">
      <div class="segmented" role="radiogroup" aria-label="Format">
        <button type="button" role="radio" aria-checked={exportKind === "mp4"} class:active={exportKind === "mp4"} onclick={() => (exportKind = "mp4")}
          >MP4</button
        >
        <button type="button" role="radio" aria-checked={exportKind === "gif"} class:active={exportKind === "gif"} onclick={() => (exportKind = "gif")}
          >GIF</button
        >
      </div>
      {#if exportKind === "mp4"}
        <div class="settings">
          <label class="field">Resolution
            <select bind:value={height}>{#each heightOptions as o (o.label)}<option value={o.value}>{o.label}</option>{/each}</select>
          </label>
          <label class="field">Frame rate
            <select bind:value={fps}>{#each fpsOptions as o (o.label)}<option value={o.value}>{o.label}</option>{/each}</select>
          </label>
          <label class="field">Quality
            <input type="range" min="0" max="100" step="5" bind:value={qualityLevel} />
            <span class="value wide">{qualityName}</span>
          </label>
        </div>
      {:else}
        <div class="settings">
          <label class="field">Width
            <select bind:value={gifWidth}
              >{#each GIF_WIDTHS.filter((w) => w === 0 || w < exportBase.w) as w (w)}<option value={w}>{w === 0 ? `${exportBase.w} px` : `${w} px`}</option>{/each}</select
            >
          </label>
          <label class="field">Frame rate
            <select bind:value={gifFps}>{#each GIF_FPS_PRESETS as fv (fv)}<option value={fv}>{fv} fps</option>{/each}</select>
          </label>
          <label class="field">Colours
            <input type="range" min="10" max="100" step="5" bind:value={gifQuality} />
            <span class="value wide">{gifQuality}%</span>
          </label>
          <div class="pair checks">
            <label class="check"><input type="checkbox" bind:checked={gifDither} /> Dithering</label>
            <label class="check"><input type="checkbox" bind:checked={gifLoop} /> Loop</label>
          </div>
        </div>
      {/if}
      <button type="button" class="destination" onclick={chooseFolder} title="Change the folder exports are saved in">
        <Icon name="folder" /><span>{folder || "…"}</span>
      </button>
      <div class="meta">
        <span>{exportSize.w ? `${exportSize.w}×${exportSize.h}` : ""}</span>
        <span>{estimate ? `≈ ${size(estimate)}` : ""}</span>
      </div>
      <button
        type="button"
        class="save"
        style={running ? `--done:${Math.round(running.done * 100)}%` : ""}
        class:busy={!!running}
        onclick={() => exportAs(exportKind)}
        disabled={!duration || !config}
      >
        {#if running}
          Saving… {Math.round(running.done * 100)}%
        {:else}
          <Icon name="save" /> Save {exportKind.toUpperCase()}
        {/if}
      </button>
      <div class="pair secondary">
        <button type="button" class="light" onclick={saveAs} disabled={!duration || !config} title="Pick a name and a place"
          ><Icon name="saveCopy" /> Save as…</button
        >
        <button type="button" class="light" onclick={copyVideo} disabled={!duration} title="Copy to the clipboard (Ctrl+C)"
          ><Icon name="copy" /> Copy</button
        >
      </div>
    </div>
  </aside>
</div>

<style>
  /* Calm and airy, in the window's theme (docs/UI_DESIGN.md). */
  .editor {
    --accent-soft: color-mix(in srgb, var(--accent) 22%, transparent);
    --trim: #f5c542;
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(270px, 25vw, 320px);
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
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  button.active {
    background: var(--accent) !important;
    color: var(--on-accent) !important;
  }
  .sep {
    width: 1px;
    height: 22px;
    margin: 0 6px;
    background: var(--border);
  }
  .muted {
    color: var(--text-faint);
  }

  /* Toolbox */
  @container (max-width: 1060px) {
    .toolbox .tool {
      width: 32px;
    }
    .toolbox .colors {
      gap: 5px;
    }
    .toolbox .colors .swatch,
    .toolbox .custom {
      width: 19px;
      height: 19px;
    }
  }
  .toolbox {
    display: flex;
    flex-wrap: wrap;
    container-type: inline-size;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 10px 14px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .cluster {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .colors {
    gap: 7px;
    padding: 0 4px;
  }
  .tool {
    position: relative;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    background: transparent;
  }
  .tool .key {
    position: absolute;
    right: 3px;
    bottom: 1px;
    font-size: 8px;
    font-weight: 700;
    font-family: var(--font-mono);
    opacity: 0.45;
  }
  .swatch {
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 50%;
    background: var(--swatch) !important;
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.45);
  }
  .swatch.active {
    background: var(--swatch) !important;
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .custom {
    position: relative;
    width: 22px;
    height: 22px;
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

  /* Close confirmation */
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
    font-size: 21px;
    font-weight: 650;
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
  .pill.big {
    min-height: 40px;
    padding: 8px 20px;
  }
  .danger-fill {
    color: var(--danger) !important;
  }
  .danger-fill:hover:not(:disabled) {
    background: var(--danger) !important;
    color: #fff !important;
  }

  /* Player: transparent around the picture, shown as a checkerboard. */
  .player {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 94%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 20px
      20px;
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
  .crop-bar {
    position: absolute;
    left: 50%;
    bottom: 16px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 6px 6px 16px;
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow);
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
    top: 16px;
    transform: translateX(-50%);
    max-width: calc(100% - 32px);
    padding: 8px 16px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow-soft);
    font-size: 13px;
    font-weight: 550;
    pointer-events: none;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .toast {
    top: auto;
    bottom: 16px;
  }
  .hint {
    color: var(--text-muted);
    font-size: 12px;
  }
  .error {
    color: var(--danger);
  }

  /* Toolbar of the selected item, over the picture. */
  .item-bar {
    position: absolute;
    z-index: 6;
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 4px;
    border-radius: 12px;
    background: var(--surface);
    box-shadow: var(--shadow);
    transform: translate(-50%, -100%);
    cursor: default;
  }
  .item-bar.below {
    transform: translate(-50%, 0);
  }
  .ib {
    display: grid;
    place-items: center;
    min-width: 32px;
    height: 32px;
    padding: 0 6px;
    background: transparent;
    font-size: 14px;
  }
  .ib.danger:hover:not(:disabled) {
    color: var(--danger);
  }
  .ib-sep {
    width: 1px;
    height: 20px;
    margin: 0 3px;
    background: var(--border);
  }
  .fill-demo {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    font-weight: 800;
    font-size: 13px;
  }
  .fill-demo.none {
    color: var(--c);
    -webkit-text-stroke: 0.6px var(--text);
  }
  .fill-demo.soft {
    background: color-mix(in srgb, var(--c) 70%, transparent);
    color: #fff;
  }
  .fill-demo.solid {
    background: var(--c);
    color: #fff;
  }

  .pill {
    justify-content: center;
    padding: 6px 16px;
    background: var(--accent) !important;
    color: var(--on-accent) !important;
    font-weight: 650;
  }
  .pill:hover:not(:disabled) {
    background: var(--accent-hover) !important;
  }
  .ghost {
    justify-content: center;
    padding: 6px 14px;
    background: transparent;
  }

  /* Dock: transport and timeline */
  .dock {
    container-type: inline-size;
    padding: 12px 16px 14px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .controls {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
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
    white-space: nowrap;
  }
  .act.danger:hover:not(:disabled) {
    color: var(--danger);
  }
  .act.on {
    background: color-mix(in srgb, var(--trim) 22%, var(--control));
  }
  .center {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .round {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border-radius: 50%;
    background: transparent;
  }
  .round.small {
    width: 28px;
    height: 28px;
  }
  .round.play {
    width: 46px;
    height: 46px;
    background: var(--text);
    color: var(--surface);
  }
  .round.play:hover {
    background: var(--text);
    transform: scale(1.04);
  }
  .time {
    min-width: 140px;
    font: 500 13px var(--font-mono);
  }
  .hint-keys {
    font-size: 11px;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
  }
  /* Narrow windows: icons only, no key hints. */
  @container (max-width: 1180px) {
    .act .lbl,
    .act :global(.kbd),
    .hint-keys {
      display: none;
    }
    .act {
      padding: 0 9px;
    }
  }
  .zoom {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .zoom input {
    width: 100px;
    background: transparent;
  }
  .anchor {
    position: relative;
  }
  .popover {
    position: absolute;
    z-index: 20;
    bottom: calc(100% + 10px);
    left: 0;
    padding: 10px;
    border-radius: var(--radius-lg);
    background: var(--raised);
    box-shadow: var(--shadow);
  }
  .speeds {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    width: 230px;
  }
  .speeds > button {
    justify-content: center;
    font-family: var(--font-mono);
    background: transparent;
  }
  .speeds .all {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-muted);
  }
  .chip {
    padding: 2px 10px;
  }
  .sound {
    display: grid;
    gap: 12px;
    width: 280px;
    padding: 16px;
  }
  .switch-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-weight: 600;
  }
  .off {
    opacity: 0.45;
  }

  /* Timeline */
  .scroller {
    overflow-x: auto;
    overflow-y: hidden;
    padding: 0 14px;
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
    height: 20px;
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
    color: var(--text-faint);
    white-space: nowrap;
    pointer-events: none;
  }
  .track {
    position: relative;
    border-radius: var(--radius);
    background: var(--control);
    cursor: pointer;
  }
  .filmstrip {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: var(--radius);
    pointer-events: none;
  }
  .filmstrip canvas {
    position: absolute;
    top: 0;
    height: 100%;
    object-fit: cover;
  }
  .segment {
    position: absolute;
    top: 0;
    bottom: 0;
  }
  .segment.removed {
    background: repeating-linear-gradient(135deg, rgb(10 10 12 / 0.82) 0 6px, rgb(10 10 12 / 0.62) 6px 12px);
  }
  .segment.selected {
    box-shadow: inset 0 0 0 2px var(--accent);
    background-color: var(--accent-soft);
    border-radius: var(--radius);
  }
  .badge {
    position: absolute;
    left: 8px;
    bottom: 6px;
    padding: 0 7px;
    border-radius: 999px;
    background: var(--trim);
    color: #1a1400;
    font: 700 11px/18px var(--font-mono);
    pointer-events: none;
  }
  .part-tools {
    position: absolute;
    top: 6px;
    right: 8px;
    display: none;
    gap: 4px;
    z-index: 3;
  }
  .segment:hover .part-tools,
  .segment.selected .part-tools {
    display: flex;
  }
  .part-tools button {
    height: 24px;
    min-width: 24px;
    padding: 0 7px;
    justify-content: center;
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-soft);
    font: 600 11px var(--font-mono);
  }
  .part-tools button:hover {
    background: var(--text);
    color: var(--surface);
  }
  .part-tools :global(svg) {
    width: 14px;
    height: 14px;
  }
  /* A cut: a bright gap between two parts, with a join button on hover. */
  .cut-mark {
    position: absolute;
    top: -6px;
    bottom: -6px;
    width: 18px;
    margin-left: -9px;
    z-index: 4;
    display: flex;
    justify-content: center;
    cursor: default;
  }
  .cut-mark::before {
    content: "";
    position: absolute;
    left: 7px;
    top: 6px;
    bottom: 6px;
    width: 4px;
    border-radius: 2px;
    background: var(--surface);
    box-shadow: 0 0 0 1px rgb(0 0 0 / 0.5);
  }
  .cut-mark::after {
    content: "";
    position: absolute;
    left: 4px;
    top: 0;
    border: 5px solid transparent;
    border-top-color: var(--text);
  }
  .join {
    position: relative;
    z-index: 1;
    align-self: center;
    width: 26px;
    height: 26px;
    padding: 0;
    justify-content: center;
    border-radius: 50%;
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-soft);
    opacity: 0;
    transform: scale(0.8);
    transition:
      opacity 0.12s,
      transform 0.12s;
  }
  .cut-mark:hover .join {
    opacity: 1;
    transform: none;
  }
  .join:hover {
    background: var(--accent) !important;
    color: var(--on-accent);
  }
  .join :global(svg) {
    width: 16px;
    height: 16px;
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
    z-index: 5;
  }
  .handle.start {
    border-radius: 6px 0 0 6px;
    transform: translateX(-12px);
  }
  .handle.end {
    border-radius: 0 6px 6px 0;
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
    margin-top: 8px;
    transition: height 0.15s;
  }
  .bar {
    position: absolute;
    height: 22px;
    display: flex;
    align-items: center;
    border-radius: 6px;
    background: color-mix(in srgb, var(--bar) 28%, var(--raised));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--bar) 60%, transparent);
    cursor: grab;
    overflow: hidden;
  }
  .bar.selected {
    outline: 2px solid var(--focus);
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
    font-weight: 600;
    color: var(--text);
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
    background: var(--focus);
    pointer-events: none;
    z-index: 6;
  }
  .knob {
    position: absolute;
    top: 0;
    left: -5px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: var(--focus);
  }

  /* Right panel */
  .side-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--surface);
    border-left: 1px solid var(--border);
  }
  .panel-head {
    padding: 20px 22px 6px;
  }
  .file {
    font-size: 15px;
    font-weight: 650;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .panel-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 22px 16px;
  }
  section {
    display: grid;
    gap: 12px;
    padding: 16px 0 20px;
  }
  section + section {
    border-top: 1px solid var(--border);
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 650;
    color: var(--text-muted);
  }
  .ratios {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .ratio {
    flex-direction: column;
    justify-content: center;
    gap: 6px;
    height: 62px;
    padding: 6px 4px;
    font-size: 12px;
  }
  .ratio .shape {
    display: block;
    border: 1.5px solid currentColor;
    border-radius: 3px;
    opacity: 0.85;
  }
  .segmented {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: 10px;
    background: var(--control);
  }
  .segmented button {
    flex: 1;
    justify-content: center;
    padding: 6px 8px;
    border-radius: 7px;
    background: transparent;
    font-size: 13px;
    white-space: nowrap;
  }
  .segmented.wrap {
    flex-wrap: wrap;
  }
  .segmented.wrap button {
    flex: 1 1 40%;
  }
  .segmented.tiny button {
    padding: 3px 6px;
    font-size: 12px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .wide-swatch {
    width: 34px;
    border-radius: 6px;
  }
  .pair {
    display: flex;
    gap: 8px;
  }
  .pair button {
    flex: 1;
    justify-content: center;
  }
  .note {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .field {
    display: grid;
    grid-template-columns: 84px 1fr auto;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .field.stacked {
    grid-template-columns: 1fr;
    gap: 8px;
  }
  .field select {
    min-width: 0;
  }
  .field input[type="range"] {
    width: 100%;
    background: transparent;
  }
  .value {
    min-width: 40px;
    text-align: right;
    font-size: 12px;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  /* Cursor looks and effects, drawn as they will appear. */
  .looks {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(76px, 1fr));
    gap: 6px;
  }
  .look {
    flex-direction: column;
    justify-content: center;
    gap: 4px;
    height: 72px;
    padding: 6px 4px;
    font-size: 11px;
    font-weight: 550;
    text-align: center;
    line-height: 1.2;
  }
  .look svg,
  .look img {
    width: 26px;
    height: 26px;
    object-fit: contain;
  }
  .look.active svg path {
    filter: drop-shadow(0 0 1px rgb(0 0 0 / 0.4));
  }
  .ghost-arrow {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    opacity: 0.5;
  }
  .slash {
    stroke: currentColor;
    stroke-width: 2.4;
    stroke-linecap: round;
  }
  .mini-swatches {
    display: flex;
    gap: 6px;
  }
  .mini-swatches .swatch {
    width: 20px;
    height: 20px;
  }
  .effects {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
  }
  .effect {
    position: relative;
  }
  .effect > button {
    width: 100%;
    flex-direction: column;
    justify-content: center;
    gap: 4px;
    height: 76px;
    font-size: 12px;
  }
  .effect.active > button {
    background: var(--accent);
    color: var(--on-accent);
  }
  .effect svg {
    width: 36px;
    height: 36px;
  }
  .dot-color {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 18px;
    height: 18px;
    padding: 0;
    border-radius: 50%;
    border: 2px solid var(--surface) !important;
    overflow: hidden;
    cursor: pointer;
  }
  .dot-color::-webkit-color-swatch-wrapper {
    padding: 0;
  }
  .dot-color::-webkit-color-swatch {
    border: none;
    border-radius: 50%;
  }

  /* Export, always at the bottom of the panel. */
  .export-card {
    display: grid;
    gap: 12px;
    padding: 18px 22px 22px;
    border-top: 1px solid var(--border);
  }
  .settings {
    display: grid;
    gap: 10px;
  }
  .value.wide {
    min-width: 88px;
  }
  .checks .check {
    flex: 1;
  }
  .destination {
    justify-content: flex-start;
    gap: 8px;
    padding: 8px 10px;
    background: transparent;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-muted);
  }
  .destination span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .meta {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .save {
    justify-content: center;
    min-height: 46px;
    background: var(--accent) !important;
    color: var(--on-accent) !important;
    font-size: 15px;
    font-weight: 650;
  }
  .save:hover:not(:disabled) {
    background: var(--accent-hover) !important;
  }
  .save.busy {
    background: linear-gradient(90deg, var(--accent) var(--done), color-mix(in srgb, var(--accent) 45%, var(--control)) var(--done)) !important;
  }
  /* Secondary actions in white, easy to see in both themes. */
  .light {
    min-height: 38px;
    background: #ffffff !important;
    color: #1c1c1f !important;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.14);
  }
  .light:hover:not(:disabled) {
    background: #ececf0 !important;
  }
</style>
