<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    type QuickInfo,
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
    copyText as copyToClipboard,
    showInFolder,
  } from "../lib/api";
  import Icon from "../editor/Icon.svelte";
  import Kbd from "../editor/Kbd.svelte";

  let { path: initial }: { path: string } = $props();

  // svelte-ignore state_referenced_locally
  let path = $state(initial);
  let info = $state<QuickInfo | null>(null);
  let url = $state("");
  let size = $state({ w: 0, h: 0 });
  let toast = $state("");
  let busy = $state<"" | "text" | "codes" | "save">("");
  /** What a tool found, shown over the picture. */
  let result = $state<{ kind: "text"; text: string } | { kind: "codes"; codes: string[] } | null>(null);
  let picking = $state(false);
  let picked = $state<{ x: number; y: number; hex: string } | null>(null);
  let colours = $state<string[]>([]);
  let asking = $state(false);
  let video = $state<HTMLVideoElement | null>(null);
  let img = $state<HTMLImageElement | null>(null);
  /** The picture's pixels, for the colour picker. */
  let pixels: CanvasRenderingContext2D | null = null;

  const name = $derived(path.split(/[\\/]/).pop() ?? path);
  const isVideo = $derived(info?.video ?? /\.(mp4|mov|m4v|wmv|avi|mkv)$/i.test(path));

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
      notify(isVideo ? "Video copied: paste it as a file" : "Copied to the clipboard");
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
        notify(`Saved · ${saved.split(/[\\/]/).pop()}`);
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

  function close() {
    getCurrentWindow().close();
  }

  function keyDown(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === "TEXTAREA") return;
    const ctrl = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (ctrl && key === "c") copy();
    else if (ctrl && key === "s") save(e.shiftKey);
    else if (!ctrl && key === "e") edit();
    else if (!ctrl && !isVideo && key === "i") picking = !picking;
    else if (key === "escape") {
      if (asking) asking = false;
      else if (picking) picking = false;
      else if (result) result = null;
      else close();
    } else return;
    e.preventDefault();
  }

  const isLink = (s: string) => /^https?:\/\//i.test(s);
</script>

<svelte:window onkeydown={keyDown} />

