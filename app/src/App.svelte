<script lang="ts">
  import {
    type AfterCapture,
    type AppInfo,
    type Config,
    FPS_MAX,
    FPS_MIN,
    FPS_PRESETS,
    GIF_FPS_MAX,
    GIF_FPS_PRESETS,
    GIF_WIDTHS,
    appInfo,
    type JobProgress,
    convertToGif,
    getConfig,
    isVideo,
    onConfigChanged,
    onFileDrop,
    onJobProgress,
    showInFolder,
    openFolder,
    saveConfig,
  } from "./lib/api";
  import HotkeyInput from "./lib/HotkeyInput.svelte";

  let config = $state<Config | null>(null);
  let saved = $state("");
  let info = $state<AppInfo | null>(null);
  let status = $state<{ kind: "ok" | "error"; text: string } | null>(null);
  let saving = $state(false);
  let conversions = $state<JobProgress[]>([]);

  const AFTER: { value: AfterCapture; title: string; text: string }[] = [
    { value: "editor", title: "Open the editor", text: "Annotate or trim right away." },
    {
      value: "preview",
      title: "Quick",
      text: "Copied, with a thumbnail in the corner; click it to open the editor.",
    },
    { value: "nothing", title: "Silent", text: "Copied and saved, nothing on screen." },
  ];
  let dragging = $state(false);

  const dirty = $derived(config !== null && JSON.stringify(config) !== saved);
  const fpsValid = $derived(
    config !== null &&
      Number.isInteger(config.recording.fps) &&
      config.recording.fps >= FPS_MIN &&
      config.recording.fps <= FPS_MAX &&
      Number.isInteger(config.gif.fps) &&
      config.gif.fps >= 1 &&
      config.gif.fps <= GIF_FPS_MAX,
  );

  async function load() {
    try {
      const [c, i] = await Promise.all([getConfig(), appInfo()]);
      config = c;
      saved = JSON.stringify(c);
      info = i;
    } catch (e) {
      status = { kind: "error", text: String(e) };
    }
  }

  async function save() {
    if (!config || !fpsValid) return;
    saving = true;
    try {
      await saveConfig($state.snapshot(config));
      saved = JSON.stringify(config);
      status = { kind: "ok", text: "Settings saved." };
    } catch (e) {
      status = { kind: "error", text: String(e) };
    } finally {
      saving = false;
    }
  }

  function revert() {
    config = JSON.parse(saved);
    status = null;
  }

  load();

  // The overlay toolbar remembers the audio toggles: apply them without
  // touching unsaved edits.
  onConfigChanged((next) => {
    if (!config) return;
    const base = JSON.parse(saved) as Config;
    for (const key of ["format", "fps", "countdown_secs", "system_audio", "microphone"] as const) {
      (base.recording as Record<string, unknown>)[key] = next.recording[key];
      (config.recording as Record<string, unknown>)[key] = next.recording[key];
    }
    base.gif.fps = next.gif.fps;
    config.gif.fps = next.gif.fps;
    saved = JSON.stringify(base);
  });

  onJobProgress((p) => {
    const i = conversions.findIndex((c) => c.job === p.job);
    if (i >= 0) conversions[i] = p;
  });

  onFileDrop((paths, over) => {
    dragging = over;
    for (const path of paths ?? []) {
      if (!isVideo(path)) continue;
      convertToGif(path).then((job) => {
        if (!conversions.some((c) => c.job === job)) {
          conversions.push({ job, input: path, done: 0, output: null, bytes: null, error: null });
        }
      });
    }
  });

  const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
  const size = (bytes: number) =>
    bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
</script>

