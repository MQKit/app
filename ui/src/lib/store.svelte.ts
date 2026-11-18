import { toolNumber } from "./numbers";
import { api, type FileInfo, type Manifest } from "./api";
import { i18n, t, L, isLang, type Lang } from "./i18n.svelte";

export type Route =
  | { name: "home"; category?: string }
  | { name: "tool"; id: string; files?: FileInfo[] }
  | { name: "history" }
  | { name: "settings" }
  | { name: "support" };

export interface Settings {
  language: "system" | "ro" | "en";
  theme: "system" | "light" | "dark";
  rememberLast: boolean;
  checkUpdates: boolean;
  contextMenu: boolean;
  lastUpdateCheck?: number;
  dismissedVersion?: string;
}

/** The launch order (`11-lansare-si-actualizari.md`): the 10 tools of version 1.0. */
export const TOP = [
  "pdf-compress",
  "pdf-to-word",
  "pdf-merge",
  "images-to-pdf",
  "word-to-pdf",
  "pdf-to-images",
  "image-compress",
  "image-convert",
  "heic-to-jpg",
  "video-to-mp3",
];

export const app = $state({
  ready: false,
  tools: [] as Manifest[],
  env: {} as Record<string, any>,
  settings: { language: "system", theme: "system", rememberLast: true, checkUpdates: true, contextMenu: true } as Settings,
  update: null as null | { version: string; url: string; notes: any },
  favorites: [] as string[],
  lastValues: {} as Record<string, any>,
  route: { name: "home" } as Route,
  /** Files dropped on the home screen, waiting for a tool. */
  pending: [] as FileInfo[],
  dragging: false,
  /** A conversion is running: the tool can't be left, its results would be lost from view. */
  busy: false,
  /** When leaving was refused (the tool shows why for a moment). */
  busyRefused: 0,
  /** The open tool takes more files (from Explorer) without starting over; false when it can't now. */
  toolFiles: null as null | ((files: FileInfo[]) => boolean),
});

export function tool(id: string): Manifest | undefined {
  return app.tools.find((t) => t.id === id);
}

export function go(r: Route) {
  if (app.busy) {
    app.busyRefused = Date.now();
    return;
  }
  app.route = r;
}

const OFFICE = ["doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt", "xls", "xlsx", "ppt", "pptx"];
const VIDEO = ["mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "3gp", "mpg", "mpeg", "ts", "mts"];

export function categories(m: Manifest): string[] {
  const c = new Set<string>([m.category]);
  const ins = m.input.extensions;
  const outs = m.output.extensions;
  if (ins.includes("pdf") || outs.includes("pdf")) c.add("pdf");
  if (ins.some((e) => OFFICE.includes(e)) || outs.some((e) => ["docx", "rtf", "odt"].includes(e))) c.add("document");
  if (ins.some((e) => VIDEO.includes(e))) c.add("video");
  if (m.category === "audio") c.add("audio");
  if (ins.includes("jpg") || ins.includes("heic")) c.add("image");
  return [...c];
}

export const CAT_COLORS: Record<string, string> = {
  pdf: "var(--c-pdf)",
  image: "var(--c-image)",
  video: "var(--c-media)",
  audio: "var(--c-media)",
  document: "var(--c-doc)",
};

export function toolColor(m: Manifest): string {
  if (m.id === "word-to-pdf" || m.id === "pdf-to-word") return CAT_COLORS.document;
  return CAT_COLORS[m.category] ?? "var(--accent)";
}

/** Tools that accept every one of the files. */
export function toolsFor(files: FileInfo[]): Manifest[] {
  if (!files.length) return [];
  return orderedTools().filter((m) => m.input.anyFile || files.every((f) => m.input.extensions.includes(f.ext)));
}

/** By tool number: the ten most used first, then each category together. */
export function orderedTools(): Manifest[] {
  const pos = (id: string) => toolNumber(id) ?? 999;
  return [...app.tools].sort((a, b) => pos(a.id) - pos(b.id) || a.id.localeCompare(b.id));
}

export async function saveSettings() {
  await api.stateSet("settings", app.settings);
  applySettings();
}

export function applySettings() {
  const sys: Lang = isLang(app.env.systemLanguage) ? app.env.systemLanguage : "en";
  i18n.lang = app.settings.language === "system" ? sys : app.settings.language;
  document.documentElement.lang = i18n.lang;
  const dark =
    app.settings.theme === "dark" ||
    (app.settings.theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.documentElement.dataset.mica = String(!!app.env.mica);
  import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow()
      .setTheme(app.settings.theme === "system" ? null : app.settings.theme)
      .catch(() => {}),
  );
}

export async function toggleFavorite(id: string) {
  app.favorites = app.favorites.includes(id) ? app.favorites.filter((f) => f !== id) : [...app.favorites, id];
  await api.stateSet("favorites", app.favorites);
}

export async function rememberValues(id: string, values: Record<string, any>, output: any) {
  if (!app.settings.rememberLast) return;
  app.lastValues = { ...app.lastValues, [id]: { values, output } };
  await api.stateSet("lastValues", app.lastValues);
}

export async function init() {
  const [tools, env, settings, favorites, lastValues] = await Promise.all([
    api.tools(),
    api.env(),
    api.stateGet("settings"),
    api.stateGet("favorites"),
    api.stateGet("lastValues"),
  ]);
  app.tools = tools;
  app.env = env;
  if (settings) app.settings = { ...app.settings, ...settings };
  if (Array.isArray(favorites)) app.favorites = favorites;
  if (lastValues && typeof lastValues === "object") app.lastValues = lastValues;
  applySettings();
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applySettings);
  app.ready = true;
  // Closing the window during a conversion: asked first (it would stop).
  import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow().onCloseRequested(async (e) => {
      if (!app.busy) return;
      const { ask } = await import("@tauri-apps/plugin-dialog");
      if (!(await ask(t("busyClose"), { title: t("busyCloseTitle"), kind: "warning" }))) e.preventDefault();
    }),
  );
  // This start's own files and those of the starts Explorer made meanwhile (one per
  // selected file), gathered into one list.
  await listenForShellFiles(await api.initialFiles());
  setTimeout(checkForUpdate, 4000);
  // The Explorer menu follows the setting and the language (installed builds only).
  if (!app.env.debug && app.settings.contextMenu) api.shellMenu(true, i18n.lang, menuNames()).catch(() => {});
}

