<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { t, L, fileCount } from "../lib/i18n.svelte";
  import { app, go, tool, toolColor } from "../lib/store.svelte";
  import { bytes, dateTime, percentChange } from "../lib/format";

  let entries = $state<any[]>([]);
  onMount(async () => {
    const h = await api.stateGet("history");
    entries = Array.isArray(h) ? h : [];
  });

  async function clear() {
    entries = [];
    await api.stateSet("history", []);
  }
</script>

<div class="page scroll">
  <div class="top">
    <h1>{t("history")}</h1>
    {#if entries.length}<button class="ghost" onclick={clear}><i class="ti ti-trash"></i> {t("clearHistory")}</button>{/if}
  </div>
  {#if !entries.length}
    <p class="muted">{t("historyEmpty")}</p>
  {/if}
  {#each entries as e}
    {@const m = tool(e.tool)}
    <button class="entry" onclick={() => m && go({ name: "tool", id: e.tool })}>
      <i class="ti ti-{m?.icon ?? 'file'}" style="color:{m ? toolColor(m) : 'var(--text-3)'}"></i>
      <div class="info">
        <div class="name">{m ? L(m.name) : e.toolName}</div>
        <div class="muted small">{dateTime(e.at)} · {fileCount(e.files)}</div>
      </div>
      <div class="stats small">
        {#if e.summary?.ok}<span class="ok"><i class="ti ti-check"></i> {e.summary.ok}</span>{/if}
        {#if e.summary?.failed}<span class="bad"><i class="ti ti-x"></i> {e.summary.failed}</span>{/if}
        {#if e.summary?.inputBytes && e.summary?.outputBytes}
          <span class="muted">{bytes(e.summary.inputBytes)} → {bytes(e.summary.outputBytes)}
            {#if e.summary.outputBytes < e.summary.inputBytes}(−{percentChange(e.summary.inputBytes, e.summary.outputBytes)}%){/if}</span>
        {/if}
      </div>
    </button>
  {/each}
</div>

<style>
  .page { padding: 24px 32px; flex: 1; min-height: 0; }
  .top { display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px; }
  h1 { font-size: 22px; margin: 0; }
  .entry { width: 100%; display: flex; align-items: center; gap: 12px; text-align: left; border: 1px solid var(--card-stroke); background: var(--card); margin-bottom: 6px; padding: 10px 12px; }
  .entry > .ti { font-size: 20px; }
  .info { flex: 1; min-width: 0; }
  .name { font-weight: 600; }
  .small { font-size: 12px; }
  .stats { display: flex; gap: 10px; align-items: center; }
  .ok { color: var(--ok-text); }
  .bad { color: var(--error-text); }
</style>
