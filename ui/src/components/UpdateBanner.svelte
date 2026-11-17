<script lang="ts">
  import { api } from "../lib/api";
  import { t, L } from "../lib/i18n.svelte";
  import { app, dismissUpdate } from "../lib/store.svelte";
</script>

{#if app.update}
  <div class="upd" role="status">
    <span class="tile"><i class="ti ti-sparkles"></i></span>
    <div class="txt">
      <b>{t("updateTitle", { v: app.update.version })}</b>
      <span class="secondary">{app.update.notes ? L(app.update.notes) : t("updateText")}</span>
    </div>
    <button class="primary small" onclick={() => api.openUrl(app.update!.url)}>{t("updateOpen")}</button>
    <button class="ghost small" onclick={dismissUpdate}>{t("updateLater")}</button>
  </div>
{/if}

<style>
  .upd {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 12px 16px 0;
    padding: 10px 12px;
    border-radius: var(--radius);
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
  }
  .tile {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    background: var(--accent-fill);
    color: var(--on-accent);
    flex: none;
  }
  .txt {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: 13px;
    line-height: 18px;
  }
  .small {
    padding: 5px 14px;
    font-size: 13px;
  }
</style>
