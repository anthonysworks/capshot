<script lang="ts">
  import CustomColor from "./CustomColor.svelte";

  /** The two colours of the background the user mixes. */
  let { colors = $bindable() }: { colors: string[] } = $props();
</script>

<div class="row">
  <span>From</span>
  <CustomColor value={colors[0] ?? "#000000"} presets={[]} onpick={(c) => (colors = [c, colors[1] ?? c])} />
  <span>to</span>
  <CustomColor value={colors[1] ?? "#000000"} presets={[]} onpick={(c) => (colors = [colors[0] ?? c, c])} />
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 10px;
    font-size: 12px;
    color: var(--text-muted);
  }
</style>
