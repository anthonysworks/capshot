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
    checkForUpdates,
    installUpdate,
    type UpdateStatus,
    type JobProgress,
    convertToGif,
    getConfig,
    isVideo,
    onConfigChanged,
    onFileDrop,
    onJobProgress,
    showInFolder,
    openFolder,
    openSetup,
    saveConfig,
  } from "./lib/api";
  import HotkeyInput from "./lib/HotkeyInput.svelte";
  import Logo from "./lib/Logo.svelte";
  import Notches from "./lib/Notches.svelte";
  import { type Theme, setTheme } from "./lib/theme";

  let config = $state<Config | null>(null);
  let saved = $state("");
  let info = $state<AppInfo | null>(null);
  let status = $state<{ kind: "ok" | "error"; text: string } | null>(null);
  let saving = $state(false);
  let conversions = $state<JobProgress[]>([]);

  const AFTER: { value: AfterCapture; title: string; text: string }[] = [
    {
      value: "actions",
      title: "Show actions",
      text: "Copy, save, edit, read its text or QR code. Saved only when you ask.",
    },
    { value: "editor", title: "Open the editor", text: "Annotate or trim right away." },
    {
      value: "preview",
      title: "Thumbnail",
      text: "Copied, with a thumbnail in the corner; click it to open the editor.",
    },
    { value: "nothing", title: "Silent", text: "Copied and saved, nothing on screen." },
  ];
  let dragging = $state(false);
  let update = $state<UpdateStatus | "checking" | null>(null);

  async function lookForUpdate() {
    update = "checking";
    try {
      update = await checkForUpdates();
    } catch (e) {
      update = { state: "failed", message: String(e) };
    }
  }

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
      c.appearance ??= { theme: "system" };
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

  const CATEGORIES = [
    { id: "general", label: "General", glyph: "\u{E713}" },
    { id: "shortcuts", label: "Shortcuts", glyph: "\u{E945}" },
    { id: "screenshots", label: "Screenshots", glyph: "\u{E722}" },
    { id: "recording", label: "Recording", glyph: "\u{E714}" },
    { id: "gif", label: "GIF", glyph: "\u{E8B9}" },
    { id: "keyboard", label: "Keyboard", glyph: "\u{E765}" },
  ] as const;
  type Category = (typeof CATEGORIES)[number]["id"];
  let category = $state<Category>("general");

  const THEMES: { value: Theme; title: string; text: string }[] = [
    { value: "system", title: "System", text: "Follows Windows." },
    { value: "dark", title: "Dark", text: "Near black, bright text." },
    { value: "light", title: "Light", text: "Light grey, black text." },
  ];

  /** The theme applies and is kept at once, like on a console. */
  async function chooseTheme(theme: Theme) {
    if (!config) return;
    setTheme(theme);
    config.appearance.theme = theme;
    const base = JSON.parse(saved) as Config;
    base.appearance = { theme };
    try {
      await saveConfig(base);
      saved = JSON.stringify(base);
    } catch (e) {
      status = { kind: "error", text: String(e) };
    }
  }

  const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
  const size = (bytes: number) =>
    bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
</script>


