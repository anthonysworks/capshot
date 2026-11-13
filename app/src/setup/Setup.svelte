<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import {
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

  type Step = "welcome" | "shortcuts" | "folders" | "quality" | "look" | "done";
  const STEPS: { id: Step; label: string }[] = [
    { id: "welcome", label: "Welcome" },
    { id: "shortcuts", label: "Shortcuts" },
    { id: "folders", label: "Folders" },
    { id: "quality", label: "Quality" },
    { id: "look", label: "Look" },
    { id: "done", label: "Ready" },
  ];

  /** Keys offered besides a custom one. Print Screen is the natural choice,
   * but Windows keeps it for its own screen capture unless told otherwise. */
  const CAPTURE_KEYS = ["PrintScreen", "Ctrl+Shift+S", "Ctrl+Alt+S"];
  const RECORD_KEYS = ["Shift+PrintScreen", "Ctrl+Shift+R", "Ctrl+Alt+R"];
  const label = (key: string) => key.replace("PrintScreen", "Print Screen");
  const caps = (key: string) => key.split("+").map(label);

  const IMAGE_FORMATS = [
    { value: "png", title: "PNG", text: "Sharp, no loss" },
    { value: "jpeg", title: "JPEG", text: "Smaller files" },
    { value: "webp", title: "WebP", text: "Small and sharp" },
  ] as const;
  const FPS = [24, 30, 60, 120];
  /** Encoder quality (lower is better) behind each wording. */
  const VIDEO_QUALITY = [
    { crf: 28, title: "Smaller" },
    { crf: 23, title: "Balanced" },
    { crf: 18, title: "Best" },
  ];
  const COUNTDOWNS = [0, 3, 5, 10];
  const GIF_FPS = [10, 15, 20, 30];
  const GIF_WIDTHS = [
    { value: 480, title: "480 px" },
    { value: 640, title: "640 px" },
    { value: 960, title: "960 px" },
    { value: 0, title: "Original" },
  ];
  /** The wording closest to an encoder quality. */
  const nearestQuality = (crf: number) =>
    VIDEO_QUALITY.reduce((best, q) => (Math.abs(q.crf - crf) < Math.abs(best.crf - crf) ? q : best)).crf;

  const THEMES = [
    { value: "system", title: "Like Windows", text: "Follows your Windows theme." },
    { value: "light", title: "Light", text: "Bright and airy." },
    { value: "dark", title: "Dark", text: "Easy on the eyes." },
  ] as const;

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
    getConfig().then((c) => {
      // Starting with Windows is on unless the user turns it off here: the
      // shortcuts only work while Capshot is running.
      c.startup = true;
      config = c;
    });
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
    const chosen = await pickFolder(
      kind === "capture" ? "Save screenshots in" : "Save recordings in",
      config[kind].output_dir,
    );
    if (chosen) config[kind].output_dir = chosen;
  }

  async function finish(skip = false) {
    if (!config) return;
    saving = true;
    error = "";
    try {
      const base = await getConfig();
      // Skipping keeps the settings as they are, but still starts with Windows.
      const next: Config = skip ? { ...base, startup: true, setup_done: true } : { ...config, setup_done: true };
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

{#snippet keys(key: string)}
  <span class="caps">
    {#each caps(key) as part, i (i)}{#if i > 0}<i>+</i>{/if}<kbd>{part}</kbd>{/each}
  </span>
{/snippet}

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
        <div class="hero" aria-hidden="true">
          <svg viewBox="0 0 560 250">
            <defs>
              <linearGradient id="desk" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0" stop-color="#4f7cff" />
                <stop offset="1" stop-color="#8b5cf6" />
              </linearGradient>
            </defs>
            <rect width="560" height="250" rx="18" fill="url(#desk)" />
            <rect x="34" y="28" width="210" height="14" rx="7" fill="#fff" opacity="0.35" />
            <rect x="34" y="52" width="140" height="10" rx="5" fill="#fff" opacity="0.25" />
            <rect x="330" y="30" width="190" height="110" rx="10" fill="#fff" opacity="0.18" />
            <!-- The area being captured -->
            <rect x="96" y="76" width="300" height="128" rx="4" fill="#fff" opacity="0.14" />
            <rect x="96" y="76" width="300" height="128" rx="4" fill="none" stroke="#fff" stroke-width="2" stroke-dasharray="7 5" />
            {#each [[96, 76], [396, 76], [96, 204], [396, 204], [246, 76], [246, 204]] as [x, y] (`${x},${y}`)}
              <circle cx={x} cy={y} r="5.5" fill="#fff" stroke="#2563eb" stroke-width="2" />
            {/each}
            <rect x="214" y="52" width="64" height="18" rx="9" fill="#111" opacity="0.7" />
            <text x="246" y="65" text-anchor="middle" font-size="10.5" font-weight="600" fill="#fff">300 × 128</text>
            <!-- The bar of actions under it -->
            <rect x="130" y="214" width="236" height="30" rx="15" fill="#fff" />
            <rect x="136" y="219" width="62" height="20" rx="10" fill="#2563eb" />
            <text x="167" y="233" text-anchor="middle" font-size="10.5" font-weight="700" fill="#fff">Copy</text>
            {#each ["Save", "Edit", "Pin", "Text"] as t, i (t)}
              <text x={222 + i * 40} y="233" text-anchor="middle" font-size="10.5" font-weight="600" fill="#444">{t}</text>
            {/each}
          </svg>
        </div>
        <h1>Welcome to Capshot</h1>
        <p class="lead">
          Capture your screen with one key. A few quick choices will set it up the way you like; you can change
          everything later.
        </p>
        <ul class="points">
          <li><b class="dot cap"></b><span><strong>Capture</strong> any area, window or screen.</span></li>
          <li><b class="dot rec"></b><span><strong>Record</strong> videos and GIFs, with sound if you want.</span></li>
          <li><b class="dot edit"></b><span><strong>Edit</strong> with annotations, zoom, backgrounds and device frames.</span></li>
        </ul>
      {:else if step === "shortcuts"}
        <h1>Shortcuts</h1>
        <p class="lead">Pick the keys that start a screenshot and a recording.</p>

        <div class="shortcut">
          <div class="shortcut-head">
            <svg viewBox="0 0 40 40" aria-hidden="true"><rect x="5" y="9" width="30" height="22" rx="5" fill="none" stroke="currentColor" stroke-width="2.4" /><circle cx="20" cy="20" r="6" fill="none" stroke="currentColor" stroke-width="2.4" /></svg>
            <div><h2>Screenshot</h2>{@render keys(config.hotkeys.capture)}</div>
          </div>
          <div class="choices" role="radiogroup" aria-label="Screenshot shortcut">
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
        </div>

        <div class="shortcut">
          <div class="shortcut-head">
            <svg viewBox="0 0 40 40" aria-hidden="true"><rect x="4" y="10" width="21" height="20" rx="5" fill="none" stroke="currentColor" stroke-width="2.4" /><path d="M25 17l11-6v18l-11-6z" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linejoin="round" /></svg>
            <div><h2>Recording</h2>{@render keys(config.hotkeys.record)}</div>
          </div>
          <div class="choices" role="radiogroup" aria-label="Recording shortcut">
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
        </div>

        {#if usesPrintScreen && taken}
          <div class="card warn" role="note">
            <h3>Windows is keeping the Print Screen key</h3>
            <p>
              Windows 11 opens its own Snipping Tool on Print Screen, so both programs would start. Free the key in
              Windows (a single switch):
            </p>
            <div class="mock" aria-hidden="true">
              <div class="mock-path">Settings › Accessibility › Keyboard</div>
              <div class="mock-row">
                <span>Use the Print screen button to open screen capture</span>
                <span class="toggle" class:on={taken}><i></i></span>
              </div>
              <div class="mock-step">Turn this switch <strong>off</strong></div>
            </div>
            <div class="buttons">
              <button type="button" class="primary" onclick={freePrintScreen}>Turn it off for me</button>
              <button type="button" onclick={openKeyboardSettings}>Open Windows settings</button>
              <button type="button" onclick={useOtherKeys}>Use Ctrl+Shift+S / R instead</button>
            </div>
            <p class="hint">
              “Turn it off for me” changes only that switch, for your account; you can turn it back on in the same
              Windows page. Windows may ask you to sign out and in again before it takes effect.
            </p>
          </div>
        {:else if usesPrintScreen}
          <div class="card ok" role="status">
            <div class="mock-row done">
              <span>Use the Print screen button to open screen capture</span>
              <span class="toggle"><i></i></span>
            </div>
            <p>{released ? "Print Screen is free: Windows no longer uses it." : "Print Screen is available: Windows is not using it."}</p>
          </div>
        {/if}
      {:else if step === "folders"}
        <h1>Where to save</h1>
        <p class="lead">Your captures go here. You can still pick another place each time.</p>
        {#each [{ kind: "capture", title: "Screenshots", art: "shot" }, { kind: "recording", title: "Recordings", art: "video" }] as const as f (f.kind)}
          <div class="folder">
            <div class="folder-art {f.art}" aria-hidden="true">
              <svg viewBox="0 0 64 52">
                <path d="M4 12a5 5 0 0 1 5-5h14l6 7h26a5 5 0 0 1 5 5v22a5 5 0 0 1-5 5H9a5 5 0 0 1-5-5z" fill="currentColor" opacity="0.9" />
                {#if f.art === "shot"}
                  <rect x="23" y="24" width="18" height="13" rx="3" fill="#fff" /><circle cx="32" cy="30.5" r="3.4" fill="currentColor" />
                {:else}
                  <rect x="21" y="24" width="14" height="13" rx="3" fill="#fff" /><path d="M36 28l7-3.5v11L36 32z" fill="#fff" />
                {/if}
              </svg>
            </div>
            <div class="folder-body">
              <h2>{f.title}</h2>
              <div class="path">
                <input bind:value={config[f.kind].output_dir} aria-label="{f.title} folder" />
                <button type="button" onclick={() => folder(f.kind)}>Browse…</button>
              </div>
            </div>
          </div>
        {/each}
      {:else if step === "quality"}
        <h1>Quality</h1>
        <p class="lead">The starting point for your captures. All of it can be changed later, and on the capture toolbar.</p>

        <div class="shortcut">
          <div class="shortcut-head">
            <svg viewBox="0 0 40 40" aria-hidden="true"><rect x="5" y="9" width="30" height="22" rx="5" fill="none" stroke="currentColor" stroke-width="2.4" /><circle cx="20" cy="20" r="6" fill="none" stroke="currentColor" stroke-width="2.4" /></svg>
            <div><h2>Images</h2><span class="sub">File type of screenshots</span></div>
          </div>
          <div class="choices" role="radiogroup" aria-label="Image format">
            {#each IMAGE_FORMATS as f (f.value)}
              <button type="button" role="radio" aria-checked={config.capture.format === f.value} class:on={config.capture.format === f.value} onclick={() => config && (config.capture.format = f.value)}
                >{f.title} <small>{f.text}</small></button
              >
            {/each}
          </div>
        </div>

        <div class="shortcut">
          <div class="shortcut-head">
            <svg viewBox="0 0 40 40" aria-hidden="true"><rect x="4" y="10" width="21" height="20" rx="5" fill="none" stroke="currentColor" stroke-width="2.4" /><path d="M25 17l11-6v18l-11-6z" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linejoin="round" /></svg>
            <div><h2>Videos</h2><span class="sub">Smoothness, sharpness and the countdown before it starts</span></div>
          </div>
          <h4>Frame rate</h4>
          <div class="choices" role="radiogroup" aria-label="Video frame rate">
            {#each FPS as fps (fps)}
              <button type="button" role="radio" aria-checked={config.recording.fps === fps} class:on={config.recording.fps === fps} onclick={() => config && (config.recording.fps = fps)}>{fps} fps</button>
            {/each}
          </div>
          <h4>Quality</h4>
          <div class="choices" role="radiogroup" aria-label="Video quality">
            {#each VIDEO_QUALITY as q (q.crf)}
              <button type="button" role="radio" aria-checked={nearestQuality(config.recording.quality) === q.crf} class:on={nearestQuality(config.recording.quality) === q.crf} onclick={() => config && (config.recording.quality = q.crf)}>{q.title}</button>
            {/each}
          </div>
          <h4>Countdown before recording</h4>
          <div class="choices" role="radiogroup" aria-label="Countdown">
            {#each COUNTDOWNS as s (s)}
              <button type="button" role="radio" aria-checked={config.recording.countdown_secs === s} class:on={config.recording.countdown_secs === s} onclick={() => config && (config.recording.countdown_secs = s)}>{s === 0 ? "None" : `${s} s`}</button>
            {/each}
          </div>
          <label class="check"><input type="checkbox" bind:checked={config.recording.capture_cursor} /> Show the mouse pointer in recordings</label>
        </div>

        <div class="shortcut">
          <div class="shortcut-head">
            <svg viewBox="0 0 40 40" aria-hidden="true"><rect x="5" y="8" width="30" height="24" rx="5" fill="none" stroke="currentColor" stroke-width="2.4" /><text x="20" y="25" text-anchor="middle" font-size="11" font-weight="800" fill="currentColor">GIF</text></svg>
            <div><h2>GIFs</h2><span class="sub">Animated pictures: lighter when smaller and slower</span></div>
          </div>
          <h4>Frame rate</h4>
          <div class="choices" role="radiogroup" aria-label="GIF frame rate">
            {#each GIF_FPS as fps (fps)}
              <button type="button" role="radio" aria-checked={config.gif.fps === fps} class:on={config.gif.fps === fps} onclick={() => config && (config.gif.fps = fps)}>{fps} fps</button>
            {/each}
          </div>
          <h4>Width</h4>
          <div class="choices" role="radiogroup" aria-label="GIF width">
            {#each GIF_WIDTHS as w (w.value)}
              <button type="button" role="radio" aria-checked={config.gif.max_width === w.value} class:on={config.gif.max_width === w.value} onclick={() => config && (config.gif.max_width = w.value)}>{w.title}</button>
            {/each}
          </div>
          <label class="check"><input type="checkbox" bind:checked={config.gif.repeat} /> Loop forever</label>
        </div>
      {:else if step === "look"}
        <h1>Choose a look</h1>
        <p class="lead">Applies to every Capshot window and to the capture toolbar.</p>
        <div class="themes" role="radiogroup" aria-label="Theme">
          {#each THEMES as t (t.value)}
            <button
              type="button"
              role="radio"
              aria-checked={config.appearance.theme === t.value}
              class="theme theme-{t.value}"
              class:on={config.appearance.theme === t.value}
              onclick={() => {
                config!.appearance.theme = t.value;
                setTheme(t.value);
              }}
            >
              <span class="preview" aria-hidden="true">
                {#each t.value === "system" ? ["light", "dark"] : [t.value] as tone (tone)}
                  <span class="mini {tone}">
                    <span class="bar"><i></i><i></i><i></i></span>
                    <span class="body">
                      <span class="rail"><i></i><i></i><i></i></span>
                      <span class="canvas"><i class="shot"></i><i class="btn"></i></span>
                    </span>
                  </span>
                {/each}
              </span>
              <strong>{t.title}</strong>
              <span class="sub">{t.text}</span>
            </button>
          {/each}
        </div>

        <div class="startup">
          <svg viewBox="0 0 40 40" aria-hidden="true"><circle cx="20" cy="20" r="14" fill="none" stroke="currentColor" stroke-width="2.4" /><path d="M20 10v10" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" /></svg>
          <label>
            <input type="checkbox" bind:checked={config.startup} />
            <span>
              <strong>Open Capshot when Windows starts</strong>
              <em>It waits quietly next to the clock, so your shortcut works as soon as you sign in.</em>
            </span>
          </label>
        </div>
        <label class="check"><input type="checkbox" bind:checked={config.capture.copy_to_clipboard} /> Copy every capture to the clipboard</label>
        <label class="check"><input type="checkbox" bind:checked={config.recording.system_audio} /> Record the computer's sound in videos</label>
        <label class="check"><input type="checkbox" bind:checked={config.recording.microphone} /> Record the microphone too</label>
      {:else}
        <div class="done-art" aria-hidden="true">
          <svg viewBox="0 0 64 64"><circle cx="32" cy="32" r="28" fill="#2563eb" /><path d="M19 33l9 9 17-19" fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </div>
        <h1>You're all set</h1>
        <p class="lead">Capshot is running in the tray, next to the clock.</p>
        <div class="card">
          <dl>
            <dt>{@render keys(config.hotkeys.capture)}</dt>
            <dd>Take a screenshot</dd>
            <dt>{@render keys(config.hotkeys.record)}</dt>
            <dd>Record a video or a GIF</dd>
          </dl>
        </div>
        <p class="hint">
          Right-click the tray icon for the settings, to run this setup again, or to quit.
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
        <button type="button" class="primary" onclick={next} disabled={saving}>{step === "look" ? "Finish" : "Continue"}</button>
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
    padding: 28px 40px 20px;
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
    margin: 0 0 4px;
    font-size: 15px;
    font-weight: 650;
  }
  .lead {
    margin: 0 0 8px;
    font-size: 15px;
    color: var(--text-muted);
  }

  /* Welcome */
  .hero svg {
    display: block;
    width: 100%;
    margin-bottom: 22px;
    border-radius: 18px;
    box-shadow: var(--shadow-soft);
  }
  .points {
    display: grid;
    gap: 10px;
    margin: 22px 0 0;
    padding: 0;
    list-style: none;
  }
  .points li {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 18px;
    border-radius: 14px;
    background: var(--surface);
    font-size: 14px;
  }
  .dot {
    flex: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
  }
  .dot.cap {
    background: #2563eb;
  }
  .dot.rec {
    background: #e5484d;
  }
  .dot.edit {
    background: #8b5cf6;
  }

  /* Shortcuts */
  .shortcut {
    margin-top: 20px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--surface);
  }
  .shortcut-head {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 12px;
  }
  .shortcut-head svg {
    width: 38px;
    height: 38px;
    padding: 8px;
    box-sizing: border-box;
    border-radius: 12px;
    background: var(--control);
    color: var(--accent-text);
  }
  .caps {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .caps i {
    font-style: normal;
    color: var(--text-faint);
    font-size: 12px;
  }
  kbd {
    padding: 3px 9px;
    border-radius: 7px;
    background: var(--control);
    box-shadow: inset 0 -2px 0 var(--border-strong);
    font: 600 12.5px var(--font-mono);
    color: var(--text);
  }
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .choices button {
    height: 36px;
    padding: 0 14px;
    border-radius: 10px;
    background: var(--control);
    font-weight: 600;
    font-size: 13px;
  }
  .choices button.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .choices small {
    margin-left: 4px;
    font-weight: 400;
    opacity: 0.75;
  }
  h4 {
    margin: 14px 0 8px;
    font-size: 12px;
    font-weight: 650;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-muted);
  }
  .card {
    margin-top: 20px;
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
  .card.warn {
    box-shadow: inset 0 0 0 1.5px var(--warning);
  }
  .card.ok {
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .mock {
    margin: 4px 0 14px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--border);
  }
  .mock-path {
    margin-bottom: 10px;
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .mock-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    padding: 12px 14px;
    border-radius: 10px;
    background: var(--surface);
    font-size: 13px;
  }
  .mock-row.done {
    margin-bottom: 12px;
  }
  .mock-step {
    margin-top: 10px;
    font-size: 12.5px;
    color: var(--accent-text);
  }
  .toggle {
    flex: none;
    position: relative;
    width: 40px;
    height: 22px;
    border-radius: 11px;
    background: var(--border-strong);
    transition: background 0.2s;
  }
  .toggle i {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    transition: transform 0.2s;
  }
  .toggle.on {
    background: var(--accent);
  }
  .toggle.on i {
    transform: translateX(18px);
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

  /* Folders */
  .folder {
    display: flex;
    align-items: center;
    gap: 18px;
    margin-top: 18px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--surface);
  }
  .folder-art {
    flex: none;
    width: 64px;
  }
  .folder-art.shot {
    color: #2563eb;
  }
  .folder-art.video {
    color: #e5484d;
  }
  .folder-body {
    flex: 1;
    min-width: 0;
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

  /* Look */
  .themes {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin: 18px 0 22px;
  }
  .theme {
    display: grid;
    gap: 4px;
    height: auto;
    padding: 12px;
    text-align: left;
    border-radius: 16px;
    background: var(--surface);
    box-shadow: inset 0 0 0 1.5px var(--border);
  }
  .theme.on {
    box-shadow: inset 0 0 0 2.5px var(--accent);
  }
  .theme strong {
    margin-top: 8px;
    font-size: 14px;
  }
  .theme .sub {
    font-size: 12px;
    font-weight: 400;
    color: var(--text-muted);
  }
  .preview {
    display: flex;
    overflow: hidden;
    border-radius: 10px;
    box-shadow: 0 0 0 1px rgb(127 127 127 / 0.3);
  }
  .mini {
    flex: 1;
    display: grid;
    grid-template-rows: 10px 62px;
    min-width: 0;
    --m-bg: #f2f2f5;
    --m-surface: #fff;
    --m-line: #d9d9df;
    --m-ink: #c4c4cc;
  }
  .mini.dark {
    --m-bg: #141416;
    --m-surface: #1c1c1f;
    --m-line: #2c2c31;
    --m-ink: #3a3a42;
  }
  .mini .bar {
    display: flex;
    align-items: center;
    gap: 3px;
    padding: 0 5px;
    background: var(--m-surface);
    border-bottom: 1px solid var(--m-line);
  }
  .mini .bar i {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--m-ink);
  }
  .mini .body {
    display: flex;
    background: var(--m-bg);
  }
  .mini .rail {
    display: grid;
    align-content: start;
    gap: 5px;
    width: 14px;
    padding: 7px 4px;
    background: var(--m-surface);
  }
  .mini .rail i {
    height: 5px;
    border-radius: 2px;
    background: var(--m-ink);
  }
  .mini .canvas {
    position: relative;
    flex: 1;
    margin: 6px;
    border-radius: 4px;
    background: var(--m-surface);
  }
  .mini .shot {
    position: absolute;
    inset: 6px 8px 14px;
    border-radius: 3px;
    background: linear-gradient(135deg, #4f7cff, #8b5cf6);
    opacity: 0.85;
  }
  .mini .btn {
    position: absolute;
    right: 6px;
    bottom: 4px;
    width: 16px;
    height: 6px;
    border-radius: 3px;
    background: #2563eb;
  }
  .startup {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--surface);
  }
  .startup svg {
    flex: none;
    width: 38px;
    height: 38px;
    padding: 8px;
    box-sizing: border-box;
    border-radius: 12px;
    background: var(--control);
    color: var(--accent-text);
  }
  .startup label {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    cursor: pointer;
  }
  .startup label input {
    margin-top: 4px;
  }
  .startup strong {
    display: block;
    font-size: 14px;
  }
  .startup em {
    display: block;
    margin-top: 2px;
    font-style: normal;
    font-size: 13px;
    color: var(--text-muted);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 12px 4px 0;
    font-size: 14px;
  }

  /* Done */
  .done-art svg {
    width: 64px;
    height: 64px;
    margin-bottom: 12px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 12px 18px;
    margin: 0;
    align-items: center;
  }
  dt {
    margin: 0;
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
