<script lang="ts">
  import { api, type Manifest } from "../lib/api";
  import { L, t, fileCount, localKeywords } from "../lib/i18n.svelte";
  import { app, categories, go, orderedTools, toolsFor, TOP } from "../lib/store.svelte";
  import { numberQuery, toolNumber } from "../lib/numbers";
  import type { Key } from "../lib/i18n.svelte";
  import { bytes, norm } from "../lib/format";
  import ToolCard from "../components/ToolCard.svelte";

  let query = $state("");
  const category = $derived(app.route.name === "home" ? app.route.category : undefined);

  /** Everything a tool can be found by, in every language, without diacritics. */
  function haystack(m: Manifest): { name: string; words: string } {
    const names = typeof m.name === "object" ? Object.values(m.name).join(" ") : String(m.name);
    const all = [
      names,
      typeof m.summary === "object" ? Object.values(m.summary).join(" ") : String(m.summary),
      ...(Object.values(m.keywords ?? {}).flat() as string[]),
      L(m.name),
      L(m.summary),
      ...localKeywords(m.id),
      ...m.input.extensions,
      ...m.output.extensions,
    ];
    return { name: norm(names), words: norm(all.join(" | ")) };
  }

  /** 0 = no match. Whole phrase > every word > most words; the name counts more. */
  function score(m: Manifest, q: string): number {
    const query = norm(q).trim();
    const words = query.split(/[\s,]+/).filter((w) => w.length > 0 && !["in", "to", "a", "la", "de", "si", "and"].includes(w));
    if (!words.length) return query ? 0 : 1;
    const h = haystack(m);
    let sc = 0;
    if (h.words.includes(query)) sc += 20;
    if (h.name.includes(query)) sc += 10;
    let found = 0;
    for (const w of words) {
      if (h.words.includes(w)) {
        found++;
        sc += h.name.includes(w) ? 3 : 2;
      } else if (w.length >= 4 && h.words.includes(w.slice(0, -1))) {
        // a typo in the last letter, or a plural
        found += 0.5;
        sc += 1;
      }
    }
    if (found < words.length * 0.6) return 0;
    return sc + found;
  }

  const list = $derived.by(() => {
    let tools = app.pending.length ? toolsFor(app.pending) : orderedTools();
    if (category === "favorites") tools = tools.filter((m) => app.favorites.includes(m.id));
    else if (category) tools = tools.filter((m) => categories(m).includes(category));
    // "ID:12", "#12" or "12": that tool.
    const n = numberQuery(query);
    if (n !== undefined) {
      const hit = tools.filter((m) => toolNumber(m.id) === n);
      if (hit.length) return hit;
    }
    if (query.trim()) {
      const scored = tools.map((m) => [m, score(m, query)] as const).filter(([, sc]) => sc > 0);
      scored.sort((a, b) => b[1] - a[1]);
      tools = scored.map(([m]) => m);
    }
    return tools;
  });

  /** With nothing typed or chosen: the most used, then every category on its own. */
  const SECTIONS: { id: string; key: Key }[] = [
    { id: "pdf", key: "cat_pdf" },
    { id: "image", key: "cat_image" },
    { id: "video", key: "cat_video" },
    { id: "audio", key: "cat_audio" },
    { id: "document", key: "cat_document" },
    { id: "other", key: "cat_other" },
  ];
  const sections = $derived.by(() => {
    if (app.pending.length || category || query.trim()) return null;
    const known = new Set(SECTIONS.map((x) => x.id));
    const out: { title: string; tools: Manifest[] }[] = [{ title: t("mostUsed"), tools: list.filter((m) => TOP.includes(m.id)) }];
    for (const sec of SECTIONS) {
      const tools = list.filter((m) => !TOP.includes(m.id) && (m.category === sec.id || (sec.id === "other" && !known.has(m.category))));
      if (tools.length) out.push({ title: t(sec.key), tools });
    }
    return out;
  });

  const pendingSummary = $derived.by(() => {
    const exts = [...new Set(app.pending.map((f) => f.ext.toUpperCase()))].slice(0, 3).join(", ");
    const size = app.pending.reduce((a, f) => a + f.size, 0);
    return `${fileCount(app.pending.length)} ${exts} · ${bytes(size)}`;
  });

  function openTool(m: Manifest) {
    const files = app.pending.length ? app.pending : undefined;
    app.pending = [];
    go({ name: "tool", id: m.id, files });
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Enter" && list.length) openTool(list[0]);
    if (e.key === "Escape") query = "";
  }
</script>

<div class="home scroll">
  <div class="search">
    <i class="ti ti-search"></i>
    <input type="search" placeholder={t("searchPlaceholder")} bind:value={query} onkeydown={onSearchKey} spellcheck="false" />
  </div>

  {#if app.pending.length}
    <div class="pending">
      <i class="ti ti-files"></i>
      <div class="grow">
        <div class="ptitle">{t("forTheseFiles", { n: pendingSummary })}</div>
        <div class="secondary">{list.length ? t("chooseTool") : t("noToolForFiles")}</div>
      </div>
      <button onclick={() => (app.pending = [])}>{t("clearFiles")}</button>
    </div>
  {/if}

  {#if sections}
    {#each sections as sec (sec.title)}
      <p class="section">{sec.title}</p>
      <div class="grid">
        {#each sec.tools as m (m.id)}
          <ToolCard tool={m} onclick={() => openTool(m)} />
        {/each}
      </div>
    {/each}
  {:else if list.length}
    <div class="grid">
      {#each list as m (m.id)}
        <ToolCard tool={m} onclick={() => openTool(m)} />
      {/each}
    </div>
  {:else if category === "favorites" && !query}
    <p class="empty"><i class="ti ti-star"></i> {t("noFavorites")}</p>
  {:else if !app.pending.length}
    <p class="empty">{t("noResults")}</p>
  {/if}

</div>

<style>
  .home {
    padding: 22px 28px 28px;
    flex: 1;
    min-height: 0;
  }
  .search {
    position: relative;
    margin-bottom: 20px;
  }
  .search .ti {
    position: absolute;
    left: 13px;
    top: 11px;
    font-size: 17px;
    color: var(--text-3);
  }
  .search input {
    width: 100%;
    padding-left: 40px;
    height: 40px;
    font-size: 14px;
    border-radius: var(--radius);
  }
  .pending {
    display: flex;
    align-items: center;
    gap: 12px;
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
    border-radius: var(--radius-lg);
    padding: 12px 14px;
    margin-bottom: 16px;
  }
  .pending > .ti {
    font-size: 24px;
    color: var(--accent);
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .ptitle {
    font-weight: 600;
  }
  .section {
    margin: 0 0 10px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
  }
  .grid + .section {
    margin-top: 26px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: 10px;
  }
  .empty {
    color: var(--text-2);
    padding: 20px 0;
  }
</style>
