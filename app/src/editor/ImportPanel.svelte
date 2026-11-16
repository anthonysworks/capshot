<script lang="ts">
  import Icon from "./Icon.svelte";
  import { type Clip, type Corner, type Music } from "./imports";
  import type { Shape } from "./shapes";

  /** A picture or a video drawn over the recording, with its time range. */
  interface Item {
    id: number;
    shape: Shape;
    start: number;
    end: number;
  }

  interface Props {
    items: Item[];
    selected: number | null;
    music: Music | null;
    clips: Clip[];
    /** Fade lengths offered, in seconds. */
    fades: number[];
    /** Current length of the result, in seconds. */
    output: number;
    /** Position of the playhead in the result, in seconds. */
    playhead: number;
    format: (seconds: number) => string;
    onAddPicture: () => void;
    onAddLayer: () => void;
    onAddClip: () => void;
    onAddMusic: () => void;
    onSelect: (id: number) => void;
    onRemove: (id: number) => void;
    onCorner: (id: number, corner: Corner) => void;
    onStartHere: (id: number) => void;
    onEndHere: (id: number) => void;
    /** Called before a slider changes something, to save an undo step. */
    onEdit: () => void;
    onRemoveMusic: () => void;
    onMoveClip: (index: number, by: number) => void;
    onRemoveClip: (id: number) => void;
  }

  let {
    items,
    selected,
    music = $bindable(),
    clips,
    fades,
    output,
    playhead,
    format,
    onAddPicture,
    onAddLayer,
    onAddClip,
    onAddMusic,
    onSelect,
    onRemove,
    onCorner,
    onStartHere,
    onEndHere,
    onEdit,
    onRemoveMusic,
    onMoveClip,
    onRemoveClip,
  }: Props = $props();

  const CORNERS: { corner: Corner; label: string }[] = [
    { corner: "top-left", label: "Top left" },
    { corner: "top-right", label: "Top right" },
    { corner: "center", label: "Centre" },
    { corner: "bottom-left", label: "Bottom left" },
    { corner: "bottom-right", label: "Bottom right" },
  ];

  const current = $derived(items.find((i) => i.id === selected) ?? null);
  const label = (item: Item) => (item.shape.kind === "video" ? "Video" : "Picture");
</script>

<div class="head"><h3>Import</h3></div>
<p class="hint">Bring pictures, videos and music into the video. You can also drop files on the window.</p>

<div class="tiles">
  <button type="button" class="tile" onclick={onAddPicture}>
    <Icon name="picture" />
    <strong>Picture</strong>
    <span>On the video</span>
  </button>
  <button type="button" class="tile" onclick={onAddLayer}>
    <Icon name="film" />
    <strong>Video layer</strong>
    <span>Over the video</span>
  </button>
  <button type="button" class="tile" onclick={onAddClip}>
    <Icon name="film" />
    <strong>Video</strong>
    <span>After the video</span>
  </button>
  <button type="button" class="tile" onclick={onAddMusic}>
    <Icon name="music" />
    <strong>Music</strong>
    <span>Under the sound</span>
  </button>
</div>

