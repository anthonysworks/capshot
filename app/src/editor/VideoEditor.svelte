<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { type Config, type JobProgress, editorExport, getConfig, onJobProgress, showInFolder } from "../lib/api";
  import Icon from "./Icon.svelte";

  let { path }: { path: string } = $props();

  let video: HTMLVideoElement;
  let track: HTMLDivElement;
  let duration = $state(0);
  let time = $state(0);
  let start = $state(0);
  let end = $state(0);
  let playing = $state(false);
  let audio = $state(true);
  let error = $state("");
  let config = $state<Config | null>(null);
  let jobs = $state<(JobProgress & { kind: string })[]>([]);
  let dragging: "start" | "end" | "time" | null = null;

  /** Smallest kept range. */
  const MIN = 0.1;
  const source = $derived(convertFileSrc(path));
  const kept = $derived(Math.max(0, end - start));

  onMount(() => {
    getConfig().then((c) => (config = c));
    const unlisten = onJobProgress((p) => {
      const i = jobs.findIndex((j) => j.job === p.job);
      const old = jobs[i];
      if (old) jobs[i] = { ...p, kind: old.kind };
    });
    return () => {
      unlisten.then((f) => f());
    };
  });

  function loaded() {
    duration = video.duration;
    end = duration;
  }

  /** Playback stays inside the kept range. */
  function timeUpdate() {
    time = video.currentTime;
    if (playing && time >= end) {
      video.currentTime = start;
    }
  }

  function toggle() {
    if (video.paused) {
      if (video.currentTime >= end - 0.05 || video.currentTime < start) video.currentTime = start;
      video.play();
    } else {
      video.pause();
    }
  }

  function seek(t: number) {
    video.currentTime = Math.max(0, Math.min(duration, t));
    time = video.currentTime;
  }

  const setStart = () => (start = Math.min(time, end - MIN));
  const setEnd = () => (end = Math.max(time, start + MIN));

  function fromPointer(e: PointerEvent) {
    const r = track.getBoundingClientRect();
    return (Math.max(0, Math.min(r.width, e.clientX - r.left)) / r.width) * duration;
  }

  function trackDown(e: PointerEvent, what: "start" | "end" | "time") {
    e.stopPropagation();
    dragging = what;
    track.setPointerCapture(e.pointerId);
    if (what === "time") seek(fromPointer(e));
  }

  function trackMove(e: PointerEvent) {
    if (!dragging) return;
    const t = fromPointer(e);
    if (dragging === "start") {
      start = Math.min(t, end - MIN);
      seek(start);
    } else if (dragging === "end") {
      end = Math.max(t, start + MIN);
      seek(end);
    } else {
      seek(t);
    }
  }

  function format(t: number) {
    const m = Math.floor(t / 60);
    const s = t - m * 60;
    return `${m}:${s.toFixed(2).padStart(5, "0")}`;
  }

  const percent = (t: number) => (duration ? (t / duration) * 100 : 0);
  /** The whole video: no range to send. */
  const whole = $derived(start <= 0.01 && end >= duration - 0.01);

  async function exportAs(kind: "mp4" | "gif") {
    if (!config) return;
    try {
      const job = await editorExport(
        path,
        kind === "mp4"
          ? { kind, start, end: whole ? null : end, audio }
          : { kind, gif: $state.snapshot(config.gif), range: whole ? null : [start, end] },
      );
      jobs.unshift({ job, kind, input: path, done: 0, output: null, bytes: null, error: null });
    } catch (e) {
      error = String(e);
    }
  }

  function keyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? 1 : 1 / 30;
    switch (e.key) {
      case " ":
        e.preventDefault();
        toggle();
        break;
      case "ArrowLeft":
        seek(time - step);
        break;
      case "ArrowRight":
        seek(time + step);
        break;
      case "i":
      case "I":
        setStart();
        break;
      case "o":
      case "O":
        setEnd();
        break;
    }
  }

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
      onclick={toggle}
    ></video>
    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>

  <footer>
    <div class="timeline">
      <button type="button" class="icon" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onclick={toggle}>
        <Icon name={playing ? "pause" : "play"} />
      </button>
      <span class="time">{format(time)}</span>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="track"
        bind:this={track}
        onpointerdown={(e) => trackDown(e, "time")}
        onpointermove={trackMove}
        onpointerup={() => (dragging = null)}
      >
        <div class="kept" style="left:{percent(start)}%;width:{percent(end) - percent(start)}%"></div>
        <div class="playhead" style="left:{percent(time)}%"></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="handle"
          style="left:{percent(start)}%"
          title="Start (I)"
          onpointerdown={(e) => trackDown(e, "start")}
        ></div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="handle" style="left:{percent(end)}%" title="End (O)" onpointerdown={(e) => trackDown(e, "end")}></div>
      </div>
      <span class="time">{format(duration)}</span>
    </div>

    <div class="actions">
      <button type="button" onclick={setStart} title="Start at the current position (I)">Start here</button>
      <button type="button" onclick={setEnd} title="End at the current position (O)">End here</button>
      <span class="range">{format(start)} → {format(end)} · {format(kept)}</span>
      <label class="check"><input type="checkbox" bind:checked={audio} /> Keep sound</label>
      <div class="spacer"></div>
      <button type="button" onclick={() => exportAs("gif")} disabled={!config || !duration}>Export GIF</button>
      <button type="button" class="primary" onclick={() => exportAs("mp4")} disabled={!duration}>Export MP4</button>
    </div>

    {#each jobs as j (j.job)}
      <div class="job">
        <span class="name">{j.kind.toUpperCase()}</span>
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
    display: grid;
    place-items: center;
    background: #000;
  }
  video {
    max-width: 100%;
    max-height: 100%;
  }
  .error {
    position: absolute;
    bottom: 12px;
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
  .job {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .time,
  .range {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  .track {
    position: relative;
    flex: 1;
    height: 34px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    cursor: pointer;
    touch-action: none;
  }
  .kept {
    position: absolute;
    top: 0;
    bottom: 0;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 35%, transparent);
    border: 2px solid var(--accent);
  }
  .playhead {
    position: absolute;
    top: -4px;
    bottom: -4px;
    width: 2px;
    margin-left: -1px;
    background: var(--text);
    pointer-events: none;
  }
  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 12px;
    margin-left: -6px;
    border-radius: 4px;
    background: var(--accent);
    cursor: ew-resize;
  }
  button.icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
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
