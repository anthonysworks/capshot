<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import { editorRead } from "../lib/api";
  import Icon from "../editor/Icon.svelte";

  /** A screenshot kept on top of the other windows: drag to move it,
   * wheel to change its opacity, double-click or Esc to close it. */
  let { path }: { path: string } = $props();

  let url = $state("");
  let opacity = $state(1);

  onMount(() => {
    editorRead(path).then((bytes) => (url = URL.createObjectURL(new Blob([bytes]))));
    return () => URL.revokeObjectURL(url);
  });

  const close = () => getCurrentWindow().destroy();

  function down(e: PointerEvent) {
    if (e.button === 0 && !(e.target as HTMLElement).closest("button")) getCurrentWindow().startDragging();
  }

  function wheel(e: WheelEvent) {
    opacity = Math.max(0.2, Math.min(1, opacity - Math.sign(e.deltaY) * 0.1));
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && close()} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="pin" onpointerdown={down} ondblclick={close} onwheel={wheel} style="opacity:{opacity}">
  {#if url}<img src={url} alt="Pinned capture" draggable="false" />{/if}
  <button type="button" class="close" title="Close (Esc)" aria-label="Close" onclick={close}><Icon name="close" /></button>
</div>

<style>
  .pin {
    position: fixed;
    inset: 0;
    cursor: move;
    background: #000;
  }
  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .close {
    position: absolute;
    top: 8px;
    right: 8px;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-radius: 50%;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .pin:hover .close {
    opacity: 1;
  }
</style>
