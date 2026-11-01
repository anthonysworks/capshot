<script lang="ts">
  import ImageEditor from "./ImageEditor.svelte";
  import VideoEditor from "./VideoEditor.svelte";

  let { path }: { path: string } = $props();

  const extension = $derived(path.split(".").pop()?.toLowerCase() ?? "");
  const isImage = $derived(["png", "jpg", "jpeg", "webp"].includes(extension));
</script>

{#if isImage}
  <ImageEditor {path} />
{:else}
  <VideoEditor {path} />
{/if}

<style>
  :global(body) {
    overflow: hidden;
  }
</style>