{#if config}
  <main>
    <header>
      <h1>Settings</h1>
      {#if info}<span class="version">Kadra {info.version}</span>{/if}
    </header>

    <section>
      <h2>After a capture</h2>
      <div class="modes" role="radiogroup" aria-label="After a capture">
        {#each AFTER as mode (mode.value)}
          <label class="mode" class:active={config.capture.after_capture === mode.value}>
            <input type="radio" name="after" value={mode.value} bind:group={config.capture.after_capture} />
            <strong>{mode.title}</strong>
            <span>{mode.text}</span>
          </label>
        {/each}
      </div>
      <label class="row check">
        <input type="checkbox" bind:checked={config.capture.copy_to_clipboard} />
        <span>Copy to the clipboard (videos are copied as files, ready to paste in a chat or a folder)</span>
      </label>
    </section>

    <section>
      <h2>Shortcuts</h2>
      <div class="row">
        <span>Capture</span>
        <HotkeyInput label="Capture shortcut" bind:value={config.hotkeys.capture} />
      </div>
      <div class="row">
        <span>Record video</span>
        <HotkeyInput label="Record shortcut" bind:value={config.hotkeys.record} />
      </div>
      <p class="hint">
        To use Print Screen, turn off “Use the Print screen key to open screen capture” in
        Windows Settings › Accessibility › Keyboard.
      </p>
    </section>

    <section>
      <h2>Screenshots</h2>
      <label class="row">
        <span>Format</span>
        <select bind:value={config.capture.format}>
          <option value="png">PNG (lossless)</option>
          <option value="jpeg">JPEG</option>
          <option value="webp">WebP</option>
        </select>
      </label>
      <label class="row">
        <span>Delay</span>
        <select bind:value={config.capture.delay_secs}>
          {#each [0, 1, 3, 5, 10] as s (s)}
            <option value={s}>{s === 0 ? "None" : `${s} s`}</option>
          {/each}
        </select>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.capture.save_to_disk} />
        <span>Save to folder</span>
      </label>
      <div class="row">
        <input class="path" bind:value={config.capture.output_dir} aria-label="Screenshots folder" />
        <button type="button" onclick={() => openFolder("screenshots")}>Open</button>
      </div>
    </section>

    <section>
      <h2>Recording</h2>
      <p class="hint">
        Format, frame rate, countdown, sound and microphone can also be changed on the capture toolbar
        (Shift+Print Screen); the toolbar remembers your last choice.
      </p>
      <label class="row">
        <span>Format</span>
        <select bind:value={config.recording.format}>
          <option value="mp4">MP4 video</option>
          <option value="gif">Animated GIF</option>
        </select>
      </label>
      <div class="row">
        <span>Frame rate</span>
        <div class="fps">
          {#each FPS_PRESETS as fps (fps)}
            <button
              type="button"
              class="chip"
              class:active={config.recording.fps === fps}
              onclick={() => config && (config.recording.fps = fps)}>{fps}</button
            >
          {/each}
          <input
            type="number"
            min={FPS_MIN}
            max={FPS_MAX}
            step="1"
            class:invalid={!fpsValid}
            bind:value={config.recording.fps}
            aria-label="Custom frame rate"
          />
        </div>
      </div>
      <label class="row">
        <span>Countdown</span>
        <select bind:value={config.recording.countdown_secs}>
          {#each [0, 3, 5, 10] as s (s)}
            <option value={s}>{s === 0 ? "None" : `${s} s`}</option>
          {/each}
        </select>
      </label>
      <label class="row">
        <span>Codec</span>
        <select bind:value={config.recording.codec}>
          <option value="h264">H.264 (best compatibility)</option>
          <option value="hevc">HEVC (smaller files)</option>
          <option value="av1">AV1 (smallest; RTX 40, RX 7000, Arc — otherwise HEVC)</option>
        </select>
      </label>
      <label class="row">
        <span>Quality</span>
        <input type="range" min="15" max="35" bind:value={config.recording.quality} />
        <span class="value">{config.recording.quality <= 20 ? "High" : config.recording.quality <= 26 ? "Balanced" : "Small"}</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.recording.capture_cursor} />
        <span>Show mouse cursor</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.recording.system_audio} />
        <span>Record system audio</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.recording.microphone} />
        <span>Record microphone</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.recording.editable_cursor} />
        <span>Editable cursor: keep a copy without the pointer so the editor can hide, enlarge or replace it and mark clicks (uses twice the disk space; the 30 latest are kept)</span>
      </label>
      <div class="row">
        <input class="path" bind:value={config.recording.output_dir} aria-label="Recordings folder" />
        <button type="button" onclick={() => openFolder("recordings")}>Open</button>
      </div>
    </section>

    <section>
      <h2>GIF</h2>
      <p class="hint">Turn on <strong>GIF</strong> in the recording toolbar (or press G) to record an animated GIF. These settings also apply to videos converted to GIF.</p>
      <div class="row">
        <span>Frame rate</span>
        <div class="fps">
          {#each GIF_FPS_PRESETS as fps (fps)}
            <button
              type="button"
              class="chip"
              class:active={config.gif.fps === fps}
              onclick={() => config && (config.gif.fps = fps)}>{fps}</button
            >
          {/each}
        </div>
      </div>
      <label class="row">
        <span>Maximum width</span>
        <select bind:value={config.gif.max_width}>
          {#each GIF_WIDTHS as w (w)}
            <option value={w}>{w === 0 ? "Original size" : `${w} px`}</option>
          {/each}
        </select>
      </label>
      <label class="row">
        <span>Quality</span>
        <input type="range" min="40" max="100" bind:value={config.gif.quality} />
        <span class="value">{config.gif.quality >= 85 ? "High" : config.gif.quality >= 65 ? "Balanced" : "Small"}</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.gif.dither} />
        <span>Dithering (smoother gradients, larger files)</span>
      </label>
      <label class="row check">
        <input type="checkbox" bind:checked={config.gif.repeat} />
        <span>Loop forever</span>
      </label>
      <div class="drop" class:dragging>
        <strong>Video to GIF</strong>
        <span>Drop a video file on this window to convert it with these settings. The GIF is saved next to the video.</span>
      </div>
      {#each conversions as c (c.job)}
        <div class="conversion">
          <span class="name" title={c.input}>{fileName(c.input)}</span>
          {#if c.error}
            <span class="status error">{c.error}</span>
          {:else if c.output}
            <span class="value">{size(c.bytes ?? 0)}</span>
            <button type="button" class="link" onclick={() => c.output && showInFolder(c.output)}>Show</button>
          {:else}
            <progress max="1" value={c.done}></progress>
            <span class="value">{Math.round(c.done * 100)} %</span>
          {/if}
        </div>
      {/each}
    </section>

    <section>
      <h2>Keyboard</h2>
      <details>
        <summary>Capture toolbar</summary>
        <dl>
          <dt>Tab / V</dt><dd>Switch between screenshot and recording</dd>
          <dt>R / F / S</dt><dd>Region, freeform, whole screen</dd>
          <dt>I</dt><dd>Colour picker (click copies #RRGGBB)</dd>
          <dt>C</dt><dd>Copy the colour under the cursor</dd>
          <dt>G</dt><dd>Record a GIF instead of a video</dd>
          <dt>A / M</dt><dd>System sound / microphone</dd>
          <dt>Click, wheel</dt><dd>On "fps" and the stopwatch: frame rate and countdown</dd>
          <dt>Arrows</dt><dd>Move the cursor by 1 px (10 px with Shift)</dd>
          <dt>Enter / Esc</dt><dd>Capture what is highlighted / cancel</dd>
        </dl>
      </details>
      <details>
        <summary>Image editor</summary>
        <dl>
          <dt>V</dt><dd>Select, move, resize</dd>
          <dt>A L R E</dt><dd>Arrow, line, rectangle, ellipse</dd>
          <dt>P H</dt><dd>Pencil (freehand), highlighter (yellow)</dd>
          <dt>T N</dt><dd>Text, numbered step</dd>
          <dt>S U B</dt><dd>Spotlight, blur, pixelate</dd>
          <dt>C F</dt><dd>Crop, frame (background, margin, corners)</dd>
          <dt>Shift</dt><dd>While drawing: straight angles, squares and circles</dd>
          <dt>Ctrl+Z / Ctrl+Y</dt><dd>Undo / redo</dd>
          <dt>Ctrl+D, Delete, arrows</dt><dd>Duplicate, delete, nudge the selection</dd>
          <dt>Ctrl+wheel, Space+drag</dt><dd>Zoom, pan (Ctrl+0 fits, Ctrl+1 is 100 %)</dd>
          <dt>Ctrl+C / Ctrl+S / Ctrl+Shift+S</dt><dd>Copy, save, save as a copy</dd>
        </dl>
      </details>
      <details>
        <summary>Video editor</summary>
        <dl>
          <dt>1 – 5</dt><dd>Panels: cut & speed, text & shapes, cursor & clicks, crop & format, export</dd>
          <dt>Space, arrows</dt><dd>Play / pause, step one frame (one second with Shift)</dd>
          <dt>S</dt><dd>Split at the playhead</dd>
          <dt>Delete</dt><dd>Remove the selected text or shape, else the selected part</dd>
          <dt>I / O</dt><dd>Start / end at the playhead</dd>
          <dt>V T A L R E P H N</dt><dd>Select, text, arrow, line, rectangle, ellipse, pencil, highlighter, step</dd>
          <dt>C</dt><dd>Crop the picture</dd>
          <dt>Ctrl+C / Ctrl+E</dt><dd>Copy the video (edited if needed), export MP4</dd>
          <dt>Ctrl+Z / Ctrl+Y</dt><dd>Undo / redo</dd>
        </dl>
      </details>
    </section>

    <footer>
      {#if status}
        <span class="status {status.kind}" role="status">{status.text}</span>
      {:else if info}
        <button type="button" class="link" onclick={() => openFolder("logs")}>Open logs</button>
      {/if}
      <div class="actions">
        <button type="button" onclick={revert} disabled={!dirty || saving}>Revert</button>
        <button type="button" class="primary" onclick={save} disabled={!dirty || !fpsValid || saving}>
          Save
        </button>
      </div>
    </footer>
  </main>
{:else if status}
  <p class="status error">{status.text}</p>
{/if}

<style>
  main {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 20px 24px 8px;
  }
  h1 {
    margin: 0;
    font-size: 1.5rem;
    font-weight: 600;
  }
  .version {
    color: var(--text-muted);
    font-size: 0.85rem;
  }
  section {
    margin: 8px 16px;
    padding: 12px 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 0.95rem;
    font-weight: 600;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 36px;
  }
  .row > span:first-child {
    flex: 1;
  }
  .row.check > span {
    flex: 1;
  }
  .path {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .fps {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    justify-content: flex-end;
  }
  .fps input {
    width: 4.5rem;
  }
  .chip.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  .value {
    width: 5rem;
    color: var(--text-muted);
  }
  .modes {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: 8px;
    margin-bottom: 8px;
  }
  .mode {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--control);
    cursor: pointer;
  }
  .mode input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .mode span {
    font-size: 12px;
    color: var(--text-muted);
  }
  .mode.active {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  details {
    margin: 4px 0;
  }
  summary {
    cursor: pointer;
    padding: 4px 0;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 16px;
    margin: 6px 0 10px 12px;
    font-size: 13px;
  }
  dt {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  dd {
    margin: 0;
  }
  .drop {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 12px;
    padding: 14px;
    border: 1px dashed var(--border, #8884);
    border-radius: 8px;
    font-size: 0.9em;
    opacity: 0.85;
    transition: background 0.15s, border-color 0.15s;
  }
  .drop.dragging {
    border-color: var(--accent, #4f7cff);
    background: color-mix(in srgb, var(--accent, #4f7cff) 12%, transparent);
    opacity: 1;
  }
  .conversion {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }
  .conversion .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conversion progress {
    width: 140px;
  }
  .hint {
    margin: 6px 0 0;
    color: var(--text-muted);
    font-size: 0.8rem;
  }
  .invalid {
    border-color: var(--danger);
  }
  footer {
    position: sticky;
    bottom: 0;
    margin-top: auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 24px;
    background: var(--bg);
    border-top: 1px solid var(--border);
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-left: auto;
  }
  .status.ok {
    color: var(--success);
  }
  .status.error {
    color: var(--danger);
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 0;
  }
</style>
