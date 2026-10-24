<script lang="ts">
  interface Props {
    value: string;
    label: string;
  }

  let { value = $bindable(), label }: Props = $props();
  let recording = $state(false);

  const MODIFIER_CODES = new Set([
    "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight",
    "AltLeft", "AltRight", "MetaLeft", "MetaRight",
  ]);

  function keyName(code: string): string {
    if (code.startsWith("Key")) return code.slice(3);
    if (code.startsWith("Digit")) return code.slice(5);
    return code;
  }

  function accelerator(e: KeyboardEvent): string | null {
    if (MODIFIER_CODES.has(e.code)) return null;
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    if (e.metaKey) parts.push("Super");
    parts.push(keyName(e.code));
    return parts.join("+");
  }

  function capture(e: KeyboardEvent) {
    if (!recording) return;
    e.preventDefault();
    if (e.code === "Escape") {
      recording = false;
      return;
    }
    const combo = accelerator(e);
    if (combo) {
      value = combo;
      recording = false;
    }
  }

  // Windows only delivers `keyup` for the Print Screen key.
  function captureUp(e: KeyboardEvent) {
    if (recording && e.code === "PrintScreen") capture(e);
  }
</script>

<button
  type="button"
  class="hotkey"
  class:recording
  aria-label={label}
  onclick={() => (recording = !recording)}
  onkeydown={capture}
  onkeyup={captureUp}
  onblur={() => (recording = false)}
>
  {recording ? "Press a key combination…" : value}
</button>

<style>
  .hotkey {
    min-width: 13rem;
    font-family: var(--font-mono);
    text-align: center;
  }
  .recording {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
