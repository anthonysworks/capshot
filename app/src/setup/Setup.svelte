<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import captureDark from "../assets/setup/capture-dark.webp";
  import captureLight from "../assets/setup/capture-light.webp";
  import editorDark from "../assets/setup/editor-dark.webp";
  import editorLight from "../assets/setup/editor-light.webp";
  import recordDark from "../assets/setup/record-dark.webp";
  import recordLight from "../assets/setup/record-light.webp";
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

  type Which = "capture" | "record";
  const SHORTCUTS = [
    {
      id: "capture",
      title: "Screenshot",
      text: "Press it anywhere, drag over what you want, then copy, save, edit or pin it.",
      keys: ["PrintScreen", "Ctrl+Shift+S", "Ctrl+Alt+S"],
      art: { light: captureLight, dark: captureDark, alt: "The selection and its action bar" },
    },
    {
      id: "record",
      title: "Recording",
      text: "Press it, choose the area, and record a video or a GIF.",
      keys: ["Shift+PrintScreen", "Ctrl+Shift+R", "Ctrl+Alt+R"],
      art: { light: recordLight, dark: recordDark, alt: "The recording toolbar" },
    },
  ] as const;
  const FALLBACK: Record<Which, string> = { capture: "Ctrl+Shift+S", record: "Ctrl+Shift+R" };

  const label = (key: string) => key.replace("PrintScreen", "Print Screen");
  const caps = (key: string) => key.split("+").map(label);
  /** Windows only reacts to the key on its own, not to Shift+Print Screen. */
  const isBarePrintScreen = (key: string) => key.toLowerCase() === "printscreen";

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
    { value: "system", title: "Like Windows", text: "Follows your Windows theme" },
    { value: "light", title: "Light", text: "Light grey, black text" },
    { value: "dark", title: "Dark", text: "Near black, bright text" },
  ] as const;

  let config = $state<Config | null>(null);
  let step = $state<Step>("welcome");
  let custom = $state<Record<Which, boolean>>({ capture: false, record: false });
  /** Windows opens its own capture on Print Screen. */
  let taken = $state(false);
  let released = $state(false);
  let keptWindowsKey = $state(false);
  let error = $state("");
  let saving = $state(false);

  const index = $derived(STEPS.findIndex((s) => s.id === step));
  const usesPrintScreen = $derived(
    !!config && (isBarePrintScreen(config.hotkeys.capture) || isBarePrintScreen(config.hotkeys.record)),
  );
  const conflict = $derived(usesPrintScreen && taken);

  onMount(() => {
    getConfig().then((c) => {
      // Starting with Windows is on unless the user turns it off here: the
      // shortcuts only work while Capshot is running.
      c.startup = true;
      config = c;
    });
    // The Windows setting may be changed in its own page meanwhile: ask again
    // whenever this window comes back to the front.
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

  function choose(which: Which, key: string) {
    if (!config) return;
    custom[which] = false;
    keptWindowsKey = false;
    config.hotkeys[which] = key;
  }

  /** Leaves Print Screen to Windows. */
  function keepWindowsKey() {
    if (!config) return;
    config.hotkeys.capture = FALLBACK.capture;
    config.hotkeys.record = FALLBACK.record;
    custom = { capture: false, record: false };
    keptWindowsKey = true;
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

<!-- Each picture exists for both themes; the current one is shown. -->
{#snippet themed(light: string, dark: string, alt: string)}
  <img class="on-light" src={light} {alt} draggable="false" />
  <img class="on-dark" src={dark} alt="" aria-hidden="true" draggable="false" />
{/snippet}

{#snippet printScreenNotice()}
  {#if conflict}
    <section class="notice warn" aria-labelledby="ps-title">
      <h2 id="ps-title">Windows also uses Print Screen</h2>
      <p>Windows opens its Snipping Tool on that key, so both programs would start. Pick one to continue:</p>
      <div class="options">
        <div class="option">
          <h3>Capshot takes it <span class="tag">Recommended</span></h3>
          <p>
            Switches off the Windows option <em>“Use the Print screen button to open screen capture”</em> for your
            account. You can turn it back on in Settings › Accessibility › Keyboard.
          </p>
          <div class="buttons">
            <button type="button" class="primary" onclick={freePrintScreen}>Turn it off for me</button>
            <button type="button" class="link" onclick={openKeyboardSettings}>Open that Windows page</button>
          </div>
        </div>
        <div class="option">
          <h3>Windows keeps it</h3>
          <p>
            Print Screen stays with the Snipping Tool. Capshot uses <b>Ctrl+Shift+S</b> to capture and
            <b>Ctrl+Shift+R</b> to record.
          </p>
          <div class="buttons">
            <button type="button" onclick={keepWindowsKey}>Use Ctrl+Shift+S and R</button>
          </div>
        </div>
      </div>
    </section>
  {:else if usesPrintScreen}
    <section class="notice ok" role="status">
      <h2>Print Screen is yours</h2>
      <p>
        {released
          ? "Windows no longer opens the Snipping Tool on Print Screen. Capshot also intercepts the key itself, so it works right away, without signing out."
          : "Windows is not using Print Screen, so Capshot can have it."}
      </p>
    </section>
  {:else if keptWindowsKey}
    <section class="notice ok" role="status">
      <h2>Print Screen stays with Windows</h2>
      <p>Capshot uses Ctrl+Shift+S to capture and Ctrl+Shift+R to record.</p>
    </section>
  {/if}
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
        <h1>Welcome to Capshot</h1>
        <p class="lead">
          Screenshots, screen recordings and GIFs, with an editor built in. A few choices and you are set; all of
          them can be changed later in the settings.
        </p>
        <div class="hero">{@render themed(editorLight, editorDark, "The Capshot editor, with an annotated screenshot")}</div>
        <div class="pillars">
          <div><h3>Capture</h3><p>Any area, window or screen, then copy, save or pin it.</p></div>
          <div><h3>Record</h3><p>Video with the computer's sound and the microphone, or a GIF.</p></div>
          <div><h3>Edit</h3><p>Annotate, blur, trim, add a background or a device frame.</p></div>
        </div>
      {:else if step === "shortcuts"}
        <h1>Shortcuts</h1>
        <p class="lead">Capshot waits in the background. These keys start a capture from any program.</p>

        {#each SHORTCUTS as s (s.id)}
          <section class="shortcut">
            <div class="shortcut-body">
              <div class="head">
                <h2>{s.title}</h2>
                {@render keys(config.hotkeys[s.id])}
              </div>
              <p class="desc">{s.text}</p>
              <div class="choices" role="radiogroup" aria-label="{s.title} shortcut">
                {#each s.keys as key (key)}
                  <button
                    type="button"
                    role="radio"
                    aria-checked={!custom[s.id] && config.hotkeys[s.id] === key}
                    class:on={!custom[s.id] && config.hotkeys[s.id] === key}
                    onclick={() => choose(s.id, key)}>{label(key)}</button
                  >
                {/each}
                <button type="button" role="radio" aria-checked={custom[s.id]} class:on={custom[s.id]} onclick={() => (custom[s.id] = true)}>Other…</button>
              </div>
              {#if custom[s.id]}<HotkeyInput label="{s.title} shortcut" bind:value={config.hotkeys[s.id]} />{/if}
            </div>
            <figure>{@render themed(s.art.light, s.art.dark, s.art.alt)}</figure>
          </section>
          {#if s.id === "capture"}{@render printScreenNotice()}{/if}
        {/each}
      {:else if step === "folders"}
        <h1>Where to save</h1>
        <p class="lead">Your captures go here. You can still pick another place each time.</p>
        {#each [{ kind: "capture", title: "Screenshots", text: "Images are saved in this folder." }, { kind: "recording", title: "Recordings", text: "Videos and GIFs are saved in this folder." }] as const as f (f.kind)}
          <section class="group">
            <h2>{f.title}</h2>
            <p class="desc">{f.text}</p>
            <div class="path">
              <input bind:value={config[f.kind].output_dir} aria-label="{f.title} folder" />
              <button type="button" onclick={() => folder(f.kind)}>Browse…</button>
            </div>
          </section>
        {/each}
      {:else if step === "quality"}
        <h1>Quality</h1>
        <p class="lead">The starting point for your captures. It can be changed later, and on the capture toolbar.</p>

        <section class="group">
          <h2>Images</h2>
          <p class="desc">File type of screenshots.</p>
          <div class="choices" role="radiogroup" aria-label="Image format">
            {#each IMAGE_FORMATS as f (f.value)}
              <button type="button" role="radio" aria-checked={config.capture.format === f.value} class:on={config.capture.format === f.value} onclick={() => config && (config.capture.format = f.value)}
                >{f.title} <small>{f.text}</small></button
              >
            {/each}
          </div>
        </section>

        <section class="group">
          <h2>Videos</h2>
          <p class="desc">Smoothness, sharpness and the countdown before recording starts.</p>
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
        </section>

        <section class="group">
          <h2>GIFs</h2>
          <p class="desc">Lighter when smaller and slower.</p>
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
        </section>
      {:else if step === "look"}
        <h1>Choose a look</h1>
        <p class="lead">Applies to every Capshot window and to the capture toolbar.</p>
        <div class="themes" role="radiogroup" aria-label="Theme">
          {#each THEMES as t (t.value)}
            <button
              type="button"
              role="radio"
              aria-checked={config.appearance.theme === t.value}
              class="theme"
              class:on={config.appearance.theme === t.value}
              onclick={() => {
                config!.appearance.theme = t.value;
                setTheme(t.value);
              }}
            >
              <span class="preview" aria-hidden="true">
                {#if t.value === "system"}
                  <img class="half-left" src={editorLight} alt="" draggable="false" />
                  <img class="half-right" src={editorDark} alt="" draggable="false" />
                {:else}
                  <img src={t.value === "dark" ? editorDark : editorLight} alt="" draggable="false" />
                {/if}
              </span>
              <strong>{t.title}</strong>
              <span class="sub">{t.text}</span>
            </button>
          {/each}
        </div>

        <section class="group">
          <h2>Behaviour</h2>
          <label class="option-row">
            <input type="checkbox" bind:checked={config.startup} />
            <span><strong>Open Capshot when Windows starts</strong><em>It waits quietly next to the clock, so your shortcuts work as soon as you sign in.</em></span>
          </label>
          <label class="option-row">
            <input type="checkbox" bind:checked={config.capture.copy_to_clipboard} />
            <span><strong>Copy every capture to the clipboard</strong></span>
          </label>
          <label class="option-row">
            <input type="checkbox" bind:checked={config.recording.system_audio} />
            <span><strong>Record the computer's sound in videos</strong></span>
          </label>
          <label class="option-row">
            <input type="checkbox" bind:checked={config.recording.microphone} />
            <span><strong>Record the microphone too</strong></span>
          </label>
        </section>
      {:else}
        <h1>You're all set</h1>
        <p class="lead">Capshot is running in the tray, next to the clock.</p>
        <section class="group">
          <dl>
            <dt>{@render keys(config.hotkeys.capture)}</dt>
            <dd>Take a screenshot</dd>
            <dt>{@render keys(config.hotkeys.record)}</dt>
            <dd>Record a video or a GIF</dd>
          </dl>
        </section>
        <p class="hint">Right-click the tray icon for the settings, to run this setup again, or to quit.</p>
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
        <button type="button" class="primary" onclick={next} disabled={saving || (step === "shortcuts" && conflict)}>{step === "look" ? "Finish" : "Continue"}</button>
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
    padding: 22px 32px 20px;
    max-width: 820px;
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
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  h3 {
    margin: 0 0 4px;
    font-size: 14px;
    font-weight: 650;
  }
  h4 {
    margin: 14px 0 8px;
    font-size: 12px;
    font-weight: 650;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .lead {
    margin: 0 0 16px;
    font-size: 15px;
    color: var(--text-muted);
  }
  .desc {
    margin: 2px 0 12px;
    font-size: 13.5px;
    color: var(--text-muted);
  }
  .hint {
    margin: 12px 0 0;
    font-size: 12.5px;
    color: var(--text-faint);
  }

  /* Pictures: the one matching the theme is shown. */
  .on-dark {
    display: none;
  }
  :global(:root[data-theme="dark"]) .on-light {
    display: none;
  }
  :global(:root[data-theme="dark"]) .on-dark {
    display: block;
  }

  /* Welcome */
  .hero {
    aspect-ratio: 1100 / 500;
    overflow: hidden;
    border-radius: 14px;
    box-shadow:
      0 0 0 1px var(--border),
      var(--shadow);
  }
  .hero img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: top;
  }
  .pillars {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 24px;
    margin-top: 22px;
  }
  .pillars p {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
  }

  /* Shortcuts */
  .shortcut {
    display: grid;
    grid-template-columns: 1fr 230px;
    align-items: center;
    gap: 20px;
    margin-top: 14px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--surface);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  figure {
    margin: 0;
    overflow: hidden;
    border-radius: 10px;
    box-shadow: 0 0 0 1px var(--border);
  }
  figure img {
    display: block;
    width: 100%;
    height: auto;
  }
  .caps {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .caps i {
    font-style: normal;
    color: var(--text-faint);
    font-size: 12px;
  }
  kbd {
    padding: 4px 10px;
    border-radius: 7px;
    background: var(--control);
    box-shadow: inset 0 -2px 0 var(--border-strong);
    font: 600 13px var(--font-mono);
    color: var(--text);
  }
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .choices button {
    height: 32px;
    padding: 0 12px;
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
  .notice {
    margin-top: 14px;
    padding: 16px 18px;
    border-radius: 14px;
    background: var(--surface);
  }
  .notice p {
    margin: 4px 0 0;
    font-size: 13.5px;
    color: var(--text-muted);
  }
  .notice.warn {
    box-shadow: inset 0 0 0 1.5px var(--warning);
  }
  .notice.ok {
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .options {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin-top: 12px;
  }
  .option {
    display: flex;
    flex-direction: column;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--border);
  }
  .option p {
    flex: 1;
    margin: 0 0 12px;
  }
  .option b {
    color: var(--text);
    font-weight: 650;
  }
  .tag {
    margin-left: 6px;
    padding: 2px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent-text);
    font-size: 11px;
    font-weight: 650;
    vertical-align: 1px;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 14px;
  }

  /* Folders and other groups */
  .group {
    margin-top: 16px;
    padding: 16px 20px;
    border-radius: 14px;
    background: var(--surface);
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
  .check {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 14px 2px 0;
    font-size: 14px;
  }

  /* Look */
  .themes {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin: 0 0 4px;
  }
  .theme {
    display: grid;
    gap: 4px;
    height: auto;
    padding: 10px;
    text-align: left;
    border-radius: 16px;
    background: var(--surface);
    box-shadow: inset 0 0 0 1.5px var(--border);
  }
  .theme.on {
    box-shadow: inset 0 0 0 2.5px var(--accent);
  }
  .theme strong {
    margin-top: 6px;
    font-size: 14px;
  }
  .theme .sub {
    font-size: 12px;
    font-weight: 400;
    color: var(--text-muted);
  }
  .preview {
    position: relative;
    aspect-ratio: 16 / 10;
    overflow: hidden;
    border-radius: 9px;
    box-shadow: 0 0 0 1px rgb(127 127 127 / 0.3);
  }
  .preview img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: top left;
  }
  .preview .half-left {
    clip-path: inset(0 50% 0 0);
  }
  .preview .half-right {
    clip-path: inset(0 0 0 50%);
  }
  .option-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 10px 0;
    cursor: pointer;
  }
  .option-row + .option-row {
    border-top: 1px solid var(--border);
  }
  .option-row input {
    margin-top: 3px;
  }
  .option-row strong {
    display: block;
    font-size: 14px;
    font-weight: 600;
  }
  .option-row em {
    display: block;
    margin-top: 2px;
    font-style: normal;
    font-size: 13px;
    color: var(--text-muted);
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-muted);
  }

  /* Done */
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
  .buttons button.link {
    height: auto;
    color: var(--accent-text);
  }
</style>
