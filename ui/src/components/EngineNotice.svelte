<script lang="ts">
  import { api } from "../lib/api";
  import { t, type Key } from "../lib/i18n.svelte";
  import { bytes } from "../lib/format";
  import { downloads, downloadEngine, ENGINE_NAMES } from "../lib/engines.svelte";

  let { name, size, ondone }: { name: string; size: number; ondone?: () => void } = $props();
  const label = $derived(ENGINE_NAMES[name] ?? name);
  const d = $derived(downloads[name]);
  const whatText = $derived.by(() => {
    const k = `engineWhat_${name}` as Key;
    const s = t(k);
    return s === k ? t("engineWhat_other") : s;
  });
  const pct = $derived(d && d.total ? Math.round((d.done / d.total) * 100) : 0);

  async function start() {
    if (await downloadEngine(name)) ondone?.();
  }
  function errText(code: string) {
    const k = `engineErr_${code}` as Key;
    const s = t(k);
    return s === k ? t("engineErr_other", { code }) : s;
  }
</script>

<div class="engine">
  <div class="top">
    <i class="ti ti-download"></i>
    <span>{t("engineNeeded", { name: label, size: bytes(size), what: whatText })}</span>
  </div>
  {#if d?.running}
    <div class="bar"><div style="width:{pct}%"></div></div>
    <div class="row">
      <span class="secondary small">
        {d.total && d.done >= d.total ? t("engineVerifying") : t("engineDownloading", { name: label, done: bytes(d.done), total: bytes(d.total || size) })}
      </span>
      <button class="small" onclick={() => api.engineCancel(name)}>{t("cancel")}</button>
    </div>
  {:else}
    {#if d?.error}<p class="err small">{errText(d.error)}</p>{/if}
    <button class="dl" onclick={start}><i class="ti ti-download"></i> {t("engineDownload", { name: label })}</button>
  {/if}
</div>

<style>
  .engine { border: 1px solid var(--accent-border); background: var(--accent-bg); border-radius: var(--radius); padding: 10px 12px; margin-bottom: 12px; }
  .top { display: flex; gap: 8px; align-items: flex-start; }
  .top .ti { font-size: 16px; color: var(--accent); margin-top: 2px; }
  .dl { margin-top: 8px; display: flex; align-items: center; gap: 6px; background: var(--surface-1); }
  .bar { height: 6px; border-radius: 3px; background: var(--hover); overflow: hidden; margin-top: 10px; }
  .bar div { height: 100%; background: var(--accent-fill); transition: width 0.2s; }
  .row { display: flex; justify-content: space-between; align-items: center; margin-top: 6px; gap: 8px; }
  .small { font-size: 12px; }
  button.small { padding: 2px 10px; }
  .err { color: var(--error-text); margin: 8px 0 0; }
</style>
