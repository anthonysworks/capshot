<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    type QuickInfo,
    copyText as copyToClipboard,
    editorRead,
    quickCodes,
    quickCopy,
    quickDiscard,
    quickEdit,
    quickInfo,
    quickPin,
    quickSave,
    quickSaveAs,
    quickText,
    showInFolder,
  } from "../lib/api";
  import Icon from "../editor/Icon.svelte";
  import type { IconName } from "../editor/icons";

  /** The toolbar shown after a capture: a preview and what can be done
   * with it. It grows for the text or QR result and for the colour picker,
   * and goes back to its size afterwards. */
  let { path: initial }: { path: string } = $props();

  // svelte-ignore state_referenced_locally
  let path = $state(initial);
  let info = $state<QuickInfo | null>(null);
  let url = $state("");
  let size = $state({ w: 0, h: 0 });
  let toast = $state("");
  let busy = $state<"" | "text" | "codes" | "save">("");
  let result = $state<{ kind: "text"; text: string } | { kind: "codes"; codes: string[] } | null>(null);
  let picking = $state(false);
  let picked = $state<{ x: number; y: number; hex: string } | null>(null);
  let colours = $state<string[]>([]);
  let asking = $state(false);
  let video = $state<HTMLVideoElement | null>(null);
  let img = $state<HTMLImageElement | null>(null);
  let pixels: CanvasRenderingContext2D | null = null;

  const name = $derived(path.split(/[\\/]/).pop() ?? path);
  const isVideo = $derived(info?.video ?? /\.(mp4|mov|m4v|wmv|avi|mkv)$/i.test(path));

  /** Window sizes (logical pixels). */
  const COMPACT = $derived({ w: 540, h: isVideo ? 196 : 252 });
  const WITH_RESULT = { w: 540, h: 460 };
  const PICKING = { w: 780, h: 600 };
  const target = $derived(picking ? PICKING : result ? WITH_RESULT : COMPACT);
  let applied = { w: 0, h: 0 };

  onMount(() => {
    load();
    const stop = getCurrentWindow().onCloseRequested((e) => {
      // A recording is lost when closed unsaved: ask first.
      if (info?.video && !info.saved) {
        e.preventDefault();
        asking = true;
      }
    });
    return () => {
      stop.then((f) => f());
      if (url.startsWith("blob:")) URL.revokeObjectURL(url);
    };
  });

  // Grows or shrinks the window upwards, keeping its bottom edge in place.
  $effect(() => {
    const { w, h } = target;
    if (!applied.w) {
      applied = { w, h };
      return;
    }
    const dh = h - applied.h;
    const dw = w - applied.w;
    const win = getCurrentWindow();
    applied = { w, h };
    win.setSize(new LogicalSize(w, h)).catch(() => {});
    win
      .setPosition(new LogicalPosition(Math.max(0, window.screenX - dw), Math.max(0, window.screenY - dh)))
      .catch(() => {});
  });

  async function load() {
    info = await quickInfo(path).catch(() => null);
    if (isVideo) {
      url = convertFileSrc(path);
      return;
    }
    const bytes = await editorRead(path);
    if (url.startsWith("blob:")) URL.revokeObjectURL(url);
    url = URL.createObjectURL(new Blob([bytes]));
  }

  function loadedImage() {
    if (!img) return;
    size = { w: img.naturalWidth, h: img.naturalHeight };
    const canvas = document.createElement("canvas");
    canvas.width = size.w;
    canvas.height = size.h;
    pixels = canvas.getContext("2d", { willReadFrequently: true });
    pixels?.drawImage(img, 0, 0);
  }

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function notify(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 2600);
  }

  async function copy() {
    try {
      await quickCopy(path);
      notify(isVideo ? "Copied: paste it as a file" : "Copied");
    } catch (e) {
      notify(String(e));
    }
  }

  /** Lets go of the video file so that it can be moved. */
  function release() {
    if (!video) return;
    video.pause();
    video.removeAttribute("src");
    video.load();
  }

  async function save(as = false) {
    if (busy) return;
    busy = "save";
    if (isVideo) release();
    try {
      const saved = as ? await quickSaveAs(path) : await quickSave(path);
      if (saved) {
        path = saved;
        notify("Saved");
      }
    } catch (e) {
      notify(String(e));
    }
    busy = "";
    await load();
  }

  async function edit() {
    if (isVideo) release();
    try {
      await quickEdit(path);
    } catch (e) {
      notify(String(e));
      await load();
    }
  }

  async function readText() {
    busy = "text";
    try {
      const text = await quickText(path);
      result = { kind: "text", text };
      if (text.trim()) {
        await copyToClipboard(text);
        notify("Text copied");
      } else {
        notify("No text found");
      }
    } catch (e) {
      notify(String(e));
    }
    busy = "";
  }

  async function readCodes() {
    busy = "codes";
    try {
      const codes = await quickCodes(path);
      result = { kind: "codes", codes };
      if (!codes.length) notify("No QR code found");
    } catch (e) {
      notify(String(e));
    }
    busy = "";
  }

  async function copyText(text: string, what = "Copied") {
    try {
      await copyToClipboard(text);
      notify(what);
    } catch (e) {
      notify(String(e));
    }
  }

  async function pin() {
    try {
      await quickPin(path);
    } catch (e) {
      notify(String(e));
    }
  }

  /** Colour under the pointer, in picture pixels. */
  function colourAt(e: PointerEvent) {
    if (!img || !pixels || !size.w) return null;
    const r = img.getBoundingClientRect();
    const x = Math.floor(((e.clientX - r.left) / r.width) * size.w);
    const y = Math.floor(((e.clientY - r.top) / r.height) * size.h);
    if (x < 0 || y < 0 || x >= size.w || y >= size.h) return null;
    const [red = 0, green = 0, blue = 0] = pixels.getImageData(x, y, 1, 1).data;
    const hex = `#${[red, green, blue].map((v) => v.toString(16).padStart(2, "0")).join("")}`.toUpperCase();
    return { x: e.clientX - r.left, y: e.clientY - r.top, hex };
  }

  function pickMove(e: PointerEvent) {
    if (picking) picked = colourAt(e);
  }

  async function pickClick(e: PointerEvent) {
    if (!picking) return;
    const at = colourAt(e);
    if (!at) return;
    colours = [at.hex, ...colours.filter((c) => c !== at.hex)].slice(0, 6);
    await copyText(at.hex, `${at.hex} copied`);
  }

  const close = () => getCurrentWindow().close();

  /** Dragging anywhere that is not a control moves the toolbar. */
  function drag(e: PointerEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button, textarea, input, .result, img.pickable")) return;
    getCurrentWindow().startDragging();
  }

  function keyDown(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === "TEXTAREA") return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (ctrl && key === "c") copy();
    else if (ctrl && key === "s") save(e.shiftKey);
    else if (!ctrl && key === "e") edit();
    else if (key === "escape") {
      if (asking) asking = false;
      else if (picking) picking = false;
      else if (result) result = null;
      else close();
    } else return;
    e.preventDefault();
  }

  const isLink = (s: string) => /^https?:\/\//i.test(s);

  const tools: { icon: IconName; label: string; run: () => void; images?: boolean; on?: () => boolean }[] = [
    { icon: "save", label: "Save", run: () => save() },
    { icon: "saveCopy", label: "Save as", run: () => save(true) },
    { icon: "edit", label: "Edit", run: edit },
    { icon: "pin", label: "Pin", run: pin, images: true },
    { icon: "text_scan", label: "Text", run: readText, images: true },
    { icon: "qr", label: "QR code", run: readCodes, images: true },
    { icon: "picker", label: "Colour", run: () => (picking = !picking), images: true, on: () => picking },
  ];
  const shown = $derived(tools.filter((t) => !t.images || !isVideo));