{#if config}
  <div class="settings">
    <nav class="rail" aria-label="Categories">
      <div class="brand">
        <Logo height={26} />
        {#if info}<span class="version">{info.version}</span>{/if}
      </div>
      {#each CATEGORIES as c (c.id)}
        <button
          type="button"
          class="category"
          class:active={category === c.id}
          aria-current={category === c.id ? "page" : undefined}
          onclick={() => (category = c.id)}><span class="glyph" aria-hidden="true">{c.glyph}</span>{c.label}</button
        >
      {/each}
      <button type="button" class="logs first" onclick={openSetup}>Run the setup again</button>
      <button type="button" class="logs" onclick={() => openFolder("logs")}>Open logs</button>
    </nav>

    <main>
      <div class="content">
        {#if category === "general"}
          <h1>General</h1>
          <section>
            <h2>Theme</h2>
            <div class="tiles" role="radiogroup" aria-label="Theme">
              {#each THEMES as t (t.value)}
                <label class="tile theme-{t.value}" class:active={config.appearance.theme === t.value}>
                  <input
                    type="radio"
                    name="theme"
                    value={t.value}
                    checked={config.appearance.theme === t.value}
                    onchange={() => chooseTheme(t.value)}
                  />
                  <span class="swatch" aria-hidden="true"><i></i><i></i></span>
                  <strong>{t.title}</strong>
                  <span>{t.text}</span>
                </label>
              {/each}
            </div>
            <p class="hint">Applies at once to every Capshot window and to the capture toolbar.</p>
          </section>
          <section>
            <h2>After a capture</h2>
            <div class="tiles" role="radiogroup" aria-label="After a capture">
              {#each AFTER as mode (mode.value)}
                <label class="tile" class:active={config.capture.after_capture === mode.value}>
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
            <h2>Updates</h2>
            <label class="row check">
              <input type="checkbox" bind:checked={config.auto_update} />
              <span>Download new versions in the background and offer to install them from the tray menu</span>
            </label>
            <div class="row">
              {#if update && update !== "checking" && update.state === "ready"}
                <button class="primary" onclick={() => installUpdate().catch((e) => (update = { state: "failed", message: String(e) }))}>
                  Restart to install {update.version}
                </button>
              {:else}
                <button onclick={lookForUpdate} disabled={update === "checking"}>
                  {update === "checking" ? "Checking…" : "Check for updates"}
                </button>
              {/if}
              {#if update && update !== "checking"}
                <span class="hint">
                  {#if update.state === "upToDate"}You have the latest version{info ? ` (${info.version})` : ""}.
                  {:else if update.state === "ready"}Version {update.version} is downloaded.
                  {:else}Could not check: {update.message}{/if}
                </span>
              {/if}
            </div>
          </section>
        {:else if category === "shortcuts"}
          <h1>Shortcuts</h1>
          <section>
            <div class="row">
              <span>Capture</span>
              <HotkeyInput label="Capture shortcut" bind:value={config.hotkeys.capture} />
            </div>
            <div class="row">
              <span>Record video</span>
              <HotkeyInput label="Record shortcut" bind:value={config.hotkeys.record} />
            </div>
            <p class="hint">
              To use Print Screen, turn off “Use the Print screen key to open screen capture” in Windows Settings ›
              Accessibility › Keyboard.
            </p>
          </section>
        {:else if category === "screenshots"}
          <h1>Screenshots</h1>
          <section>
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
        {:else if category === "recording"}
          <h1>Recording</h1>
          <p class="lead">
            Format, frame rate, countdown, sound and microphone can also be changed on the capture toolbar
            (Shift+Print Screen); the toolbar remembers your last choice.
          </p>
          <section>
            <label class="row">
              <span>Format</span>
              <select bind:value={config.recording.format}>
                <option value="mp4">MP4 video</option>
                <option value="gif">Animated GIF</option>
              </select>
            </label>
            <div class="row">
              <span>Frame rate</span>
              <div class="chips">
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
                <option value="av1">AV1 (smallest; RTX 40, RX 7000, Arc, otherwise HEVC)</option>
              </select>
            </label>
            <div class="row">
              <span>Quality</span>
              <!-- In percent; the setting is an encoder value (15 best – 35), lower is better. -->
              <Notches
                value={Math.round(((35 - config.recording.quality) / 20) * 80 + 20)}
                onchange={(pct) => config && (config.recording.quality = Math.round(35 - ((pct - 20) / 80) * 20))}
              />
            </div>
          </section>
          <section>
            <h2>Sound and cursor</h2>
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
              <span
                >Editable cursor: keep a copy without the pointer so the editor can hide, enlarge or replace it and
                mark clicks (uses twice the disk space; the 30 latest are kept)</span
              >
            </label>
          </section>
          <section>
            <h2>Folder</h2>
            <div class="row">
              <input class="path" bind:value={config.recording.output_dir} aria-label="Recordings folder" />
              <button type="button" onclick={() => openFolder("recordings")}>Open</button>
            </div>
          </section>
        {:else if category === "gif"}
          <h1>GIF</h1>
          <p class="lead">
            Turn on <strong>GIF</strong> in the recording toolbar (or press G) to record an animated GIF. These settings
            also apply to videos converted to GIF.
          </p>
          <section>
            <div class="row">
              <span>Frame rate</span>
              <div class="chips">
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
            <div class="row">
              <span>Quality</span>
              <Notches min={40} bind:value={config.gif.quality} />
            </div>
            <label class="row check">
              <input type="checkbox" bind:checked={config.gif.dither} />
              <span>Dithering (smoother gradients, larger files)</span>
            </label>
            <label class="row check">
              <input type="checkbox" bind:checked={config.gif.repeat} />
              <span>Loop forever</span>
            </label>
          </section>
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
                <button type="button" onclick={() => c.output && showInFolder(c.output)}>Show</button>
              {:else}
                <progress max="1" value={c.done}></progress>
                <span class="value">{Math.round(c.done * 100)} %</span>
              {/if}
            </div>
          {/each}
        {:else}
          <h1>Keyboard</h1>
          <section>
            <h2>Capture toolbar</h2>
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
          </section>
          <section>
            <h2>Image editor</h2>
            <dl>
              <dt>V</dt><dd>Select, move, resize</dd>
              <dt>A Q L R E</dt><dd>Arrow, curved arrow, line, rectangle, ellipse</dd>
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
          </section>
          <section>
            <h2>Video editor</h2>
            <dl>
              <dt>Space, arrows</dt><dd>Play / pause, step one frame (one second with Shift)</dd>
              <dt>S</dt><dd>Split at the playhead</dd>
              <dt>Delete</dt><dd>Remove the selected text or shape, else the selected part</dd>
              <dt>I / O</dt><dd>Start / end at the playhead</dd>
              <dt>V T A Q L R E</dt><dd>Select, text, arrow, curved arrow, line, rectangle, ellipse</dd>
              <dt>P H N B</dt><dd>Pencil, highlighter, numbered step, pixelate</dd>
              <dt>C</dt><dd>Crop the picture</dd>
              <dt>Ctrl+C / Ctrl+E</dt><dd>Copy the video (edited if needed), save</dd>
              <dt>Ctrl+Z / Ctrl+Y</dt><dd>Undo / redo</dd>
              <dt>Ctrl+wheel</dt><dd>Zoom the timeline</dd>
            </dl>
          </section>
        {/if}
      </div>

      <footer>
        {#if status}
          <span class="status {status.kind}" role="status">{status.text}</span>
        {:else if dirty}
          <span class="status">Unsaved changes</span>
        {/if}
        <div class="actions">
          <button type="button" onclick={revert} disabled={!dirty || saving}>Revert</button>
          <button type="button" class="primary save" onclick={save} disabled={!dirty || !fpsValid || saving}>Save</button>
        </div>
      </footer>
    </main>
  </div>
{:else if status}
  <p class="status error">{status.text}</p>
{/if}

<style>
  .settings {
    display: grid;
    grid-template-columns: 210px 1fr;
    height: 100vh;
  }

  /* Left rail: one category at a time. */
  .rail {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 22px 0;
    background: var(--surface);
    border-right: 1px solid var(--border);
    overflow-y: auto;
  }
  .brand {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 4px 24px 22px;
  }
  .brand .version {
    padding-bottom: 2px;
    color: var(--text-faint);
    font-size: 12px;
  }
  .category {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 0 12px;
    padding: 10px 14px;
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-size: 15px;
    text-align: left;
  }
  .category:not(:disabled):hover {
    background: var(--hover);
    color: var(--text);
  }
  .category.active {
    background: var(--accent);
    color: var(--on-accent);
  }
  .glyph {
    font-family: "Segoe Fluent Icons", "Segoe MDL2 Assets";
    font-size: 16px;
    font-weight: 400;
  }
  .logs {
    margin: 0 20px 8px;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-muted);
    font-weight: 400;
    font-size: 13px;
    text-align: left;
  }
  .logs.first {
    margin-top: auto;
  }
  .logs:not(:disabled):hover {
    color: var(--text);
    text-decoration: underline;
  }

  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 32px 40px 40px;
  }
  h1 {
    margin: 0 0 24px;
    font-size: 28px;
    font-weight: 650;
  }
  .lead {
    margin: -6px 0 14px;
    color: var(--text-muted);
  }
  section {
    margin-bottom: 20px;
    padding: 20px 24px;
    border-radius: var(--radius-lg);
    background: var(--raised);
    box-shadow: var(--shadow-soft);
  }
  h2 {
    margin: 0 0 14px;
    font-size: 15px;
    font-weight: 650;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 44px;
  }
  .row > span:first-child,
  .row.check > span {
    flex: 1;
  }
  .row select {
    min-width: 220px;
  }
  .path {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 13px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    justify-content: flex-end;
  }
  .chips input {
    width: 4.5rem;
  }
  .chip {
    min-width: 44px;
  }
  .chip.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  .value {
    width: 5rem;
    color: var(--text-muted);
    font-weight: 600;
  }

  /* Choice tiles, blue when chosen. */
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 10px;
    margin-bottom: 12px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 16px;
    border: 2px solid transparent;
    border-radius: var(--radius);
    background: var(--control);
    cursor: pointer;
  }
  .tile:hover {
    background: var(--hover);
  }
  .tile input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .tile strong {
    font-size: 15px;
  }
  .tile span {
    font-size: 13px;
    color: var(--text-muted);
  }
  .tile.active {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .tile.active::after {
    content: "\E73E";
    position: absolute;
    top: 8px;
    right: 8px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
    font-family: "Segoe Fluent Icons", "Segoe MDL2 Assets";
    font-size: 12px;
  }
  .swatch {
    display: flex;
    height: 34px;
    margin-bottom: 6px;
    border-radius: 6px;
    overflow: hidden;
    box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  .swatch i {
    flex: 1;
  }
  .theme-dark .swatch i:first-child,
  .theme-system .swatch i:first-child {
    background: #1c1c1f;
  }
  .theme-dark .swatch i:last-child {
    background: #141416;
  }
  .theme-light .swatch i:first-child {
    background: #ffffff;
  }
  .theme-light .swatch i:last-child,
  .theme-system .swatch i:last-child {
    background: #f2f2f5;
  }

  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 18px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 600;
    color: var(--accent-text);
  }
  dd {
    margin: 0;
  }
  .drop {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 22px;
    border: 2px dashed var(--border-strong);
    border-radius: var(--radius-lg);
    color: var(--text-muted);
  }
  .drop strong {
    color: var(--text);
    font-size: 15px;
  }
  .drop.dragging {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
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
    accent-color: var(--accent);
  }
  .hint {
    margin: 8px 0 0;
    color: var(--text-muted);
    font-size: 13px;
  }
  .invalid {
    border-color: var(--danger);
  }
  footer {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 40px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-left: auto;
  }
  .actions button {
    min-height: 40px;
    padding: 8px 20px;
  }
  .save {
    min-width: 120px;
  }
  .status {
    color: var(--text-muted);
    font-weight: 600;
  }
  .status.ok {
    color: var(--success);
  }
  .status.error {
    color: var(--danger);
  }
  @media (max-width: 620px) {
    .settings {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    .rail {
      flex-direction: row;
      flex-wrap: wrap;
      padding: 8px;
      border-right: none;
      border-bottom: 1px solid var(--border);
    }
    .brand,
    .logs {
      display: none;
    }
    .category {
      margin: 0;
      padding: 6px 10px;
      font-size: 13px;
    }
  }
</style>
