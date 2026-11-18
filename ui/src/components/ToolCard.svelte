<script lang="ts">
  import type { Manifest } from "../lib/api";
  import { L, t } from "../lib/i18n.svelte";
  import { toolColor } from "../lib/store.svelte";
  import { toolNumber } from "../lib/numbers";

  let { tool, onclick }: { tool: Manifest; onclick: () => void } = $props();
  // A tool in beta says so in its English name or summary ("PDF to Word (beta)"); in any
  // language the word in brackets gives way to the badge.
  const beta = /\(?\bbeta\b\)?/i;
  const en = (v: any) => (typeof v === "object" ? (v?.en ?? "") : String(v ?? ""));
  const isBeta = $derived(beta.test(en(tool.name)) || beta.test(en(tool.summary)));
  const unbracket = (text: string, english: string) => (beta.test(english) ? text.replace(beta, "").replace(/\s*[(（][^()（）]*[)）]\s*$/, "").trim() : text);
  const name = $derived(unbracket(L(tool.name), en(tool.name)));
  const summary = $derived(unbracket(L(tool.summary), en(tool.summary)));
  const num = $derived(toolNumber(tool.id));
</script>

<button class="card" {onclick} title={summary}>
  <span class="tile" style="--c:{toolColor(tool)}"><i class="ti ti-{tool.icon ?? 'file'}"></i></span>
  <span class="text">
    <span class="name">
      <span class="nm">{name}{#if isBeta}<span class="badge">{t("beta")}</span>{/if}</span>
    </span>
    <span class="sum">{summary}</span>
  </span>
  {#if num}<span class="id" title={t("toolIdHelp")}>{t("toolId", { n: num })}</span>{/if}
</button>

<style>
  .card {
    position: relative;
    text-align: left;
    border: 1px solid var(--card-stroke);
    border-radius: var(--radius-lg);
    padding: 12px 14px;
    background: var(--card);
    display: grid;
    grid-template-columns: 40px minmax(0, 1fr);
    align-items: center;
    gap: 12px;
    width: 100%;
    min-height: 66px;
    transition: background 0.12s, border-color 0.12s, box-shadow 0.12s, transform 0.12s;
  }
  .card:hover {
    background: var(--card-hover) !important;
    border-color: var(--border-strong);
    box-shadow: var(--shadow);
  }
  .card:active {
    transform: scale(0.985);
  }
  .tile {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--c) 12%, transparent);
    color: var(--c);
  }
  .tile .ti {
    font-size: 21px;
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    padding-right: 30px;
  }
  .name {
    font-weight: 600;
    font-size: 14px;
    min-width: 0;
  }
  /* Whole names, on two lines when they must (never cut to "PDF to W…"). */
  .nm,
  .sum {
    overflow: hidden;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    line-height: 18px;
  }
  .sum {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .badge {
    display: inline-block;
    vertical-align: 1px;
    margin-left: 7px;
    line-height: 15px;
    font-size: 9.5px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--warn-bg);
    color: var(--warn-text);
  }
  .id {
    position: absolute;
    top: 8px;
    right: 10px;
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
    letter-spacing: 0.02em;
  }
</style>
