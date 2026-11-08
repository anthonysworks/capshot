import { mount } from "svelte";
import "@fontsource-variable/inter";
import "./app.css";
import App from "./App.svelte";
import { followTheme } from "./lib/theme";
import Editor from "./editor/Editor.svelte";
import Pin from "./quick/Pin.svelte";
import Quick from "./quick/Quick.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("missing #app element");

// Editor, quick-actions and pinned windows open the same page with
// `?edit=`, `?quick=` or `?pin=<path>`.
const params = new URLSearchParams(location.search);
const edit = params.get("edit");
const quick = params.get("quick");
const pin = params.get("pin");

followTheme();

// Tool windows fill their window and scroll inside; the settings page scrolls.
if (edit || quick || pin) document.documentElement.classList.add("editor");

export default edit
  ? mount(Editor, { target, props: { path: edit } })
  : quick
    ? mount(Quick, { target, props: { path: quick } })
    : pin
      ? mount(Pin, { target, props: { path: pin } })
      : mount(App, { target });