<div class="quick">
  <div class="preview" class:picking>
    {#if url && isVideo}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video bind:this={video} src={url} controls autoplay muted loop></video>
    {:else if url}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <img
        bind:this={img}
        src={url}
        alt="Capture"
        draggable="false"
        onload={loadedImage}
        onpointermove={pickMove}
        onpointerleave={() => (picked = null)}
        onpointerdown={pickClick}
      />
    {/if}
    {#if picking && picked}
      <div class="loupe" style="left:{picked.x}px;top:{picked.y}px">
        <span class="chip" style="background:{picked.hex}"></span>{picked.hex}
      </div>
    {/if}
    {#if result}
      <div class="result">
        <div class="result-head">
          <strong>{result.kind === "text" ? "Text in the picture" : "QR codes"}</strong>
          <button type="button" class="icon" aria-label="Close" onclick={() => (result = null)}><Icon name="close" /></button>
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
    {#if toast}<div class="toast" role="status">{toast}</div>{/if}
  </div>

  <footer>
    <div class="actions">
      <button type="button" class="copy" onclick={copy}><Icon name="copy" /> Copy <Kbd keys="Ctrl+C" /></button>
      <button type="button" class="light" onclick={() => save()} disabled={busy === "save" || info?.saved}
        ><Icon name="save" /> {info?.saved ? "Saved" : "Save"}</button
      >
      <button type="button" class="light" onclick={() => save(true)} disabled={busy === "save"}><Icon name="saveCopy" /> Save as…</button>
      <button type="button" class="light" onclick={edit}><Icon name="edit" /> Edit</button>
    </div>
    {#if !isVideo}
      <div class="tools">
        <button type="button" class="tool" onclick={readText} disabled={busy === "text"}
          ><Icon name="text_scan" /> {busy === "text" ? "Reading…" : "Copy text"}</button
        >
        <button type="button" class="tool" onclick={readCodes} disabled={busy === "codes"}><Icon name="qr" /> QR code</button>
        <button type="button" class="tool" class:active={picking} aria-pressed={picking} onclick={() => (picking = !picking)}
          ><Icon name="picker" /> Pick colour</button
        >
        <button type="button" class="tool" onclick={pin}><Icon name="pin" /> Pin</button>
        {#each colours as c (c)}
          <button type="button" class="swatch" style="background:{c}" title="{c}: copy" aria-label="Copy {c}" onclick={() => copyText(c, `${c} copied`)}
          ></button>
        {/each}
      </div>
    {/if}
    <p class="status">
      {#if info?.saved}
        Saved in {info.folder}
        <button type="button" class="link" onclick={() => showInFolder(path)}>Show</button>
      {:else}
        {name}{size.w ? ` · ${size.w}×${size.h}` : ""} · not saved yet{isVideo ? ": closing deletes it" : ""}
      {/if}
    </p>
  </footer>

  {#if asking}
    <div class="backdrop">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="ask-title">
        <h2 id="ask-title">Close without saving?</h2>
        <p>This recording was never saved. Closing deletes it.</p>
        <div class="dialog-actions">
          <button
            type="button"
            class="primary"
            onclick={async () => {
              asking = false;
              await save();
            }}><Icon name="save" /> Save</button
          >
          <button type="button" class="danger" onclick={() => quickDiscard(path)}>Delete and close</button>
          <button type="button" onclick={() => (asking = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .quick {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg);
  }
  .preview {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    place-items: center;
    padding: 24px;
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--bg) 94%, var(--text)) 0 25%, var(--bg) 0 50%) 0 0 / 20px
      20px;
  }
  img,
  video {
    max-width: 100%;
    max-height: 100%;
    min-width: 0;
    min-height: 0;
    border-radius: 6px;
    box-shadow: var(--shadow);
  }
  .picking img {
    cursor: crosshair;
  }
  .loupe {
    position: absolute;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px 6px 6px;
    margin: 24px 0 0 24px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow);
    font: 600 13px var(--font-mono);
    pointer-events: none;
    translate: 0 0;
  }
  .loupe .chip {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.5);
  }
  .result {
    position: absolute;
    right: 20px;
    top: 20px;
    bottom: 20px;
    width: min(360px, calc(100% - 40px));
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 18px;
    border-radius: var(--radius-lg);
    background: var(--surface);
    box-shadow: var(--shadow);
  }
  .result-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .result textarea {
    flex: 1;
    min-height: 0;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font: 13px/1.5 var(--font);
    resize: none;
    user-select: text;
  }
  .code {
    display: grid;
    gap: 8px;
    padding: 12px;
    border-radius: var(--radius);
    background: var(--bg);
  }
  .code .value {
    overflow-wrap: anywhere;
    user-select: text;
    font-size: 13px;
  }
  .code .link {
    color: var(--accent-text);
  }
  .faint,
  .status {
    color: var(--text-faint);
    font-size: 12px;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    background: transparent;
  }
  footer {
    display: grid;
    gap: 14px;
    padding: 20px 24px 16px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .actions {
    display: grid;
    grid-template-columns: 1.6fr 1fr 1fr 1fr;
    gap: 10px;
  }
  .actions button,
  .tools button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }
  .copy {
    min-height: 50px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 16px;
    font-weight: 650;
  }
  .copy:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  /* Secondary actions in white: easy to see in both themes. */
  .light {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 42px;
    background: #ffffff;
    color: #1c1c1f;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.14);
  }
  .light:hover:not(:disabled) {
    background: #ececf0;
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .tool {
    min-height: 36px;
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--border);
  }
  .tool.active {
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: none;
  }
  .swatch {
    width: 26px;
    height: 26px;
    padding: 0;
    border-radius: 50%;
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.5);
  }
  .status {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link {
    padding: 0 0 0 6px;
    background: none;
    color: var(--accent-text);
    font-size: 12px;
  }
  .toast {
    position: absolute;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    padding: 8px 16px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow-soft);
    font-weight: 550;
    white-space: nowrap;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.45);
  }
  .dialog {
    width: min(420px, calc(100vw - 32px));
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
