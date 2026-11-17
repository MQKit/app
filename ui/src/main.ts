import "@tabler/icons-webfont/dist/tabler-icons.min.css";
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

const app = mount(App, { target: document.getElementById("app")! });

export default app;

if (import.meta.env.DEV) {
  import("./lib/store.svelte").then((s) => import("./lib/api").then((a) => ((window as any).__dev = { ...s, api: a.api })));
}
