// Fills github.com/MQKit/engines, MQKit's own copy of the engines the app downloads, so a
// tool still installs when a source removes or replaces a file (src-tauri/src/engines.rs
// falls back on it). One release per engine and version, the files unchanged, each
// checked against the catalog's size and SHA-256 before it is uploaded. Releases that
// already exist are left alone. Needs the GitHub CLI (gh), signed in.
//
//   node scripts/mirror-engines.mjs
//
// Engines marked `mirror = false` are skipped: copying them would mean hosting the source
// of every library inside them too (FFmpeg and LibreOffice: GPL/LGPL parts). AGPL
// packages (PyMuPDF) go with their source package from PyPI, as the licence asks.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createWriteStream, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pipeline } from "node:stream/promises";
import { Readable, Transform } from "node:stream";
import { engines, MIRROR_REPO } from "./engine-catalog.mjs";

const gh = (args) => execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
const has = (tag) => {
  try {
    gh(["release", "view", tag, "-R", MIRROR_REPO, "--json", "tagName"]);
    return true;
  } catch {
    return false;
  }
};

async function download(url, dest) {
  const res = await fetch(url, { redirect: "follow", headers: { "User-Agent": "MQKit-engine-mirror" } });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  const h = createHash("sha256");
  let size = 0;
  const count = new Transform({
    transform(chunk, _, done) {
      h.update(chunk);
      size += chunk.length;
      done(null, chunk);
    },
  });
  await pipeline(Readable.fromWeb(res.body), count, createWriteStream(dest));
  return { size, sha256: h.digest("hex") };
}

/** The source package (sdist) of an AGPL wheel, from PyPI, with its SHA-256. */
async function sdist(wheelName) {
  const [project, version] = wheelName.split("-");
  const res = await fetch(`https://pypi.org/pypi/${project}/${version}/json`);
  const info = await res.json();
  const s = info.urls.find((u) => u.packagetype === "sdist");
  if (!s) throw new Error(`no source package for ${wheelName}`);
  return { url: s.url, size: s.size, sha256: s.digests.sha256, name: s.filename, source: true };
}

const work = join(tmpdir(), "mqkit-engine-mirror");
for (const e of engines()) {
  if (!e.mirror) {
    console.log(`skip  ${e.tag} (not copied: mirror = false)`);
    continue;
  }
  if (has(e.tag)) {
    console.log(`have  ${e.tag}`);
    continue;
  }
  const files = [...e.files];
  if (/AGPL/i.test(e.license ?? "")) {
    for (const f of e.files) files.push(await sdist(f.name));
  }
  const dir = join(work, e.tag);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const paths = [];
  for (const f of files) {
    const dest = join(dir, f.name);
    process.stdout.write(`get   ${e.tag}/${f.name} (${(f.size / 1e6).toFixed(1)} MB)… `);
    const got = await download(f.url, dest);
    if (got.size !== f.size || got.sha256 !== f.sha256.toLowerCase()) {
      throw new Error(`${f.name}: ${got.size} bytes, SHA-256 ${got.sha256}; the catalog says ${f.size}, ${f.sha256}`);
    }
    console.log("checked");
    paths.push(dest);
  }
  const notes = [
    `Unchanged copies of the files MQKit downloads for **${e.name} ${e.version}**, kept so the app can fall back on them when the source is unavailable. The app accepts them only with the size and SHA-256 below.`,
    "",
    `Licence: ${e.license || "see the source"}`,
    `Source: ${e.source || e.files[0].url}`,
    "",
    "| File | Size | SHA-256 |",
    "|---|---|---|",
    ...files.map((f) => `| ${f.name}${f.source ? " (source code)" : ""} | ${f.size} | \`${f.sha256.toLowerCase()}\` |`),
    "",
    "Original addresses:",
    ...files.map((f) => `- ${f.url}`),
  ].join("\n");
  const notesFile = join(dir, "NOTES.md");
  writeFileSync(notesFile, notes);
  gh(["release", "create", e.tag, "-R", MIRROR_REPO, "--title", `${e.name} ${e.version}`, "--notes-file", notesFile, ...paths]);
  console.log(`made  ${e.tag}`);
  rmSync(dir, { recursive: true, force: true });
}
rmSync(work, { recursive: true, force: true });
console.log("\nDone. Check them with: node scripts/check-engines.mjs");
