<script lang="ts">
  import { ICONS, type IconName } from "./icons";

  let { name }: { name: IconName } = $props();
  const filled = $derived(name === "play" || name === "pause");
</script>

<svg viewBox="0 0 20 20" width="18" height="18" aria-hidden="true">
  <path
    d={ICONS[name]}
    fill={filled ? "currentColor" : "none"}
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
</svg>
