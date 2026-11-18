// Checks each language file in ui/src/lib/locales against source.json:
// - every interface text and every tool text translated (English shows otherwise);
// - plural texts (`key_one`, `key_few`…) in every form the language's own rules use;
// - the same placeholders as the English text ({n}, {name}…), none lost or invented.
//   node scripts/i18n-check.mjs            all languages
//   node scripts/i18n-check.mjs de fr      some
// Exit 1 on any problem (CI runs it).
import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "ui", "src", "lib", "locales");
const source = JSON.parse(readFileSync(join(dir, "source.json"), "utf8"));
const wanted = process.argv.slice(2);
const files = readdirSync(dir).filter((f) => f.endsWith(".json") && f !== "source.json").map((f) => f.replace(".json", ""));
const langs = wanted.length ? wanted : files;

const holes = (s) => [...String(s).matchAll(/\{([a-zA-Z0-9_]+)\}/g)].map((m) => m[1]).sort().join(",");
// A plural text has its "_one" form in the source (fileCount_one…); "engineErr_other" alone
// is just a key named "other".
const pluralBase = (k) => {
  const base = k.match(/^(.*)_(zero|one|two|few|many|other)$/)?.[1];
  return base && `${base}_one` in source.ui ? base : undefined;
};

let problems = 0;
const say = (lang, msg) => {
  problems++;
  console.log(`${lang}: ${msg}`);
};

for (const lang of langs) {
  let loc;
  try {
    loc = JSON.parse(readFileSync(join(dir, `${lang}.json`), "utf8"));
  } catch (e) {
    say(lang, `can't read ${lang}.json (${e.message})`);
    continue;
  }
  const ui = loc.ui ?? {};
  const text = loc.text ?? {};
  const categories = new Intl.PluralRules(lang).resolvedOptions().pluralCategories;
  const bases = new Set();
  for (const [k, v] of Object.entries(source.ui)) {
    const base = pluralBase(k);
    if (base) {
      bases.add(base);
      continue;
    }
    if (!ui[k]) say(lang, `ui.${k} missing`);
    else if (holes(ui[k]) !== holes(v.en)) say(lang, `ui.${k} placeholders {${holes(ui[k])}} differ from {${holes(v.en)}}`);
  }
  for (const base of bases) {
    const enOther = source.ui[`${base}_other`]?.en ?? "";
    for (const c of categories) {
      const v = ui[`${base}_${c}`];
      if (!v) say(lang, `ui.${base}_${c} missing (plural form "${c}")`);
      else if (c !== "one" && holes(v) !== holes(enOther)) say(lang, `ui.${base}_${c} placeholders {${holes(v)}} differ from {${holes(enOther)}}`);
    }
  }
  for (const k of Object.keys(ui)) {
    const base = pluralBase(k);
    if (!(k in source.ui) && !(base && bases.has(base))) say(lang, `ui.${k} is not in the source (renamed or removed?)`);
  }
  for (const en of Object.keys(source.text)) {
    if (!text[en]) say(lang, `text missing: ${JSON.stringify(en.slice(0, 70))}`);
    else if (holes(text[en]) !== holes(en)) say(lang, `text placeholders differ: ${JSON.stringify(en.slice(0, 50))} → {${holes(text[en])}}`);
  }
  for (const en of Object.keys(text)) if (!(en in source.text)) say(lang, `text not in the source: ${JSON.stringify(en.slice(0, 70))}`);
}
console.log(problems ? `\n${problems} problem(s).` : `\nAll ${langs.length} language(s) complete.`);
process.exit(problems ? 1 : 0);
