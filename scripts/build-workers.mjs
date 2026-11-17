// Builds the workers and gathers them, with the plugin manifests, into src-tauri/resources.
//   node scripts/build-workers.mjs [--release]
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const release = process.argv.includes("--release");
const res = join(root, "src-tauri", "resources");
const workers = join(res, "workers");
const plugins = join(res, "plugins");
const run = (cmd, args, cwd = root) => execFileSync(cmd, args, { cwd, stdio: "inherit" });

rmSync(plugins, { recursive: true, force: true });
mkdirSync(workers, { recursive: true });
mkdirSync(plugins, { recursive: true });

// worker-core (Rust)
run("cargo", ["build", "-p", "worker-core", ...(release ? ["--release"] : [])]);
const core = join(root, "target", release ? "release" : "debug", "worker-core.exe");
cpSync(core, join(workers, "worker-core.exe"));
run(core, ["manifests", plugins]);

// office-worker (C#, .NET Framework 4.8)
const officeProj = join(root, "workers", "office-worker");
if (existsSync(officeProj)) {
  try {
    run("dotnet", ["build", "-c", "Release", "--nologo", "-v", "q"], officeProj);
    const exe = join(officeProj, "bin", "Release", "net48", "office-worker.exe");
    cpSync(exe, join(workers, "office-worker.exe"));
    if (existsSync(exe + ".config")) cpSync(exe + ".config", join(workers, "office-worker.exe.config"));
    run(exe, ["manifests", plugins]);
  } catch (e) {
    console.warn("office-worker skipped:", e.message);
  }
}
// Python plugins: the scripts go next to their manifest.
const pyPlugins = join(root, "workers", "python");
if (existsSync(pyPlugins)) {
  for (const dir of readdirSync(pyPlugins)) {
    const target = join(plugins, dir);
    mkdirSync(target, { recursive: true });
    for (const f of readdirSync(join(pyPlugins, dir))) {
      if (f.endsWith(".py") && !f.startsWith("test_")) cpSync(join(pyPlugins, dir, f), join(target, f));
    }
  }
}
// Search words: workers/keywords.json is merged into every manifest (all languages).
const kwFile = join(root, "workers", "keywords.json");
if (existsSync(kwFile)) {
  const kw = JSON.parse(readFileSync(kwFile, "utf8"));
  for (const id of readdirSync(plugins)) {
    const mf = join(plugins, id, "plugin.json");
    if (!existsSync(mf) || !kw[id]) continue;
    const m = JSON.parse(readFileSync(mf, "utf8"));
    m.keywords = m.keywords ?? {};
    for (const [lang, words] of Object.entries(kw[id])) {
      m.keywords[lang] = [...new Set([...(m.keywords[lang] ?? []), ...words])];
    }
    writeFileSync(mf, JSON.stringify(m, null, 2));
  }
}
// Every tool's icon must exist in the icon font the UI ships, or its tile stays empty.
const iconCss = join(root, "ui", "node_modules", "@tabler", "icons-webfont", "dist", "tabler-icons.min.css");
if (existsSync(iconCss)) {
  const css = readFileSync(iconCss, "utf8");
  const missing = readdirSync(plugins)
    .map((id) => join(plugins, id, "plugin.json"))
    .filter(existsSync)
    .map((mf) => JSON.parse(readFileSync(mf, "utf8")))
    .filter((m) => !css.includes(`.ti-${m.icon}:before`))
    .map((m) => `${m.id} (${m.icon})`);
  if (missing.length) {
    console.error("icons not in the Tabler font:", missing.join(", "));
    process.exit(1);
  }
}
console.log("workers ready in", res);
