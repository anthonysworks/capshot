<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  /** A small slider with an icon, for the bar of a selected item. */
  interface Props {
    icon: IconName;
    title: string;
    value: number;
    min?: number;
    max?: number;
    step?: number;
    /** Before the value starts to change (to save an undo step). */
    onstart?: () => void;
    onchange: (value: number) => void;
  }
  let { icon, title, value, min = 0, max = 1, step = 0.05, onstart, onchange }: Props = $props();
</script>

<label class="chip" {title}>
  <Icon name={icon} />
  <input
    type="range"
    {min}
    {max}
    {step}
    {value}
    aria-label={title}
    onpointerdown={onstart}
    oninput={(e) => onchange(Number(e.currentTarget.value))}
  />
</label>

<style>
  .chip {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 6px;
    color: var(--text-muted);
  }
  input {
    width: 90px;
  }
</style>
