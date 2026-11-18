<script lang="ts">
  import type { Field, Manifest } from "../lib/api";
  import { fileSrc, api } from "../lib/api";
  import { L, t, errorText, fillDetails, stageText } from "../lib/i18n.svelte";
  import { bytes, duration, percentChange } from "../lib/format";
  import type { Item } from "../lib/item";

  let {
    item = $bindable(),
    manifest,
    index,
    count,
    editable,
    onremove,
    onmove,
    onunlock,
    onretry,
  }: {
    item: Item;
    manifest: Manifest;
    index: number;
    count: number;
    editable: boolean;
    onremove: () => void;
    onmove: (delta: number) => void;
    onunlock: (password: string) => void;
    onretry?: () => void;
  } = $props();

  let password = $state("");
  const ordered = $derived(!!manifest.input.ordered);
  const thumbs = $derived(manifest.ui?.inputList === "thumbnails");
  const canThumb = $derived(["jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "avif"].includes(item.info.ext));
  const itemFields = $derived<Field[]>(manifest.input.itemFields ?? []);

  const icon = $derived.by(() => {
    const e = item.info.ext;
    if (e === "pdf") return ["file-type-pdf", "var(--c-pdf)"];
    if (["doc", "docx", "docm", "dot", "dotx", "rtf", "odt", "txt"].includes(e)) return ["file-word", "var(--c-doc)"];
    if (["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif", "ico", "jfif"].includes(e)) return ["photo", "var(--c-image)"];
    if (["mp3", "wav", "m4a", "flac", "ogg", "opus", "aac", "wma", "webm"].includes(e) && !item.props?.hasVideo) return ["music", "var(--c-media)"];
    return ["movie", "var(--c-media)"];
  });

  const meta = $derived.by(() => {
    const p = item.props ?? {};
    const parts = [bytes(item.info.size)];
    if (p.pageCount) parts.push(p.pageCount === 1 ? t("page1") : t("pages", { n: p.pageCount }));
    else if (p.cachedPageCount) parts.push(p.cachedPageCount === 1 ? t("page1") : t("pages", { n: p.cachedPageCount }));
    if (p.width && p.height) parts.push(`${p.width} × ${p.height}`);
    if (p.durationSec) parts.push(duration(p.durationSec));
    // What the tool found and shows right away (tempo, key…).
    for (const k of manifest.ui?.showProps ?? []) {
      if (p[k] == null || p[k] === "") continue;
      parts.push(k === "bpm" ? `${Math.round(p[k])} BPM` : String(p[k]));
    }
    return parts.join(" · ");
  });

  const problem = $derived.by(() => {
    switch (item.status) {
      case "analyzing":
        return { text: t("analyzing"), kind: "muted" };
      case "locked":
        return { text: t("locked"), kind: "warn" };
      case "unsupported":
        return { text: t("unsupported"), kind: "error" };
      case "corrupt":
        return { text: t("corrupt"), kind: "error" };
      case "failed":
        if (item.code === "engine_missing") return { text: t("waitingEngine"), kind: "muted" };
        return { text: errorText(item.code, manifest.errors, item.details) || t("analysisFailed"), kind: "error", retry: true };
    }
    return null;
  });
  /** A worker warning: the tool's own message, else the rule with that id, else the common text. */
  function warnText(code: string): string {
    const details = item.warningDetails?.[code];
    if (manifest.errors?.[code]) return fillDetails(L(manifest.errors[code]), details);
    const rule = (manifest.rules ?? []).find((r: any) => r.id === code);
    return rule ? fillDetails(L(rule.message), details) : errorText(code, undefined, details);
  }
</script>

<div class="row" class:bad={problem && problem.kind === "error"}>
  {#if ordered && editable}
    <div class="order">
      <button class="icon" title={t("moveUp")} disabled={index === 0} onclick={() => onmove(-1)}><i class="ti ti-chevron-up"></i></button>
      <button class="icon" title={t("moveDown")} disabled={index === count - 1} onclick={() => onmove(1)}><i class="ti ti-chevron-down"></i></button>
    </div>
  {/if}
  {#if thumbs && canThumb}
    <img class="thumb" src={fileSrc(item.info.path)} alt="" loading="lazy" />
  {:else}
    <i class="ti ti-{icon[0]} ficon" style="color:{icon[1]}"></i>
  {/if}
  <div class="info">
    <div class="name" title={item.info.path}>{item.info.name}</div>
    <div class="meta">
      {meta}
      {#if problem}<span class="p-{problem.kind}"> · {problem.text}</span>{#if problem.retry && editable && onretry}
          <button class="link small retry" onclick={onretry}>{t("tryAgain")}</button>{/if}{/if}
    </div>

    {#if item.status === "locked" && editable}
      <form class="pw" onsubmit={(e) => (e.preventDefault(), onunlock(password))}>
        <input type="password" placeholder={t("password")} bind:value={password} />
        <button type="submit" disabled={!password}>{t("unlock")}</button>
      </form>
      {#if item.code === "wrong_password"}<div class="p-error small">{errorText("wrong_password")}</div>{/if}
    {/if}

    {#if itemFields.length && editable && item.status === "ok"}
      <div class="ifields">
        {#each itemFields as f}
          <label title={f.help ? L(f.help) : undefined}>
            <span class="muted">{L(f.label)}</span>
            <input type="text" placeholder={L(f.placeholder)} bind:value={item.item[f.key]} spellcheck="false" />
          </label>
          {#if f.help}<span class="muted ihelp">{L(f.help)}</span>{/if}
        {/each}
      </div>
    {/if}

    {#if item.state === "running" || item.state === "waiting"}
      <div class="bar"><div style="width:{Math.round((item.progress ?? 0) * 100)}%"></div></div>
      <div class="stage">{item.state === "waiting" ? t("waiting") : `${stageText(item.stage)} · ${Math.round((item.progress ?? 0) * 100)}%`}</div>
    {/if}

    {#if item.state === "ok" && item.results?.length}
      {#each item.results.filter((r) => !r.skipped).slice(0, 3) as r}
        <div class="result">
          <i class="ti ti-circle-check"></i>
          <span class="rname" title={r.path}>{r.name}</span>
          <span class="muted rsize">{bytes(r.bytes)}</span>
          {#if manifest.id.includes("compress") && item.results.length === 1 && item.info.size && r.bytes}
            {@const p = percentChange(item.info.size, r.bytes)}
            {#if p > 0}<span class="save">−{p}%</span>{/if}
          {/if}
          <button class="icon" title={t("open")} onclick={() => api.openPath(r.path)}><i class="ti ti-external-link"></i></button>
          <button class="icon" title={t("showInFolder")} onclick={() => api.reveal(r.path)}><i class="ti ti-folder"></i></button>
        </div>
      {/each}
      {#if item.results.length > 3}
        <div class="muted small">+ {item.results.length - 3}</div>
      {/if}
      {#each item.results.filter((r) => r.originalKept) as r}
        <div class="p-warn small wline"><i class="ti ti-info-circle"></i> {t(r.originalKept === "no_recycle_bin" ? "origKeptNoBin" : "origKeptLocked")}</div>
      {/each}
    {/if}
    {#each item.warnings ?? [] as w}
      <div class="p-warn small wline"><i class="ti ti-alert-triangle"></i> {warnText(w)}</div>
    {/each}
    {#if item.state === "skipped"}
      <div class="p-muted small"><i class="ti ti-player-skip-forward"></i> {errorText(item.code, manifest.errors, item.details)}</div>
    {:else if item.state === "failed"}
      <div class="p-error small"><i class="ti ti-alert-circle"></i> {errorText(item.code, manifest.errors, item.details)}</div>
    {/if}
  </div>
  {#if editable}
    <button class="icon rm" title={t("remove")} onclick={onremove}><i class="ti ti-x"></i></button>
  {:else if item.state === "running"}
    <span class="spin"><i class="ti ti-loader-2"></i></span>
  {/if}
</div>

<style>
  .row {
    border: 1px solid var(--card-stroke);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin-bottom: 6px;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    background: var(--card);
  }
  .row.bad {
    border-color: color-mix(in srgb, var(--error-text) 35%, transparent);
  }
  .order {
    display: flex;
    flex-direction: column;
    margin: -2px -2px -2px -4px;
  }
  .order button {
    padding: 0 2px;
    font-size: 14px;
  }
  .ficon {
    font-size: 22px;
    margin-top: 2px;
  }
  .thumb {
    width: 36px;
    height: 36px;
    object-fit: cover;
    border-radius: 6px;
    background: var(--surface-2);
    flex: none;
  }
  .info {
    min-width: 0;
    flex: 1;
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    font-size: 12px;
    color: var(--text-2);
  }
  .small {
    font-size: 12px;
  }
  .p-warn {
    color: var(--warn-text);
  }
  .wline {
    display: flex;
    gap: 5px;
    align-items: center;
    margin-top: 2px;
  }
  .p-error {
    color: var(--error-text);
  }
  .p-muted {
    color: var(--text-3);
  }
  .pw {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 6px;
  }
  /* In a narrow window the button goes under the box rather than squeeze it. */
  .pw input {
    flex: 1 1 110px;
    min-width: 0;
    height: 30px;
  }
  .pw button {
    padding: 3px 10px;
  }
  .ifields {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 8px;
    margin-top: 6px;
  }
  .ihelp {
    font-size: 11px;
  }
  .ifields label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .ifields input {
    height: 28px;
    width: 120px;
    min-width: 0;
    flex: 1 1 120px;
  }
  .bar {
    height: 4px;
    background: var(--hover);
    border-radius: 2px;
    margin-top: 6px;
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent-fill);
    transition: width 0.2s;
  }
  .stage {
    font-size: 11px;
    color: var(--text-3);
    margin-top: 2px;
  }
  .result {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    margin-top: 4px;
  }
  .result .ti-circle-check {
    color: var(--ok-text);
  }
  /* The name keeps room to be read; size and buttons wrap under it when the row is narrow. */
  .rname {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1 1 90px;
  }
  /* The size and the saving stay whole; the name gives way. */
  .rsize,
  .save {
    white-space: nowrap;
    flex: none;
  }
  .save {
    color: var(--ok-text);
    font-weight: 600;
  }
  .rm {
    font-size: 14px;
  }
  .spin .ti {
    display: inline-block;
    animation: spin 1s linear infinite;
    color: var(--accent);
    font-size: 16px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
