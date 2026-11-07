import { type Config, getConfig, onConfigChanged } from "./api";

export type Theme = Config["appearance"]["theme"];

const query = window.matchMedia("(prefers-color-scheme: dark)");
let chosen: Theme = "system";

function apply() {
  const dark = chosen === "dark" || (chosen === "system" && query.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

/** Shows `theme` in this window now (also before it is saved). */
export function setTheme(theme: Theme) {
  chosen = theme;
  apply();
}

/** Follows the theme setting and, with "system", Windows. */
export function followTheme() {
  apply();
  query.addEventListener("change", apply);
  getConfig()
    .then((c) => setTheme(c.appearance?.theme ?? "system"))
    .catch(() => {});
  onConfigChanged((c) => setTheme(c.appearance?.theme ?? "system"));
}
