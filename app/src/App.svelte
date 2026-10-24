<script lang="ts">
  import {
    type AppInfo,
    type Config,
    FPS_MAX,
    FPS_MIN,
    FPS_PRESETS,
    appInfo,
    getConfig,
    openFolder,
    saveConfig,
  } from "./lib/api";
  import HotkeyInput from "./lib/HotkeyInput.svelte";

  let config = $state<Config | null>(null);
  let saved = $state("");
  let info = $state<AppInfo | null>(null);
  let status = $state<{ kind: "ok" | "error"; text: string } | null>(null);
  let saving = $state(false);

  const dirty = $derived(config !== null && JSON.stringify(config) !== saved);
  const fpsValid = $derived(
    config !== null &&
      Number.isInteger(config.recording.fps) &&
      config.recording.fps >= FPS_MIN &&
      config.recording.fps <= FPS_MAX,
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
</script>

{#if config}
  <main>
    <header>
      <h1>Settings</h1>
      {#if info}<span class="version">Kadra {info.version}</span>{/if}
    </header>

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
        <input type="checkbox" bind:checked={config.capture.copy_to_clipboard} />
        <span>Copy to clipboard</span>
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
        <span>Codec</span>
        <select bind:value={config.recording.codec}>
          <option value="h264">H.264 (best compatibility)</option>
          <option value="hevc">HEVC (smaller files)</option>
          <option value="av1">AV1 (smallest, recent GPUs)</option>
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
      <div class="row">
        <input class="path" bind:value={config.recording.output_dir} aria-label="Recordings folder" />
        <button type="button" onclick={() => openFolder("recordings")}>Open</button>
      </div>
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
