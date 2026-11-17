<script lang="ts">
  import { t, type Key } from "../lib/i18n.svelte";
  const ways: [string, Key][] = [
    ["share", "supportShare"],
    ["bug", "supportBugs"],
    ["thumb-up", "supportVote"],
    ["language", "supportTranslate"],
  ];
</script>

<div class="page scroll">
  <div class="hero"><i class="ti ti-heart"></i></div>
  <h1>{t("supportTitle")}</h1>
  <p class="secondary">{t("supportText")}</p>
  <div class="ways">
    {#each ways as [icon, key]}
      <div class="way"><i class="ti ti-{icon}"></i><span>{t(key)}</span></div>
    {/each}
  </div>
  <p class="muted small">{t("freeForever")}</p>
</div>

<style>
  .page { padding: 32px 32px; flex: 1; min-height: 0; max-width: 620px; }
  .hero .ti { font-size: 34px; color: #d6455d; }
  h1 { font-size: 20px; margin: 8px 0; }
  .ways { display: flex; flex-direction: column; gap: 8px; margin: 18px 0; }
  .way { display: flex; gap: 12px; align-items: center; border: 1px solid var(--card-stroke); background: var(--card); border-radius: var(--radius); padding: 12px 14px; }
  .way .ti { font-size: 20px; color: var(--accent); }
  .small { font-size: 12px; }
</style>
