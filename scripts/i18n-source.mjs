// Gathers every text the interface shows, in English and Romanian, into
// ui/src/lib/locales/source.json, the file translations are made from:
//   ui:   the interface's texts by key (ui/src/lib/i18n.svelte.ts)
//   text: the tools' texts (their manifests) and the `{ ro, en }` texts written in the
//         components, by their English text, with the Romanian one for context.
// Run after the workers are built (their manifests are in src-tauri/resources/plugins):
//   node scripts/build-workers.mjs && node scripts/i18n-source.mjs
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const ui = join(root, "ui", "src");

/** An object literal of string values, by name, from i18n.svelte.ts. */
function dictionary(src, decl) {
  const start = src.indexOf(decl);
  if (start < 0) throw new Error(`${decl} not found`);
  let i = src.indexOf("{", start);
  const open = i;
  let depth = 0;
  for (; i < src.length; i++) {
    const c = src[i];
    if (c === '"' || c === "'" || c === "`") {
      const q = c;
      for (i++; i < src.length && src[i] !== q; i++) if (src[i] === "\\") i++;
      continue;
    }
    if (c === "{") depth++;
    if (c === "}" && --depth === 0) break;
  }
  return Function(`"use strict"; return (${src.slice(open, i + 1)});`)();
}

const i18nSrc = readFileSync(join(ui, "lib", "i18n.svelte.ts"), "utf8");
const ro = dictionary(i18nSrc, "const ro = {");
const en = dictionary(i18nSrc, "const en: Record<Key, string> = {");
const uiTexts = {};
for (const k of Object.keys(en)) uiTexts[k] = { en: en[k], ro: ro[k] ?? "" };

const text = {};
const add = (enText, roText) => {
  // Numbers and symbols ("1", "20", "→") are the same everywhere: nothing to translate.
  if (typeof enText !== "string" || !/\p{L}/u.test(enText)) return;
  if (!(enText in text)) text[enText] = roText ?? "";
};

// `{ ro: "…", en: "…" }` (either order) anywhere in the interface's code.
const STR = String.raw`"((?:[^"\\]|\\.)*)"`;
const pair = new RegExp(String.raw`\{\s*ro:\s*${STR},\s*en:\s*${STR}\s*\}|\{\s*en:\s*${STR},\s*ro:\s*${STR}\s*\}`, "gs");
const unq = (s) => JSON.parse(`"${s}"`);
function walkFiles(dir, out = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) walkFiles(p, out);
    else if (/\.(svelte|ts)$/.test(e.name)) out.push(p);
  }
  return out;
}
for (const f of walkFiles(ui)) {
  for (const m of readFileSync(f, "utf8").matchAll(pair)) {
    if (m[1] !== undefined) add(unq(m[2]), unq(m[1]));
    else add(unq(m[3]), unq(m[4]));
  }
}

// The tools' manifests: every `{ ro, en }` text (not the search words, which differ per language).
const plugins = join(root, "src-tauri", "resources", "plugins");
if (!existsSync(plugins)) throw new Error("No manifests: run node scripts/build-workers.mjs first.");
function walkManifest(v) {
  if (Array.isArray(v)) return v.forEach(walkManifest);
  if (v && typeof v === "object") {
    if (typeof v.en === "string" && typeof v.ro === "string") return add(v.en, v.ro);
    for (const [k, x] of Object.entries(v)) if (k !== "keywords") walkManifest(x);
  }
}
for (const id of readdirSync(plugins)) {
  const mf = join(plugins, id, "plugin.json");
  if (existsSync(mf)) walkManifest(JSON.parse(readFileSync(mf, "utf8")));
}

const out = { ui: uiTexts, text };
writeFileSync(join(ui, "lib", "locales", "source.json"), JSON.stringify(out, null, 1) + "\n");
const words = (s) => s.split(/\s+/).filter(Boolean).length;
console.log(`ui texts: ${Object.keys(uiTexts).length}, tool texts: ${Object.keys(text).length}, words: ${[...Object.values(uiTexts).map((v) => v.en), ...Object.keys(text)].reduce((n, s) => n + words(s), 0)}`);
