<script lang="ts">
  /** A slider that stops on notches, labelled in percent. */
  let {
    value = $bindable(),
    min = 20,
    max = 100,
    step = 10,
    label = "Quality",
    disabled = false,
    onchange,
  }: {
    value: number;
    min?: number;
    max?: number;
    step?: number;
    label?: string;
    disabled?: boolean;
    onchange?: (value: number) => void;
  } = $props();

  const notches = $derived(Array.from({ length: Math.round((max - min) / step) + 1 }, (_, i) => min + i * step));
  /** Snapped to the nearest notch. */
  const shown = $derived(Math.max(min, Math.min(max, Math.round((value - min) / step) * step + min)));
  const place = (v: number) => (v - min) / (max - min);

  function input(e: Event & { currentTarget: HTMLInputElement }) {
    value = Number(e.currentTarget.value);
    onchange?.(value);
  }
</script>

<div class="notches" class:disabled style="--fill:{place(shown) * 100}%">
  <div class="rail">
    <input type="range" {min} {max} {step} value={shown} aria-label={label} aria-valuetext="{shown}%" {disabled} oninput={input} />
    <div class="marks" aria-hidden="true">
      {#each notches as n, i (n)}
        <span class="mark" class:passed={n <= shown} style="--at:{place(n)}">
          <i></i>
          {#if i % 2 === 0 || i === notches.length - 1}<em class:current={n === shown}>{n}%</em>{/if}
        </span>
      {/each}
    </div>
  </div>
  <b>{shown}%</b>
</div>

<style>
  .notches {
    --thumb: 16px;
    display: flex;
    align-items: flex-start;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }
  .notches.disabled {
    opacity: 0.45;
  }
  .rail {
    position: relative;
    flex: 1;
    min-width: 120px;
    padding-bottom: 16px;
  }
  input {
    display: block;
    width: 100%;
    height: 20px;
    margin: 0;
    background: transparent;
    appearance: none;
    cursor: pointer;
  }
  input::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 2px;
    background: linear-gradient(
      to right,
      var(--accent) calc(var(--thumb) / 2 + (100% - var(--thumb)) * var(--fill) / 100%),
      var(--border-strong) 0
    );
  }
  input::-webkit-slider-thumb {
    width: var(--thumb);
    height: var(--thumb);
    margin-top: -6px;
    border: 2px solid var(--accent);
    border-radius: 50%;
    background: var(--surface, #fff);
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.25);
    appearance: none;
  }
  input:focus-visible::-webkit-slider-thumb {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .marks {
    position: absolute;
    left: calc(var(--thumb) / 2);
    right: calc(var(--thumb) / 2);
    top: 18px;
    height: 16px;
    pointer-events: none;
  }
  .mark {
    position: absolute;
    left: calc(var(--at) * 100%);
    display: flex;
    flex-direction: column;
    align-items: center;
    transform: translateX(-50%);
  }
  .mark i {
    width: 2px;
    height: 5px;
    border-radius: 1px;
    background: var(--border-strong);
  }
  .mark.passed i {
    background: var(--accent);
  }
  .mark em {
    margin-top: 1px;
    font-size: 10px;
    font-style: normal;
    line-height: 11px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .mark em.current {
    color: var(--text);
    font-weight: 600;
  }
  b {
    min-width: 40px;
    line-height: 20px;
    text-align: right;
    font-size: 13px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
</style>
