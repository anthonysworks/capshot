import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import Editor from "./editor/Editor.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("missing #app element");

// Editor windows open the same page with `?edit=<path>`.
const path = new URLSearchParams(location.search).get("edit");

export default path ? mount(Editor, { target, props: { path } }) : mount(App, { target });
