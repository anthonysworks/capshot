<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { editorCopy, editorRead, editorSave } from "../lib/api";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import { type Rect, type Shape, type ShapeKind, bounds, hit, normalize, render } from "./shapes";

  let { path }: { path: string } = $props();

  type Tool = "select" | "crop" | ShapeKind;
  const TOOLS: { tool: Tool; icon: IconName; label: string; key: string }[] = [
    { tool: "select", icon: "select", label: "Select and move", key: "V" },
    { tool: "arrow", icon: "arrow", label: "Arrow", key: "A" },
    { tool: "rect", icon: "rect", label: "Rectangle", key: "R" },
    { tool: "ellipse", icon: "ellipse", label: "Ellipse", key: "E" },
    { tool: "highlight", icon: "highlight", label: "Highlighter", key: "H" },
    { tool: "text", icon: "text", label: "Text", key: "T" },
    { tool: "number", icon: "number", label: "Numbered step", key: "N" },
    { tool: "pixelate", icon: "pixelate", label: "Pixelate (hide sensitive information)", key: "B" },
    { tool: "crop", icon: "crop", label: "Crop", key: "C" },
  ];
  const COLORS = ["#e5484d", "#f5a524", "#30a46c", "#3b82f6", "#ffffff", "#111111"];
  const SIZES = [
    { level: 1, label: "Thin" },
    { level: 2, label: "Medium" },
    { level: 4, label: "Thick" },
  ];

  let image = $state<ImageBitmap | null>(null);
  let error = $state("");
  let tool = $state<Tool>("arrow");
  let color = $state<string>("#e5484d");
  let level = $state(2);
  let shapes = $state<Shape[]>([]);
  let crop = $state<Rect | null>(null);
  let draft = $state<Shape | null>(null);
  let cropDraft = $state<Rect | null>(null);
  let selected = $state<number | null>(null);
  let past = $state<string[]>([]);
  let future = $state<string[]>([]);
  let dirty = $state(false);
  let toast = $state("");
  let editing = $state<{ x: number; y: number; text: string; id: number | null } | null>(null);

  let canvas: HTMLCanvasElement;
  let stage: HTMLDivElement;
  let textarea = $state<HTMLTextAreaElement | null>(null);
  const scratch = document.createElement("canvas");
  let view = $state({ s: 1, ox: 0, oy: 0, dpr: 1 });
  let nextId = 1;
  let drag: { x: number; y: number; moved: boolean } | null = null;

  /** Stroke width for the current level, relative to the image size so
   * annotations look the same on a small capture and a 4K one. */
  const unit = $derived(image ? Math.max(1, Math.round(Math.max(image.width, image.height) / 1400)) : 1);
  const stroke = $derived((level + 2) * unit);

  onMount(() => {
    (async () => {
      try {
        const bytes = await editorRead(path);
        image = await createImageBitmap(new Blob([bytes]));
      } catch (e) {
        error = String(e);
      }
    })();
    const observer = new ResizeObserver(layout);
    observer.observe(stage);
    const unlisten = getCurrentWindow().onCloseRequested((event) => {
      if (dirty && !confirm("Close without saving your changes?")) event.preventDefault();
    });
    return () => {
      observer.disconnect();
      unlisten.then((f) => f());
    };
  });

  function layout() {
    if (!canvas || !stage) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(stage.clientWidth * dpr);
    canvas.height = Math.round(stage.clientHeight * dpr);
    if (!image) return;
    const margin = 24 * dpr;
    // Never enlarge: one image pixel per screen pixel at most.
    const s = Math.min((canvas.width - margin * 2) / image.width, (canvas.height - margin * 2) / image.height, 1);
    view = {
      s,
      ox: (canvas.width - image.width * s) / 2,
      oy: (canvas.height - image.height * s) / 2,
      dpr,
    };
  }

  $effect(() => {
    if (image) layout();
  });

  // Redraw whenever anything visible changes.
  $effect(() => {
    const state = $state.snapshot({ shapes, crop, draft, cropDraft, selected, view, editing });
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
    render(ctx, image, extra ? [...visible, extra] : visible, scratch);

    const area = cropDraft ?? crop;
    if (area) {
      ctx.fillStyle = "rgba(0,0,0,0.55)";
      const { width: w, height: h } = image;
      ctx.fillRect(0, 0, w, area.y);
      ctx.fillRect(0, area.y + area.h, w, h - area.y - area.h);
      ctx.fillRect(0, area.y, area.x, area.h);
      ctx.fillRect(area.x + area.w, area.y, w - area.x - area.w, area.h);
      outline(ctx, area);
    }
    const chosen = list.find((sh) => sh.id === selected);
    if (chosen && tool === "select") outline(ctx, bounds(ctx, chosen));
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

  // ── History ────────────────────────────────────────────────────────────

  const serialize = () => JSON.stringify({ shapes: $state.snapshot(shapes), crop: $state.snapshot(crop) });

  function commit(change: () => void) {
    past.push(serialize());
    future = [];
    change();
    dirty = true;
  }

  function restore(from: string[], to: string[]) {
    const state = from.pop();
    if (state === undefined) return;
    to.push(serialize());
    const parsed = JSON.parse(state);
    shapes = parsed.shapes;
    crop = parsed.crop;
    selected = null;
    dirty = true;
  }

  const undo = () => restore(past, future);
  const redo = () => restore(future, past);

  // ── Pointer ────────────────────────────────────────────────────────────

  function toImage(e: PointerEvent | MouseEvent) {
    const rect = canvas.getBoundingClientRect();
    return {
      x: ((e.clientX - rect.left) * view.dpr - view.ox) / view.s,
      y: ((e.clientY - rect.top) * view.dpr - view.oy) / view.s,
    };
  }

  function clamp(p: { x: number; y: number }) {
    if (!image) return p;
    return { x: Math.max(0, Math.min(image.width, p.x)), y: Math.max(0, Math.min(image.height, p.y)) };
  }

  function topmost(x: number, y: number): Shape | undefined {
    const ctx = canvas.getContext("2d");
    if (!ctx) return undefined;
    return [...shapes].reverse().find((sh) => hit(ctx, sh, x, y, 6 / view.s));
  }

  function pointerDown(e: PointerEvent) {
    if (!image || e.button !== 0) return;
    // Keeps the focus where it is (the text box opened below).
    e.preventDefault();
    if (editing) return commitText();
    canvas.setPointerCapture(e.pointerId);
    const p = clamp(toImage(e));
    drag = { ...p, moved: false };

    if (tool === "select") {
      const found = topmost(p.x, p.y);
      selected = found?.id ?? null;
      if (found) past.push(serialize());
      return;
    }
    if (tool === "text") {
      const found = shapes.find((sh) => sh.kind === "text" && sh.id === topmost(p.x, p.y)?.id);
      editing = found
        ? { x: found.x1, y: found.y1, text: found.text ?? "", id: found.id }
        : { x: p.x, y: p.y, text: "", id: null };
      drag = null;
      return;
    }
    if (tool === "number") {
      const n = Math.max(0, ...shapes.map((sh) => sh.n ?? 0)) + 1;
      commit(() =>
        shapes.push({ id: nextId++, kind: "number", color, size: stroke, x1: p.x, y1: p.y, x2: p.x, y2: p.y, n }),
      );
      drag = null;
      return;
    }
    if (tool === "crop") {
      cropDraft = { x: p.x, y: p.y, w: 0, h: 0 };
      return;
    }
    draft = { id: nextId++, kind: tool, color, size: stroke, x1: p.x, y1: p.y, x2: p.x, y2: p.y };
  }

  function pointerMove(e: PointerEvent) {
    if (!drag) return;
    const p = clamp(toImage(e));
    drag.moved ||= Math.hypot(p.x - drag.x, p.y - drag.y) * view.s > 3;
    if (tool === "select" && selected !== null) {
      const sh = shapes.find((x) => x.id === selected);
      if (!sh) return;
      const dx = p.x - drag.x;
      const dy = p.y - drag.y;
      sh.x1 += dx;
      sh.x2 += dx;
      sh.y1 += dy;
      sh.y2 += dy;
      drag.x = p.x;
      drag.y = p.y;
    } else if (tool === "crop" && cropDraft) {
      cropDraft = normalize({ x1: drag.x, y1: drag.y, x2: p.x, y2: p.y });
    } else if (draft) {
      draft.x2 = p.x;
      draft.y2 = p.y;
    }
  }

  function pointerUp() {
    const moved = drag?.moved ?? false;
    drag = null;
    if (tool === "select") {
      // Nothing moved: forget the history entry taken on press.
      if (!moved && selected !== null) past.pop();
      else if (moved) {
        future = [];
        dirty = true;
      }
      return;
    }
    if (tool === "crop" && cropDraft) {
      const area = cropDraft;
      cropDraft = null;
      if (moved && area.w > 4 && area.h > 4) {
        commit(() => (crop = { x: Math.round(area.x), y: Math.round(area.y), w: Math.round(area.w), h: Math.round(area.h) }));
      }
      return;
    }
    if (draft) {
      const shape = draft;
      draft = null;
      if (moved) commit(() => shapes.push(shape));
    }
  }

  // ── Text ───────────────────────────────────────────────────────────────

  $effect(() => {
    const box = textarea;
    if (editing && box) setTimeout(() => box.focus());
  });

  function commitText() {
    if (!editing) return;
    const { x, y, text, id } = editing;
    editing = null;
    const trimmed = text.replace(/\s+$/, "");
    const existing = shapes.find((sh) => sh.id === id);
    if (existing) {
      commit(() => {
        if (trimmed) existing.text = trimmed;
        else shapes = shapes.filter((sh) => sh.id !== id);
      });
    } else if (trimmed) {
      commit(() =>
        shapes.push({ id: nextId++, kind: "text", color, size: (level + 1) * unit, x1: x, y1: y, x2: x, y2: y, text: trimmed }),
      );
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

  // ── Output ─────────────────────────────────────────────────────────────

  async function flatten(): Promise<Uint8Array> {
    if (!image) throw new Error("no image");
    const area = crop ?? { x: 0, y: 0, w: image.width, h: image.height };
    const out = document.createElement("canvas");
    out.width = area.w;
    out.height = area.h;
    const ctx = out.getContext("2d");
    if (!ctx) throw new Error("no canvas");
    ctx.translate(-area.x, -area.y);
    render(ctx, image, $state.snapshot(shapes) as Shape[], scratch);
    const blob = await new Promise<Blob | null>((resolve) => out.toBlob(resolve, "image/png"));
    if (!blob) throw new Error("cannot encode the image");
    return new Uint8Array(await blob.arrayBuffer());
  }

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function notify(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 2200);
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

  // ── Keyboard ───────────────────────────────────────────────────────────

  function keyDown(e: KeyboardEvent) {
    if (editing) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
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
      save();
    } else if ((e.key === "Delete" || e.key === "Backspace") && selected !== null) {
      const id = selected;
      selected = null;
      commit(() => (shapes = shapes.filter((sh) => sh.id !== id)));
    } else if (e.key === "Escape") {
      selected = null;
      draft = null;
      cropDraft = null;
    } else if (!ctrl && !e.altKey) {
      const found = TOOLS.find((t) => t.key.toLowerCase() === key);
      if (found) tool = found.tool;
    }
  }

  const textStyle = $derived.by(() => {
    if (!editing) return "";
    const left = (view.ox + editing.x * view.s) / view.dpr;
    const top = (view.oy + editing.y * view.s) / view.dpr;
    const size = ((16 + (level + 1) * unit * 6) * view.s) / view.dpr;
    return `left:${left}px;top:${top}px;font-size:${size}px;color:${color}`;
  });
</script>

<svelte:window onkeydown={keyDown} />

<div class="editor">
  <header>
    <div class="group" role="toolbar" aria-label="Tools">
      {#each TOOLS as t (t.tool)}
        <button
          type="button"
          class="icon"
          class:active={tool === t.tool}
          title="{t.label} ({t.key})"
          aria-label={t.label}
          onclick={() => (tool = t.tool)}><Icon name={t.icon} /></button
        >
      {/each}
    </div>
    <div class="group" role="radiogroup" aria-label="Colour">
      {#each COLORS as c (c)}
        <button
          type="button"
          class="swatch"
          class:active={color === c}
          style="--swatch:{c}"
          aria-label="Colour {c}"
          onclick={() => {
            color = c;
            const sh = shapes.find((x) => x.id === selected);
            if (sh && tool === "select") commit(() => (sh.color = c));
          }}
        ></button>
      {/each}
    </div>
    <div class="group" role="radiogroup" aria-label="Thickness">
      {#each SIZES as s (s.level)}
        <button
          type="button"
          class="icon"
          class:active={level === s.level}
          title={s.label}
          aria-label={s.label}
          onclick={() => (level = s.level)}><span class="dot" style="--d:{3 + s.level * 2}px"></span></button
        >
      {/each}
    </div>
    <div class="group">
      <button type="button" class="icon" title="Undo (Ctrl+Z)" aria-label="Undo" disabled={!past.length} onclick={undo}
        ><Icon name="undo" /></button
      >
      <button type="button" class="icon" title="Redo (Ctrl+Y)" aria-label="Redo" disabled={!future.length} onclick={redo}
        ><Icon name="redo" /></button
      >
      {#if crop}
        <button type="button" class="text" onclick={() => commit(() => (crop = null))}>Reset crop</button>
      {/if}
    </div>
    <div class="spacer"></div>
    <button type="button" class="text" title="Copy (Ctrl+C)" onclick={copy}><Icon name="copy" /> Copy</button>
    <button type="button" class="text primary" title="Save (Ctrl+S)" onclick={save}><Icon name="save" /> Save</button>
  </header>

  <div class="stage" bind:this={stage}>
    <canvas
      bind:this={canvas}
      class:crosshair={tool !== "select"}
      onpointerdown={pointerDown}
      onpointermove={pointerMove}
      onpointerup={pointerUp}
    ></canvas>
    {#if editing}
      <textarea
        bind:this={textarea}
        bind:value={editing.text}
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
      <div class="toast" role="status">{toast}</div>
    {/if}
  </div>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-right: 8px;
    border-right: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
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
  button.icon.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  button.text {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    width: 22px;
    height: 22px;
    padding: 0;
    margin: 0 2px;
    border-radius: 50%;
    background: var(--swatch);
    border: 2px solid var(--border);
  }
  .swatch.active {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .dot {
    width: var(--d);
    height: var(--d);
    border-radius: 50%;
    background: currentColor;
  }
  .stage {
    position: relative;
    flex: 1;
    overflow: hidden;
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 92%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 20px
      20px;
  }
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    touch-action: none;
  }
  canvas.crosshair {
    cursor: crosshair;
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
  .message {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    margin: 0;
    color: var(--text-muted);
  }
  .toast {
    position: absolute;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    padding: 8px 14px;
    border-radius: 6px;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.25);
  }
</style>
