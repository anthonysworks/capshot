<script lang="ts">
  /** The "any colour" button that ends a row of preset colours. It shows a
   * rainbow, or the colour itself once the current one is not a preset. */
  interface Props {
    value: string;
    /** The preset colours of the row: the button takes over when `value` is none of them. */
    presets: readonly string[];
    onpick: (color: string) => void;
    size?: number;
    title?: string;
  }
  let { value, presets, onpick, size = 24, title = "Pick any colour" }: Props = $props();

  const custom = $derived(!presets.some((c) => c.toLowerCase() === value.toLowerCase()));
  /** `<input type="color">` only takes #rrggbb. */
  const hex = $derived(/^#[0-9a-f]{6}$/i.test(value) ? value : "#ff0000");
</script>

<label class="custom" class:on={custom} style="--size:{size}px;--c:{hex}" {title}>
  <input type="color" value={hex} aria-label={title} oninput={(e) => onpick(e.currentTarget.value)} />
</label>

<style>
  .custom {
    position: relative;
    display: inline-block;
    flex: none;
    width: var(--size);
    height: var(--size);
    margin: 0 2px;
    border-radius: 50%;
    background: conic-gradient(#f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);
    box-shadow: inset 0 0 0 1px rgb(127 127 127 / 0.45);
    cursor: pointer;
    overflow: hidden;
  }
  /* A chosen colour fills the middle of the rainbow ring. */
  .custom.on::after {
    content: "";
    position: absolute;
    inset: 4px;
    border-radius: 50%;
    background: var(--c);
    box-shadow: 0 0 0 1px rgb(127 127 127 / 0.5);
    pointer-events: none;
  }
  .custom.on {
    overflow: visible;
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    opacity: 0;
    cursor: pointer;
  }
</style>
