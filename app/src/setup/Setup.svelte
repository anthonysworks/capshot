<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
    type AfterCapture,
    type Config,
    getConfig,
    openKeyboardSettings,
    pickFolder,
    printScreenTaken,
    releasePrintScreen,
    saveConfig,
  } from "../lib/api";
  import HotkeyInput from "../lib/HotkeyInput.svelte";
  import Logo from "../lib/Logo.svelte";
  import { setTheme } from "../lib/theme";

  type Step = "welcome" | "shortcuts" | "folders" | "behaviour" | "done";
  const STEPS: { id: Step; label: string }[] = [
    { id: "welcome", label: "Welcome" },
    { id: "shortcuts", label: "Shortcuts" },
    { id: "folders", label: "Folders" },
    { id: "behaviour", label: "Preferences" },
    { id: "done", label: "Ready" },
  ];

  /** Keys offered besides a custom one. Print Screen is the natural choice,
   * but Windows keeps it for its own screen capture unless told otherwise. */
  const CAPTURE_KEYS = ["PrintScreen", "Ctrl+Shift+S", "Ctrl+Alt+S"];
  const RECORD_KEYS = ["Shift+PrintScreen", "Ctrl+Shift+R", "Ctrl+Alt+R"];
  const label = (key: string) => key.replace("PrintScreen", "Print Screen");

  const AFTER: { value: AfterCapture; title: string; text: string }[] = [
    { value: "actions", title: "Show actions", text: "Copy, save, edit… under the capture." },
    { value: "editor", title: "Open the editor", text: "Straight into the editor." },
    { value: "preview", title: "Copy and preview", text: "A thumbnail confirms it." },
    { value: "nothing", title: "Copy only", text: "Nothing on screen." },
  ];

  let config = $state<Config | null>(null);
  let step = $state<Step>("welcome");
  let customCapture = $state(false);
  let customRecord = $state(false);
  /** Windows opens its own capture on Print Screen. */
  let taken = $state(false);
  let released = $state(false);
  let error = $state("");
  let saving = $state(false);

  const index = $derived(STEPS.findIndex((s) => s.id === step));
  const usesPrintScreen = $derived(
    !!config && (config.hotkeys.capture.includes("PrintScreen") || config.hotkeys.record.includes("PrintScreen")),
  );

  onMount(() => {
    getConfig().then((c) => (config = c));
    // Windows' own setting may have been changed since: ask again whenever
    // the window comes back to the front.
    void checkPrintScreen();
    window.addEventListener("focus", checkPrintScreen);
    return () => window.removeEventListener("focus", checkPrintScreen);
  });

  async function checkPrintScreen() {
    taken = await printScreenTaken().catch(() => false);
  }

  async function freePrintScreen() {
    error = "";
    try {
      await releasePrintScreen();
      released = true;
      await checkPrintScreen();
    } catch (e) {
      error = String(e);
    }
  }

  /** Other keys, for people who want to keep Windows' capture. */
  function useOtherKeys() {
    if (!config) return;
    config.hotkeys.capture = "Ctrl+Shift+S";
    config.hotkeys.record = "Ctrl+Shift+R";
    customCapture = false;
    customRecord = false;
  }

  async function folder(kind: "capture" | "recording") {
    if (!config) return;
    const current = config[kind].output_dir;
    const chosen = await pickFolder(kind === "capture" ? "Save screenshots in" : "Save recordings in", current);
    if (chosen) config[kind].output_dir = chosen;
  }

  async function finish(skip = false) {
    if (!config) return;
    saving = true;
    error = "";
    try {
      const base = await getConfig();
      const next: Config = skip ? { ...base, setup_done: true } : { ...config, setup_done: true };
      await saveConfig(next);
      setTheme(next.appearance.theme);
      if (skip) await getCurrentWindow().close();
      else step = "done";
    } catch (e) {
      error = String(e);
      if (!skip) step = "shortcuts";
    } finally {
      saving = false;
    }
  }

  function next() {
    const at = STEPS[index + 1];
    if (!at) return;
    if (at.id === "done") void finish();
    else step = at.id;
  }
  const back = () => {
    const at = STEPS[index - 1];
    if (at) step = at.id;
  };
</script>