{#if items.length}
  <div class="head"><h3>On the video</h3></div>
  <ul class="list">
    {#each items as item (item.id)}
      <li class:on={item.id === selected}>
        <button type="button" class="row" onclick={() => onSelect(item.id)}>
          <Icon name={item.shape.kind === "video" ? "film" : "picture"} />
          <span class="name">{label(item)}</span>
          <span class="time">{format(item.start)} – {format(item.end)}</span>
        </button>
        <button type="button" class="icon danger" title="Remove" aria-label="Remove" onclick={() => onRemove(item.id)}><Icon name="trash" /></button>
      </li>
    {/each}
  </ul>

  {#if current}
    {@const shape = current.shape}
    <div class="group">
      <div class="field stack">
        <span>Shows</span>
        <div class="buttons">
          <button type="button" onclick={() => onStartHere(current.id)}>Starts here</button>
          <button type="button" onclick={() => onEndHere(current.id)}>Ends here</button>
        </div>
      </div>
      <div class="field stack">
        <span>Place</span>
        <div class="buttons wrap">
          {#each CORNERS as c (c.corner)}
            <button type="button" onclick={() => onCorner(current.id, c.corner)}>{c.label}</button>
          {/each}
        </div>
      </div>
      <label class="field">
        <span>Opacity</span>
        <input
          type="range"
          min="0.1"
          max="1"
          step="0.05"
          value={shape.opacity ?? 1}
          onpointerdown={onEdit}
          oninput={(e) => (shape.opacity = Number(e.currentTarget.value))}
        />
        <span class="value">{Math.round((shape.opacity ?? 1) * 100)}%</span>
      </label>
      {#if shape.kind === "video"}
        <label class="field">
          <span>Sound</span>
          <input
            type="range"
            min="0"
            max="2"
            step="0.05"
            value={shape.volume ?? 1}
            onpointerdown={onEdit}
            oninput={(e) => (shape.volume = Number(e.currentTarget.value))}
          />
          <span class="value">{Math.round((shape.volume ?? 1) * 100)}%</span>
        </label>
      {/if}
      <p class="hint">Drag it on the video to move it; drag a corner to resize it.</p>
    </div>
  {/if}
{/if}

{#if music}
  <div class="head">
    <h3>Music</h3>
    <button type="button" class="link danger" onclick={onRemoveMusic}>Remove</button>
  </div>
  <div class="group">
    <div class="file" title={music.path}><Icon name="music" /> <span>{music.name}</span></div>
    <label class="field">
      <span>Volume</span>
      <input type="range" min="0" max="2" step="0.05" bind:value={music.volume} onpointerdown={onEdit} />
      <span class="value">{Math.round(music.volume * 100)}%</span>
    </label>
    <div class="field stack">
      <span>Starts at {format(music.start)}</span>
      <div class="buttons">
        <button type="button" onclick={() => music && (music.start = 0)}>At the start</button>
        <button type="button" onclick={() => music && (music.start = Math.max(0, Math.min(output, playhead)))}>At the playhead</button>
      </div>
    </div>
    <label class="field">
      <span>Plays from</span>
      <input type="range" min="0" max={Math.max(0, music.length - 1)} step="0.5" bind:value={music.from} />
      <span class="value">{format(music.from)}</span>
    </label>
    <div class="field stack">
      <span>Fade in</span>
      <div class="buttons">
        {#each fades as f (f)}
          <button type="button" class:active={music.fadeIn === f} onclick={() => music && (music.fadeIn = f)}>{f ? `${f}s` : "Off"}</button>
        {/each}
      </div>
    </div>
    <div class="field stack">
      <span>Fade out</span>
      <div class="buttons">
        {#each fades as f (f)}
          <button type="button" class:active={music.fadeOut === f} onclick={() => music && (music.fadeOut = f)}>{f ? `${f}s` : "Off"}</button>
        {/each}
      </div>
    </div>
  </div>
{/if}

{#if clips.length}
  <div class="head"><h3>After the video</h3></div>
  <p class="hint">Played in this order, scaled to fit. MP4 exports only.</p>
  <ul class="list">
    {#each clips as clip, i (clip.id)}
      <li>
        <span class="row static" title={clip.path}>
          <Icon name="film" />
          <span class="name">{clip.name}</span>
          <span class="time">{format(clip.length)}</span>
        </span>
        <button type="button" class="icon" title="Move earlier" aria-label="Move earlier" disabled={i === 0} onclick={() => onMoveClip(i, -1)}>↑</button>
        <button type="button" class="icon" title="Move later" aria-label="Move later" disabled={i === clips.length - 1} onclick={() => onMoveClip(i, 1)}>↓</button>
        <button type="button" class="icon danger" title="Remove" aria-label="Remove" onclick={() => onRemoveClip(clip.id)}><Icon name="trash" /></button>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 6px;
  }
  h3 {
    margin: 0;
    font-size: 12px;
    font-weight: 650;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.4;
    color: var(--text-muted);
  }
  .tiles {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    height: auto;
    padding: 12px;
    border-radius: 14px;
    background: var(--control);
    text-align: left;
  }
  .tile :global(svg) {
    width: 22px;
    height: 22px;
    margin-bottom: 4px;
  }
  .tile strong {
    font-size: 13px;
  }
  .tile span {
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .list li {
    display: flex;
    align-items: center;
    gap: 2px;
    border-radius: 10px;
  }
  .list li.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  .row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 34px;
    padding: 0 8px;
    border-radius: 10px;
    background: transparent;
    font-size: 13px;
    text-align: left;
  }
  .row.static {
    color: var(--text);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .time {
    font-size: 11.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border-radius: 9px;
    background: transparent;
  }
  .icon:disabled {
    opacity: 0.35;
  }
  .danger {
    color: var(--danger);
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border-radius: 14px;
    background: var(--bg);
  }
  .field {
    display: grid;
    grid-template-columns: 74px 1fr auto;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .field.stack {
    grid-template-columns: 1fr;
    gap: 6px;
  }
  .buttons {
    display: flex;
    gap: 4px;
  }
  .buttons.wrap,
  .buttons {
    flex-wrap: wrap;
  }
  .buttons button {
    height: 28px;
    padding: 0 10px;
    border-radius: 9px;
    background: var(--control);
    font-size: 12px;
    font-weight: 600;
  }
  .buttons button.active {
    background: var(--accent);
    color: var(--on-accent);
  }
  .value {
    min-width: 40px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .file span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link {
    padding: 0;
    background: none;
    font-size: 12px;
    text-decoration: underline;
  }
</style>
