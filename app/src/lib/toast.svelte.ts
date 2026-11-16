/** A short message shown for a few seconds: `toast.text` is empty when none is. */
export function createToast(milliseconds = 3000) {
  let text = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    get text() {
      return text;
    },
    show(message: string) {
      text = message;
      clearTimeout(timer);
      timer = setTimeout(() => (text = ""), milliseconds);
    },
  };
}
