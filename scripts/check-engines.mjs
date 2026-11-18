// Checks that every engine the app downloads is still where src-tauri/engines*.toml says,
// at its source and in MQKit's copy (github.com/MQKit/engines): the size, and with --full
// the SHA-256 too. Run daily by .github/workflows/engines.yml; a failure means a tool
// would fail to install for someone, so the run fails and GitHub sends an email.
//
//   node scripts/check-engines.mjs            sizes (a few KB per file)
//   node scripts/check-engines.mjs --full     whole files and their SHA-256 (about 1.9 GB)
import { createHash } from "node:crypto";
import { engines, mirrorUrl } from "./engine-catalog.mjs";

const full = process.argv.includes("--full");

/** Every file of the catalogs, with where MQKit's copy is (none for engines not copied). */
function files() {
  return engines().flatMap((e) =>
    e.files.map((f) => ({ engine: e.name, version: e.version, url: f.url, size: f.size, sha256: f.sha256, mirror: e.mirror ? mirrorUrl(e, f) : null }))
  );
}

/** The size a server gives for `url`, following redirects, without the body. */
async function remoteSize(url) {
  let res = await fetch(url, { method: "HEAD", redirect: "follow", headers: { "User-Agent": "MQKit-engine-check" } });
  if (res.status === 405 || res.status === 403 || !res.headers.get("content-length")) {
    res = await fetch(url, { headers: { Range: "bytes=0-0", "User-Agent": "MQKit-engine-check" }, redirect: "follow" });
    await res.body?.cancel();
    const range = res.headers.get("content-range");
    if (res.ok && range) return { status: res.status, size: Number(range.split("/").pop()) };
  }
  return { status: res.status, size: Number(res.headers.get("content-length") ?? -1) };
}

async function remoteHash(url) {
  const res = await fetch(url, { redirect: "follow", headers: { "User-Agent": "MQKit-engine-check" } });
  if (!res.ok) return { status: res.status };
  const h = createHash("sha256");
  let size = 0;
  for await (const chunk of res.body) {
    h.update(chunk);
    size += chunk.length;
  }
  return { status: res.status, size, sha256: h.digest("hex") };
}

async function check(f, url) {
  // Some download mirrors (the LibreOffice archive picks one per request) send their
  // certificate without the intermediate one. The app completes the chain as Windows
  // does (src-tauri: platform verifier); Node can't, so that alone is only noted, after
  // a few tries for a mirror that sends it whole.
  let incomplete = false;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const r = full ? await remoteHash(url) : await remoteSize(url);
      if (r.status >= 400) return `HTTP ${r.status}`;
      if (r.size !== f.size) return `size ${r.size}, expected ${f.size}`;
      if (full && r.sha256 !== f.sha256.toLowerCase()) return `SHA-256 ${r.sha256}, expected ${f.sha256}`;
      return null;
    } catch (e) {
      const code = e.cause?.code ?? e.message;
      if (code !== "UNABLE_TO_VERIFY_LEAF_SIGNATURE") return `unreachable (${code})`;
      incomplete = true;
    }
  }
  if (incomplete) console.log(`note  ${url}: the server's certificate chain is incomplete (the app completes it)`);
  return null;
}

let failed = 0;
for (const f of files()) {
  const [source, copy] = await Promise.all([check(f, f.url), f.mirror ? check(f, f.mirror) : null]);
  const label = `${f.engine} ${f.version}: ${f.url.split("/").pop()}`;
  if (!source && !copy) {
    console.log(`ok    ${label}`);
    continue;
  }
  failed++;
  console.log(`FAIL  ${label}`);
  if (source) console.log(`      source: ${source}  ${f.url}`);
  if (copy) console.log(`      MQKit copy: ${copy}  ${f.mirror}`);
}
console.log(failed ? `\n${failed} file(s) with a problem.` : "\nEvery engine is available at its source and in MQKit's copy.");
process.exit(failed ? 1 : 0);
