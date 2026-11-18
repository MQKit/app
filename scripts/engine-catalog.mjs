// The engine catalogs (src-tauri/engines.toml, src-tauri/engines.python.toml), read the
// way the app reads them, for scripts/check-engines.mjs and scripts/mirror-engines.mjs.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
/** MQKit's copies: a release per engine and version, assets named as at their source. */
export const MIRROR_REPO = "MQKit/engines";
export const MIRROR = `https://github.com/${MIRROR_REPO}/releases/download`;

/** Engines: { name, version, license, source, mirror, files: [{ url, size, sha256, name }] }. */
export function engines() {
  const out = [];
  for (const catalog of ["engines.toml", "engines.python.toml"]) {
    let engine = null;
    let item = null;
    for (const line of readFileSync(join(root, "src-tauri", catalog), "utf8").split(/\r?\n/)) {
      const table = line.match(/^\[([A-Za-z0-9_-]+)\]\s*$/);
      const wheel = line.match(/^\[\[([A-Za-z0-9_-]+)\.wheels\]\]\s*$/);
      if (table) {
        engine = { name: table[1], mirror: true, files: [] };
        item = engine;
        out.push(engine);
        continue;
      }
      if (wheel) {
        item = {};
        engine.files.push(item);
        continue;
      }
      const kv = line.match(/^(version|url|size|sha256|mirror|license|source)\s*=\s*(?:"([^"]*)"|(\d+)|(true|false))/);
      if (!kv || !item) continue;
      item[kv[1]] = kv[4] ? kv[4] === "true" : (kv[2] ?? Number(kv[3]));
    }
  }
  for (const e of out) {
    if (e.url) e.files.unshift({ url: e.url, size: e.size, sha256: e.sha256 });
    for (const f of e.files) f.name = f.url.split("/").pop().split(/[?#]/)[0];
    e.tag = `${e.name}-${e.version}`;
  }
  return out;
}

export const mirrorUrl = (e, f) => `${MIRROR}/${e.tag}/${f.name}`;
