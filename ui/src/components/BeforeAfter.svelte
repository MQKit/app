<script lang="ts">
  import { api, fileSrc } from "../lib/api";
  import { t, L } from "../lib/i18n.svelte";
  import type { Item } from "../lib/item";

  let { tool, items, options }: { tool: string; items: Item[]; options: Record<string, any> } = $props();

  let fileId = $state<string | null>(null);
  let pos = $state(50);
  let data = $state<any>(null);
  let loading = $state(false);
  let seq = 0;
  let shown = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const BOX = 400;
  const dpr = window.devicePixelRatio || 1;

  const choices = $derived(items.filter((i) => i.status === "ok"));
  const current = $derived(choices.find((i) => i.id === fileId) ?? choices[0]);

  $effect(() => {
    const it = current;
    const opts = JSON.stringify(options);
    clearTimeout(timer);
    if (!it) {
      data = null;
      return;
    }
    const my = ++seq;
    loading = true;
    timer = setTimeout(async () => {
      try {
        // 100 % means one image pixel per screen pixel.
        const px = Math.round(BOX * (window.devicePixelRatio || 1));
        const r = await api.preview(tool, { id: it.id, path: it.info.path, analysis: $state.snapshot(it.props) }, JSON.parse(opts), my, px);
        if (my === seq) {
          data = r?.type === "preview" ? r : null;
          shown = my;
        }
      } catch {
        if (my === seq) data = null;
      } finally {
        if (my === seq) loading = false;
      }
    }, 300);
  });
</script>

{#if current}
  <div class="ba">
    <div class="head">
      <span class="lbl">{data && !data.before ? L({ ro: "Previzualizare", en: "Preview" }) : t("previewTitle")}</span>
      {#if choices.length > 1}
        <select value={current.id} onchange={(e) => (fileId = (e.target as HTMLSelectElement).value)}>
          {#each choices as c}<option value={c.id}>{c.info.name}</option>{/each}
        </select>
      {/if}
      {#if loading}<span class="muted small">{t("previewLoading")}</span>{/if}
    </div>
    {#if data && !data.before}
      <!-- The result alone (nothing to compare with, as a QR code made from text). -->
      <div class="frame" style="width:{data.width / dpr}px; aspect-ratio:{data.width} / {data.height}">
        <img src={fileSrc(data.path) + `?v=${shown}`} alt={L({ ro: "Previzualizare", en: "Preview" })} class="smooth" draggable="false" />
      </div>
    {:else if data}
      <div class="frame" style="width:{data.width / dpr}px; height:{data.height / dpr}px">
        <img src={fileSrc(data.before) + `?v=${shown}`} alt={t("before")} draggable="false" />
        <img src={fileSrc(data.path) + `?v=${shown}`} alt={t("after")} class="after" style="clip-path: inset(0 0 0 {pos}%)" draggable="false" />
        <div class="line" style="left:{pos}%"></div>
        <span class="tag l">{t("before")}</span>
        <span class="tag r">{t("after")}</span>
        <input type="range" min="0" max="100" step="0.5" bind:value={pos} aria-label={t("previewTitle")} />
      </div>
    {:else}
      <div class="frame empty"></div>
    {/if}
  </div>
{/if}

<style>
  .ba {
    margin-top: 14px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }
  .head .lbl {
    margin: 0;
  }
  .head select {
    height: 28px;
    max-width: 220px;
    font-size: 12px;
  }
  .small {
    font-size: 12px;
  }
  .frame {
    position: relative;
    max-width: 100%;
    border-radius: var(--radius);
    overflow: hidden;
    border: 1px solid var(--border);
    background: repeating-conic-gradient(var(--hover) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
  }
  .frame.empty {
    width: 400px;
    height: 400px;
  }
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    image-rendering: pixelated;
  }
  img.smooth {
    image-rendering: auto;
  }
  .line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: #fff;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.35);
    transform: translateX(-1px);
    pointer-events: none;
  }
  .tag {
    position: absolute;
    top: 8px;
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.55);
    color: #fff;
    pointer-events: none;
  }
  .tag.l {
    left: 8px;
  }
  .tag.r {
    right: 8px;
  }
  input[type="range"] {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    opacity: 0;
    cursor: ew-resize;
    margin: 0;
  }
</style>