</script>

<svelte:window onkeydown={keyDown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="quick" class:picking onpointerdown={drag}>
  {#if picking}
    <div class="big">
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <img
        bind:this={img}
        class="pickable"
        src={url}
        alt="Capture"
        draggable="false"
        onload={loadedImage}
        onpointermove={pickMove}
        onpointerleave={() => (picked = null)}
        onpointerdown={pickClick}
      />
      {#if picked}
        <div class="loupe" style="left:{picked.x + 20}px;top:{picked.y + 20}px">
          <span class="dot" style="background:{picked.hex}"></span>{picked.hex}
        </div>
      {/if}
    </div>
    <div class="pickbar">
      <span>Click a colour to copy it</span>
      {#each colours as c (c)}
        <button type="button" class="swatch" style="background:{c}" title="{c}: copy" aria-label="Copy {c}" onclick={() => copyText(c, `${c} copied`)}
        ></button>
      {/each}
      <button type="button" class="primary" onclick={() => (picking = false)}>Done</button>
    </div>
  {:else}
    <div class="card">
      <div class="preview">
        {#if url && isVideo}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video bind:this={video} src={url} autoplay muted loop></video>
        {:else if url}
          <img bind:this={img} src={url} alt="Capture" draggable="false" onload={loadedImage} />
        {/if}
      </div>
      <div class="side">
        <div class="head">
          <span class="status" title={path}>
            {#if info?.saved}
              Saved<button type="button" class="link" onclick={() => showInFolder(path)}>Show</button>
            {:else}
              {isVideo ? "Recording" : "Screenshot"}{size.w ? ` · ${size.w}×${size.h}` : ""}
            {/if}
          </span>
          <button type="button" class="x" title="Close (Esc)" aria-label="Close" onclick={close}><Icon name="close" /></button>
        </div>
        <button type="button" class="copy" onclick={copy}><Icon name="copy" /> Copy</button>
        <div class="grid" style="--n:{shown.length > 4 ? 4 : shown.length}">
          {#each shown as t (t.label)}
            <button
              type="button"
              class="tool"
              class:active={t.on?.()}
              disabled={(t.label === "Save" && (info?.saved || busy === "save")) || (t.label === "Text" && busy === "text") || (t.label === "QR code" && busy === "codes")}
              onclick={t.run}
              ><Icon name={t.icon} /><span>{t.label === "Save" && info?.saved ? "Saved" : t.label}</span></button
            >
          {/each}
        </div>
      </div>
    </div>
    {#if result}
      <div class="result">
        <div class="result-head">
          <strong>{result.kind === "text" ? "Text in the picture" : "QR codes"}</strong>
          <button type="button" class="x" aria-label="Close" onclick={() => (result = null)}><Icon name="close" /></button>
        </div>
        {#if result.kind === "text"}
          <textarea bind:value={result.text} spellcheck="false" placeholder="No text found"></textarea>
          {#if result.text.trim()}
            {@const text = result.text}
            <button type="button" class="light" onclick={() => copyText(text, "Text copied")}><Icon name="copy" /> Copy text</button>
          {/if}
        {:else if !result.codes.length}
          <p class="faint">No QR code found in the picture.</p>
        {:else}
          {#each result.codes as code, i (i)}
            <div class="code">
              <span class="value" class:link={isLink(code)}>{code}</span>
              <button type="button" class="light" onclick={() => copyText(code, isLink(code) ? "Link copied" : "Copied")}
                ><Icon name="copy" /> Copy</button
              >
            </div>
          {/each}
        {/if}
      </div>
    {/if}
  {/if}
  {#if toast}<div class="toast" role="status">{toast}</div>{/if}

  {#if asking}
    <div class="backdrop">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="ask-title">
        <h2 id="ask-title">Delete this recording?</h2>
        <p>It was never saved. Closing deletes it.</p>
        <div class="dialog-actions">
          <button
            type="button"
            class="primary"
            onclick={async () => {
              asking = false;
              await save();
            }}><Icon name="save" /> Save</button
          >
          <button type="button" class="danger" onclick={() => quickDiscard(path)}>Delete</button>
          <button type="button" onclick={() => (asking = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .quick {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
  }
  .card {
    display: grid;
    grid-template-columns: 190px minmax(0, 1fr);
    gap: 16px;
    padding: 16px;
  }
  .preview {
    display: grid;
    place-items: center;
    height: 100%;
    min-height: 120px;
    overflow: hidden;
    border-radius: var(--radius);
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 92%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 14px
      14px;
  }
  .preview img,
  .preview video {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .side {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 24px;
  }
  .status {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-faint);
    font-size: 12px;
  }
  .x {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 28px;
    padding: 0;
    border-radius: 50%;
    background: transparent;
  }
  .copy {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 46px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 16px;
    font-weight: 650;
  }
  .copy:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(var(--n), minmax(0, 1fr));
    gap: 8px;
  }
  /* White in both themes: easy to see next to the green button. */
  .tool {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    min-height: 54px;
    padding: 6px 2px;
    background: #ffffff;
    color: #1c1c1f;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.14);
    font-size: 11px;
    font-weight: 600;
  }
  .tool:hover:not(:disabled) {
    background: #ececf0;
  }
  .tool.active {
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: none;
  }
  .light {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 40px;
    background: #ffffff;
    color: #1c1c1f;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.14);
  }
  .link {
    padding: 0 0 0 8px;
    background: none;
    color: var(--accent-text);
    font-size: 12px;
  }
  .result {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 10px;
    min-height: 0;
    margin: 0 16px 16px;
    padding: 14px;
    border-radius: var(--radius);
    background: var(--bg);
  }
  .result-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .result textarea {
    flex: 1;
    min-height: 0;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font: 13px/1.5 var(--font);
    resize: none;
    user-select: text;
  }
  .code {
    display: grid;
    gap: 8px;
    padding: 10px;
    border-radius: var(--radius);
    background: var(--surface);
  }
  .code .value {
    overflow-wrap: anywhere;
    user-select: text;
    font-size: 13px;
  }
  .code .link {
    padding: 0;
    color: var(--accent-text);
    font-size: 13px;
  }
  .faint {
    color: var(--text-faint);
    font-size: 12px;
  }
  .big {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    place-items: center;
    padding: 16px;
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 92%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 14px
      14px;
  }
  .big img {
    max-width: 100%;
    max-height: 100%;
    cursor: crosshair;
    border-radius: 4px;
  }
  .loupe {
    position: absolute;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 12px 5px 6px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow);
    font: 600 13px var(--font-mono);
    pointer-events: none;
  }
  .dot,
  .swatch {
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 50%;
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.5);
  }
  .pickbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }
  .pickbar span {
    flex: 1;
    color: var(--text-muted);
  }
  .toast {
    position: absolute;
    left: 50%;
    bottom: 12px;
    transform: translateX(-50%);
    padding: 7px 16px;
    border-radius: 999px;
    background: var(--text);
    color: var(--surface);
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    pointer-events: none;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.45);
  }
  .dialog {
    width: calc(100% - 32px);
    padding: 20px 22px;
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow);
  }
  .dialog h2 {
    margin: 0 0 4px;
    font-size: 18px;
  }
  .dialog p {
    margin: 0 0 16px;
    color: var(--text-muted);
  }
  .dialog-actions {
    display: flex;
    gap: 8px;
  }
  .dialog-actions button {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .danger {
    color: var(--danger);
  }
  .danger:hover {
    background: var(--danger) !important;
    color: #fff;
  }
</style>
