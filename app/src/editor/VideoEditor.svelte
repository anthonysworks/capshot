<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import {
    type Config,
    type JobProgress,
    type VideoEdit,
    GIF_FPS_PRESETS,
    GIF_WIDTHS,
    editorExport,
    getConfig,
    onJobProgress,
    showInFolder,
  } from "../lib/api";
  import Icon from "./Icon.svelte";
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

  const SPEEDS = [0.5, 1, 1.5, 2, 3];
  const QUALITIES = [
    { value: 18, label: "High" },
    { value: 23, label: "Balanced" },
    { value: 28, label: "Small" },
  ];
  const FILMSTRIP = 12;

  let video: HTMLVideoElement;
  let track: HTMLDivElement;
  let duration = $state(0);
  let time = $state(0);
  let playing = $state(false);
  let segments = $state<Segment[]>([]);
  let selected = $state(0);
  let speed = $state(1);
  let audio = $state(true);
  let quality = $state(18);
  let crop = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  let cropping = $state(false);
  let cropDraft = $state<{ x: number; y: number; w: number; h: number } | null>(null);
  let frames = $state<string[]>([]);
  let error = $state("");
  let config = $state<Config | null>(null);
  let gifFps = $state(15);
  let gifWidth = $state(0);
  let jobs = $state<(JobProgress & { kind: string })[]>([]);
  let past = $state<string[]>([]);
  let future = $state<string[]>([]);
  /** Displayed video area inside the player, in CSS pixels. */
  let box = $state({ left: 0, top: 0, width: 0, height: 0, scale: 1 });
  let dragging: "time" | null = null;
  let cropStart: { x: number; y: number } | null = null;

  const source = $derived(convertFileSrc(path));
  const output = $derived(keptLength(segments) / speed);
  const soundLost = $derived(speed !== 1);

  onMount(() => {
    getConfig().then((c) => {
      config = c;
      gifFps = c.gif.fps;
      gifWidth = c.gif.max_width;
    });
    const unlisten = onJobProgress((p) => {
      const i = jobs.findIndex((j) => j.job === p.job);
      const old = jobs[i];
      if (old) jobs[i] = { ...p, kind: old.kind };
    });
    const observer = new ResizeObserver(measure);
    observer.observe(video);
    return () => {
      observer.disconnect();
      unlisten.then((f) => f());
    };
  });

  function loaded() {
    duration = video.duration;
    segments = whole(duration);
    measure();
    filmstrip();
  }

  /** Where the picture is drawn inside the element (letterboxed). */
  function measure() {
    if (!video?.videoWidth) return;
    const r = video.getBoundingClientRect();
    const scale = Math.min(r.width / video.videoWidth, r.height / video.videoHeight);
    const width = video.videoWidth * scale;
    const height = video.videoHeight * scale;
    box = { left: (r.width - width) / 2, top: (r.height - height) / 2, width, height, scale };
  }

  /** Thumbnails along the timeline, from a second, hidden player. */
  async function filmstrip() {
    const probe = document.createElement("video");
    probe.src = source;
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

  // ── Playback ───────────────────────────────────────────────────────────

  /** Playback skips removed parts and loops over the kept ones. */
  function timeUpdate() {
    time = video.currentTime;
    if (!playing) return;
    const next = nextKept(segments, time);
    if (next === null) {
      video.currentTime = keptRanges(segments)[0]?.[0] ?? 0;
    } else if (next > time + 0.01) {
      video.currentTime = next;
    }
  }

  function togglePlay() {
    if (video.paused) {
      if (nextKept(segments, video.currentTime) === null) {
        video.currentTime = keptRanges(segments)[0]?.[0] ?? 0;
      }
      video.playbackRate = speed;
      video.play();
    } else {
      video.pause();
    }
  }

  function seek(t: number) {
    video.currentTime = Math.max(0, Math.min(duration, t));
    time = video.currentTime;
    selected = indexAt(segments, time);
  }

  $effect(() => {
    if (video) video.playbackRate = speed;
  });

  // ── Editing ────────────────────────────────────────────────────────────

  const snapshot = () => JSON.stringify({ segments: $state.snapshot(segments), crop: $state.snapshot(crop) });

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
  }

  const splitHere = () => commit(() => (segments = split(segments, time)));
  const removeSelected = () => commit(() => (segments = toggle(segments, selected)));
  const startHere = () => commit(() => (segments = trimAt(segments, time, true)));
  const endHere = () => commit(() => (segments = trimAt(segments, time, false)));
  const reset = () =>
    commit(() => {
      segments = whole(duration);
      crop = null;
    });

  function fromPointer(e: PointerEvent) {
    const r = track.getBoundingClientRect();
    return (Math.max(0, Math.min(r.width, e.clientX - r.left)) / r.width) * duration;
  }

  function trackDown(e: PointerEvent) {
    dragging = "time";
    track.setPointerCapture(e.pointerId);
    seek(fromPointer(e));
  }

  function trackMove(e: PointerEvent) {
    if (dragging) seek(fromPointer(e));
  }

  // ── Crop ───────────────────────────────────────────────────────────────

  function toVideo(e: PointerEvent) {
    const r = video.getBoundingClientRect();
    const x = (e.clientX - r.left - box.left) / box.scale;
    const y = (e.clientY - r.top - box.top) / box.scale;
    return {
      x: Math.max(0, Math.min(video.videoWidth, x)),
      y: Math.max(0, Math.min(video.videoHeight, y)),
    };
  }

  function cropDown(e: PointerEvent) {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    cropStart = toVideo(e);
    cropDraft = { ...cropStart, w: 0, h: 0 };
  }

  function cropMove(e: PointerEvent) {
    if (!cropStart) return;
    const p = toVideo(e);
    cropDraft = {
      x: Math.min(p.x, cropStart.x),
      y: Math.min(p.y, cropStart.y),
      w: Math.abs(p.x - cropStart.x),
      h: Math.abs(p.y - cropStart.y),
    };
  }

  function cropUp() {
    const area = cropDraft;
    cropStart = null;
    cropDraft = null;
    if (area && area.w > 8 && area.h > 8) {
      commit(() => (crop = { x: Math.round(area.x), y: Math.round(area.y), w: Math.round(area.w), h: Math.round(area.h) }));
      cropping = false;
    }
  }

  /** Crop rectangle in CSS pixels over the player. */
  const cropStyle = (c: { x: number; y: number; w: number; h: number }) =>
    `left:${box.left + c.x * box.scale}px;top:${box.top + c.y * box.scale}px;width:${c.w * box.scale}px;height:${c.h * box.scale}px`;

  // ── Export ─────────────────────────────────────────────────────────────

  function edit(): VideoEdit {
    return {
      segments: keptRanges(segments),
      speed,
      crop: crop ? [crop.x, crop.y, crop.w, crop.h] : null,
    };
  }

  async function exportAs(kind: "mp4" | "gif") {
    if (!config || !keptRanges(segments).length) return;
    try {
      const job = await editorExport(
        path,
        kind === "mp4"
          ? { kind, edit: edit(), audio: audio && !soundLost, quality }
          : { kind, gif: { ...$state.snapshot(config.gif), fps: gifFps, max_width: gifWidth }, edit: edit() },
      );
      jobs.unshift({ job, kind, input: path, done: 0, output: null, bytes: null, error: null });
    } catch (e) {
      error = String(e);
    }
  }

  function keyDown(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === "SELECT") return;
    const ctrl = e.ctrlKey || e.metaKey;
    const step = e.shiftKey ? 1 : 1 / 30;
    const key = e.key.toLowerCase();
    if (ctrl && key === "z") {
      e.preventDefault();
      if (e.shiftKey) restore(future, past);
      else restore(past, future);
      return;
    }
    if (ctrl && key === "y") {
      e.preventDefault();
      restore(future, past);
      return;
    }
    if (ctrl) return;
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
        removeSelected();
        break;
      case "i":
        startHere();
        break;
      case "o":
        endHere();
        break;
      case "c":
        cropping = !cropping;
        break;
      case "escape":
        cropping = false;
        break;
    }
  }

  const percent = (t: number) => (duration ? (t / duration) * 100 : 0);
  const size = (bytes: number) =>
    bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
