<script lang="ts">
  /** A keyboard shortcut, shown as key caps ("Ctrl+Shift+S"). */
  let { keys }: { keys: string } = $props();
  const parts = $derived(keys.split("+"));
</script>

<span class="kbd" aria-label="Shortcut {keys}">
  {#each parts as part, i (i)}<kbd>{part}</kbd>{#if i < parts.length - 1}<span class="plus">+</span>{/if}{/each}
</span>

<style>
  .kbd {
    display: inline-flex;
    align-items: center;
    gap: 1px;
    white-space: nowrap;
  }
  kbd {
    padding: 0 4px;
    border: 1px solid color-mix(in srgb, currentColor 35%, transparent);
    border-bottom-width: 2px;
    border-radius: 4px;
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 15px;
    opacity: 0.8;
  }
  .plus {
    font-size: 9px;
    opacity: 0.6;
  }
</style>