{#if config}
  <div class="setup">
    <header>
      <Logo height={34} />
      <ol aria-label="Steps">
        {#each STEPS as s, i (s.id)}
          <li class:active={s.id === step} class:past={i < index}><span>{i + 1}</span>{s.label}</li>
        {/each}
      </ol>
    </header>

    <main>
      {#if step === "welcome"}
        <h1>Welcome to Capshot</h1>
        <p class="lead">
          Fast, free screen capture and recording. A few questions will set it up the way you like; everything can be
          changed later in the settings.
        </p>
        <ul class="points">
          <li><strong>Capture</strong> any area, window or screen with one key.</li>
          <li><strong>Record</strong> videos and GIFs, with sound if you want.</li>
          <li><strong>Edit</strong> in a built-in editor: annotations, zoom, backgrounds, device frames.</li>
        </ul>
      {:else if step === "shortcuts"}
        <h1>Shortcuts</h1>
        <p class="lead">Pick the keys that start a screenshot and a recording.</p>

        <h2>Screenshot</h2>
        <div class="keys" role="radiogroup" aria-label="Screenshot shortcut">
          {#each CAPTURE_KEYS as key (key)}
            <button
              type="button"
              role="radio"
              aria-checked={!customCapture && config.hotkeys.capture === key}
              class:on={!customCapture && config.hotkeys.capture === key}
              onclick={() => {
                customCapture = false;
                config!.hotkeys.capture = key;
              }}>{label(key)}</button
            >
          {/each}
          <button type="button" role="radio" aria-checked={customCapture} class:on={customCapture} onclick={() => (customCapture = true)}>Other…</button>
        </div>
        {#if customCapture}<HotkeyInput label="Screenshot shortcut" bind:value={config.hotkeys.capture} />{/if}

        <h2>Recording</h2>
        <div class="keys" role="radiogroup" aria-label="Recording shortcut">
          {#each RECORD_KEYS as key (key)}
            <button
              type="button"
              role="radio"
              aria-checked={!customRecord && config.hotkeys.record === key}
              class:on={!customRecord && config.hotkeys.record === key}
              onclick={() => {
                customRecord = false;
                config!.hotkeys.record = key;
              }}>{label(key)}</button
            >
          {/each}
          <button type="button" role="radio" aria-checked={customRecord} class:on={customRecord} onclick={() => (customRecord = true)}>Other…</button>
        </div>
        {#if customRecord}<HotkeyInput label="Recording shortcut" bind:value={config.hotkeys.record} />{/if}

        {#if usesPrintScreen && taken}
          <div class="card warn" role="note">
            <h3>Windows keeps the Print Screen key</h3>
            <p>
              By default Windows 11 opens its own Snipping Tool when you press Print Screen, so both programs would
              start. To use Print Screen with Capshot, free it:
            </p>
            <ol>
              <li>Open <strong>Settings › Accessibility › Keyboard</strong>.</li>
              <li>Turn off <strong>Use the Print screen button to open screen capture</strong>.</li>
              <li>Come back here: this page notices the change.</li>
            </ol>
            <div class="buttons">
              <button type="button" class="primary" onclick={freePrintScreen}>Turn it off for me</button>
              <button type="button" onclick={openKeyboardSettings}>Open Windows settings</button>
              <button type="button" onclick={useOtherKeys}>Use Ctrl+Shift+S / R instead</button>
            </div>
            <p class="hint">
              “Turn it off for me” only changes this one option for your account; you can turn it back on in the same
              Windows page. Windows may ask you to sign out and in again before it takes effect.
            </p>
          </div>
        {:else if usesPrintScreen && released}
          <div class="card ok" role="status">
            <h3>Print Screen is free</h3>
            <p>Windows no longer opens its own capture on that key.</p>
          </div>
        {:else if usesPrintScreen}
          <div class="card ok" role="status">
            <h3>Print Screen is available</h3>
            <p>Windows is not using it for its own capture.</p>
          </div>
        {/if}
      {:else if step === "folders"}
        <h1>Where to save</h1>
        <p class="lead">Captures are saved here (you can still pick another place each time).</p>
        <h2>Screenshots</h2>
        <div class="path">
          <input bind:value={config.capture.output_dir} aria-label="Screenshots folder" />
          <button type="button" onclick={() => folder("capture")}>Browse…</button>
        </div>
        <h2>Recordings</h2>
        <div class="path">
          <input bind:value={config.recording.output_dir} aria-label="Recordings folder" />
          <button type="button" onclick={() => folder("recording")}>Browse…</button>
        </div>
      {:else if step === "behaviour"}
        <h1>Preferences</h1>
        <h2>Theme</h2>
        <div class="keys" role="radiogroup" aria-label="Theme">
          {#each [["system", "System"], ["dark", "Dark"], ["light", "Light"]] as const as [value, text] (value)}
            <button
              type="button"
              role="radio"
              aria-checked={config.appearance.theme === value}
              class:on={config.appearance.theme === value}
              onclick={() => {
                config!.appearance.theme = value;
                setTheme(value);
              }}>{text}</button
            >
          {/each}
        </div>
        <h2>After a capture</h2>
        <div class="tiles" role="radiogroup" aria-label="After a capture">
          {#each AFTER as mode (mode.value)}
            <label class="tile" class:on={config.capture.after_capture === mode.value}>
              <input type="radio" name="after" value={mode.value} bind:group={config.capture.after_capture} />
              <strong>{mode.title}</strong>
              <span>{mode.text}</span>
            </label>
          {/each}
        </div>
        <label class="check"><input type="checkbox" bind:checked={config.capture.copy_to_clipboard} /> Copy captures to the clipboard</label>
        <label class="check"><input type="checkbox" bind:checked={config.recording.system_audio} /> Record the computer's sound with videos</label>
        <label class="check"><input type="checkbox" bind:checked={config.recording.microphone} /> Record the microphone too</label>
        <label class="check"><input type="checkbox" bind:checked={config.startup} /> Start Capshot when I sign in to Windows (it waits in the tray)</label>
      {:else}
        <h1>You're all set</h1>
        <p class="lead">Capshot is running in the tray, next to the clock.</p>
        <div class="card">
          <dl>
            <dt>{label(config.hotkeys.capture)}</dt>
            <dd>Take a screenshot</dd>
            <dt>{label(config.hotkeys.record)}</dt>
            <dd>Record a video or a GIF</dd>
          </dl>
        </div>
        <p class="hint">
          Right-click the tray icon for the settings, to run this setup again, or to quit. Shortcuts, folders and
          everything else can be changed there.
        </p>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    </main>

    <footer>
      {#if step === "welcome"}
        <button type="button" class="link" onclick={() => finish(true)} disabled={saving}>Skip setup</button>
      {:else if step !== "done"}
        <button type="button" onclick={back}>Back</button>
      {/if}
      <span class="grow"></span>
      {#if step === "done"}
        <button type="button" class="primary" onclick={() => getCurrentWindow().close()}>Start using Capshot</button>
      {:else}
        <button type="button" class="primary" onclick={next} disabled={saving}>{step === "behaviour" ? "Finish" : "Continue"}</button>
      {/if}
    </footer>
  </div>
{/if}

<style>
  .setup {
    display: grid;
    grid-template-rows: auto 1fr auto;
    height: 100vh;
    background: var(--bg);
    color: var(--text);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
    padding: 18px 28px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  ol[aria-label="Steps"] {
    display: flex;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 13px;
    color: var(--text-faint);
  }
  ol[aria-label="Steps"] li {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4px 10px 4px 5px;
    border-radius: 999px;
  }
  ol[aria-label="Steps"] li span {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--control);
    font-size: 11px;
    font-weight: 700;
  }
  ol[aria-label="Steps"] li.active {
    color: var(--text);
    font-weight: 650;
  }
  ol[aria-label="Steps"] li.active span {
    background: var(--accent);
    color: var(--on-accent);
  }
  ol[aria-label="Steps"] li.past span {
    background: color-mix(in srgb, var(--accent) 25%, var(--control));
    color: var(--accent-text);
  }
  main {
    overflow-y: auto;
    padding: 30px 40px 20px;
    max-width: 680px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
  }
  h1 {
    margin: 0 0 8px;
    font-family: var(--font-display);
    font-size: 26px;
    font-weight: 700;
  }
  h2 {
    margin: 24px 0 10px;
    font-size: 13px;
    font-weight: 650;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .lead {
    margin: 0 0 8px;
    font-size: 15px;
    color: var(--text-muted);
  }
  .points {
    display: grid;
    gap: 10px;
    margin: 26px 0 0;
    padding: 0;
    list-style: none;
  }
  .points li {
    padding: 14px 18px;
    border-radius: 14px;
    background: var(--surface);
    font-size: 14px;
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 10px;
  }
  .keys button {
    height: 40px;
    padding: 0 16px;
    border-radius: 10px;
    background: var(--control);
    font-weight: 600;
  }
  .keys button.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .card {
    margin-top: 24px;
    padding: 18px 20px;
    border-radius: 14px;
    background: var(--surface);
  }
  .card h3 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  .card p {
    margin: 0 0 10px;
    font-size: 13.5px;
    color: var(--text-muted);
  }
  .card ol {
    margin: 0 0 14px;
    padding-left: 20px;
    font-size: 13.5px;
    line-height: 1.7;
  }
  .card.warn {
    box-shadow: inset 0 0 0 1.5px var(--warning);
  }
  .card.ok {
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .hint {
    margin: 12px 0 0;
    font-size: 12.5px;
    color: var(--text-faint);
  }
  .path {
    display: flex;
    gap: 8px;
  }
  .path input {
    flex: 1;
    min-width: 0;
    height: 40px;
    padding: 0 12px;
    border: none;
    border-radius: 10px;
    background: var(--control);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
    margin-bottom: 18px;
  }
  .tile {
    display: grid;
    gap: 2px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--surface);
    cursor: pointer;
    font-size: 13px;
  }
  .tile input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .tile span {
    color: var(--text-muted);
  }
  .tile.on {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 10px 0;
    font-size: 14px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 10px 18px;
    margin: 0;
    align-items: center;
  }
  dt {
    padding: 5px 12px;
    border-radius: 8px;
    background: var(--control);
    font-family: var(--font-mono);
    font-size: 13px;
    font-weight: 600;
  }
  dd {
    margin: 0;
    font-size: 14px;
  }
  .error {
    margin-top: 18px;
    color: var(--danger);
    font-size: 13.5px;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 28px;
    background: var(--surface);
    border-top: 1px solid var(--border);
  }
  .grow {
    flex: 1;
  }
  button {
    height: 40px;
    padding: 0 18px;
    border-radius: 10px;
  }
  button.primary {
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 650;
  }
  button.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  button.link {
    padding: 0;
    background: none;
    color: var(--text-muted);
    text-decoration: underline;
  }
</style>