</script>

<svelte:window onkeydown={keyDown} />

<div class="editor">
  <div class="player">
    <!-- svelte-ignore a11y_media_has_caption -->
    <video
      bind:this={video}
      src={source}
      preload="auto"
      onloadedmetadata={loaded}
      ontimeupdate={timeUpdate}
      onplay={() => (playing = true)}
      onpause={() => (playing = false)}
      onerror={() => (error = "This video cannot be played here, but it can still be exported.")}
      onclick={togglePlay}
    ></video>
    {#if crop && !cropping}
      <div class="crop-frame" style={cropStyle(crop)}></div>
    {/if}
    {#if cropping}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="crop-layer" onpointerdown={cropDown} onpointermove={cropMove} onpointerup={cropUp}>
        {#if cropDraft ?? crop}
          <div class="crop-frame active" style={cropStyle((cropDraft ?? crop)!)}></div>
        {/if}
        <div class="hint">Drag over the video to keep only that area · Esc to leave</div>
      </div>
    {/if}
    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>

  <footer>
    <div class="timeline">
      <button type="button" class="icon" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onclick={togglePlay}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <span class="time">{format(time)}</span>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="track" bind:this={track} onpointerdown={trackDown} onpointermove={trackMove} onpointerup={() => (dragging = null)}>
        <div class="filmstrip">
          {#each frames as src, i (i)}
            <img {src} alt="" />
          {/each}
        </div>
        {#each segments as s, i (s.start)}
          <div
            class="segment"
            class:removed={!s.kept}
            class:selected={selected === i}
            style="left:{percent(s.start)}%;width:{percent(s.end) - percent(s.start)}%"
          ></div>
        {/each}
        <div class="playhead" style="left:{percent(time)}%"></div>
      </div>
      <span class="time">{format(duration)}</span>
    </div>

    <div class="actions">
      <button type="button" onclick={splitHere} title="Split at the playhead (S)"><Icon name="split" /> Split</button>
      <button type="button" onclick={removeSelected} title="Remove or restore the selected part (Delete)"
        ><Icon name="trash" /> {segments[selected]?.kept === false ? "Restore" : "Remove"}</button
      >
      <button type="button" onclick={startHere} title="Remove everything before the playhead (I)">Start here</button>
      <button type="button" onclick={endHere} title="Remove everything after the playhead (O)">End here</button>
      <button type="button" class:active={cropping} onclick={() => (cropping = !cropping)} title="Keep only part of the picture (C)"
        ><Icon name="crop" /> Crop</button
      >
      {#if crop}
        <button type="button" class="link" onclick={() => commit(() => (crop = null))}>Reset crop</button>
      {/if}
      <button type="button" class="icon" title="Undo (Ctrl+Z)" aria-label="Undo" disabled={!past.length} onclick={() => restore(past, future)}
        ><Icon name="undo" /></button
      >
      <button type="button" class="icon" title="Redo (Ctrl+Y)" aria-label="Redo" disabled={!future.length} onclick={() => restore(future, past)}
        ><Icon name="redo" /></button
      >
      <button type="button" class="link" onclick={reset}>Reset all</button>
      <div class="spacer"></div>
      <span class="summary">Result: {format(output)}{crop ? ` · ${crop.w}×${crop.h}` : ""}</span>
    </div>

    <div class="export">
      <label>Speed
        <select bind:value={speed}>
          {#each SPEEDS as s (s)}<option value={s}>{s}×</option>{/each}
        </select>
      </label>
      <div class="box">
        <strong>MP4</strong>
        <label>Quality
          <select bind:value={quality}>
            {#each QUALITIES as q (q.value)}<option value={q.value}>{q.label}</option>{/each}
          </select>
        </label>
        <label class="check" title={soundLost ? "Sound is removed when the speed changes" : ""}
          ><input type="checkbox" bind:checked={audio} disabled={soundLost} /> Keep sound{soundLost ? " (1× only)" : ""}</label
        >
        <button type="button" class="primary" onclick={() => exportAs("mp4")} disabled={!duration}>Export MP4</button>
      </div>
      <div class="box">
        <strong>GIF</strong>
        <label>FPS
          <select bind:value={gifFps}>
            {#each GIF_FPS_PRESETS as f (f)}<option value={f}>{f}</option>{/each}
          </select>
        </label>
        <label>Width
          <select bind:value={gifWidth}>
            {#each GIF_WIDTHS as w (w)}<option value={w}>{w === 0 ? "Original" : `${w} px`}</option>{/each}
          </select>
        </label>
        <button type="button" onclick={() => exportAs("gif")} disabled={!config || !duration}>Export GIF</button>
      </div>
    </div>

    {#each jobs as j (j.job)}
      <div class="job">
        <span class="kind">{j.kind.toUpperCase()}</span>
        {#if j.error}
          <span class="failed">{j.error}</span>
        {:else if j.output}
          <span class="name" title={j.output}>{name(j.output)}</span>
          <span class="muted">{size(j.bytes ?? 0)}</span>
          <button type="button" class="icon" title="Show in folder" aria-label="Show in folder" onclick={() => j.output && showInFolder(j.output)}
            ><Icon name="folder" /></button
          >
        {:else}
          <progress max="1" value={j.done}></progress>
          <span class="muted">{Math.round(j.done * 100)} %</span>
        {/if}
      </div>
    {/each}
  </footer>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .player {
    position: relative;
    flex: 1;
    min-height: 0;
    background: #000;
  }
  video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .crop-layer {
    position: absolute;
    inset: 0;
    cursor: crosshair;
    background: rgb(0 0 0 / 0.35);
  }
  .crop-frame {
    position: absolute;
    border: 2px solid #fff;
    box-shadow: 0 0 0 9999px rgb(0 0 0 / 0.5);
    pointer-events: none;
  }
  .crop-frame:not(.active) {
    box-shadow: 0 0 0 9999px rgb(0 0 0 / 0.6);
    border-style: dashed;
  }
  .hint {
    position: absolute;
    top: 12px;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 12px;
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-muted);
    font-size: 13px;
  }
  .error {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    margin: 0;
    padding: 6px 12px;
    border-radius: 6px;
    background: var(--surface);
    color: var(--danger);
  }
  footer {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .timeline,
  .actions,
  .export,
  .box,
  .job {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .box {
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .export label,
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .time,
  .summary {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  .track {
    position: relative;
    flex: 1;
    min-width: 200px;
    height: 52px;
    border-radius: 6px;
    overflow: hidden;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    cursor: pointer;
    touch-action: none;
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
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  button.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  button.icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
  }
  button.link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 0 4px;
  }
  .spacer {
    flex: 1;
  }
  .job .name {
    max-width: 50%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .job .kind {
    font-weight: 600;
    min-width: 36px;
  }
  .job progress {
    flex: 1;
  }
  .muted {
    color: var(--text-muted);
  }
  .failed {
    color: var(--danger);
  }
</style>