/** The tools' names in the current language, for the Explorer menu. */
export function menuNames(): Record<string, string> {
  return Object.fromEntries(app.tools.map((m) => [m.id, L(m.name)]));
}

/** Explorer starts one process per selected file: they arrive one by one and are gathered here. */
let pendingShell: { tool: string | null; files: FileInfo[] } | null = null;
let shellTimer: ReturnType<typeof setTimeout> | undefined;
function gatherShellFiles(p: { tool: string | null; files: FileInfo[] }) {
  if (!p.files.length) return;
  if (!pendingShell || pendingShell.tool !== p.tool) pendingShell = { tool: p.tool, files: [] };
  pendingShell.files.push(...p.files);
  clearTimeout(shellTimer);
  shellTimer = setTimeout(() => {
    if (pendingShell) openFromShell(pendingShell.tool, pendingShell.files);
    pendingShell = null;
  }, 400);
}

/** Listening first, then what the host kept while the interface was starting: no file
 *  from a multiple selection in Explorer is lost. */
async function listenForShellFiles(initial: { tool: string | null; files: FileInfo[] }) {
  const { listen } = await import("@tauri-apps/api/event");
  await listen<{ tool: string | null; files: FileInfo[] }>("open-files", (e) => gatherShellFiles(e.payload));
  gatherShellFiles(initial);
  for (const p of await api.shellReady()) gatherShellFiles(p);
}

function openFromShell(toolId: string | null, files: FileInfo[]) {
  if (!files.length) return;
  if (toolId && tool(toolId)) {
    // The same tool already open: its list grows, the options and files added by other
    // means stay (a new route would start the screen over).
    const r = app.route;
    if (r.name === "tool" && r.id === toolId && app.toolFiles) {
      if (!app.toolFiles(files)) app.busyRefused = Date.now();
      return;
    }
    go({ name: "tool", id: toolId, files });
  } else openWithFiles(files);
}

/** Once a day, if allowed: is there a newer version? Only a notification, never an install. */
export async function checkForUpdate(force = false) {
  const day = 24 * 3600 * 1000;
  if (!force && (!app.settings.checkUpdates || Date.now() - (app.settings.lastUpdateCheck ?? 0) < day)) return;
  try {
    const r = await api.checkUpdate();
    app.settings.lastUpdateCheck = Date.now();
    await api.stateSet("settings", app.settings);
    if (r?.available && (force || r.version !== app.settings.dismissedVersion)) app.update = r;
    return r;
  } catch {
    return null;
  }
}

export async function dismissUpdate() {
  if (app.update) app.settings.dismissedVersion = app.update.version;
  app.update = null;
  await api.stateSet("settings", app.settings);
}

/** Files from a drop or the command line: one fitting tool opens directly. */
export function openWithFiles(files: FileInfo[]) {
  const fits = toolsFor(files);
  if (fits.length === 1) {
    go({ name: "tool", id: fits[0].id, files });
    app.pending = [];
  } else {
    app.pending = files;
    go({ name: "home" });
  }
}
