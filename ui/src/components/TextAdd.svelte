<script lang="ts">
  // "Type or paste text…": for the tools that take text (text to speech, Markdown, HTML).
  // The text is saved as a file in Documents\MQKit, then added like any other file, so
  // the result has a place next to it.
  import { api, type FileInfo } from "../lib/api";
  import { t, L, type Key } from "../lib/i18n.svelte";

  let {
    ext,
    placeholder,
    startOpen = false,
    onadd,
  }: { ext: "txt" | "md" | "html"; placeholder?: { ro: string; en: string }; startOpen?: boolean; onadd: (files: FileInfo[]) => void } = $props();

  // svelte-ignore state_referenced_locally
  let open = $state(startOpen);
  let text = $state("");
  let busy = $state(false);
  let error = $state("");

  async function add() {
    busy = true;
    error = "";
    try {
      const f = await api.saveText(text, ext, t("typeTextName"));
      onadd([f]);
      text = "";
      open = false;
    } catch (e) {
      const k = `engineErr_${e}` as Key;
      error = t(k) === k ? String(e) : t(k);
    } finally {
      busy = false;
    }
  }
</script>

<button class="link" onclick={() => (open = !open)} aria-expanded={open}><i class="ti ti-keyboard"></i>{t("typeText")}</button>
{#if open}
  <div class="panel">
    <!-- svelte-ignore a11y_autofocus -->
    <textarea bind:value={text} rows="8" placeholder={placeholder ? L(placeholder) : t(`typeTextPh_${ext}` as Key)} spellcheck="true" autofocus aria-label={t("typeText")}></textarea>
    <div class="row">
      <span class="muted small">{t("typeTextHint")}</span>
      <button class="primary small" disabled={!text.trim() || busy} onclick={add}>{t("typeTextAdd")}</button>
    </div>
    {#if error}<p class="err small">{error}</p>{/if}
  </div>
{/if}

<style>
  .link { display: flex; align-items: center; gap: 6px; background: none; border: none; color: var(--accent); padding: 6px 2px; cursor: pointer; font-size: 13px; }
  .panel { margin-top: 8px; display: flex; flex-direction: column; gap: 8px; width: 100%; }
  textarea { width: 100%; min-height: 140px; resize: vertical; font: inherit; padding: 8px 10px; border-radius: var(--radius); border: 1px solid var(--control-stroke, var(--border)); background: var(--layer, var(--surface-1)); color: var(--text); box-sizing: border-box; }
  .row { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  .small { font-size: 12px; }
  button.small { padding: 4px 14px; }
  .err { color: var(--error-text); margin: 0; }
</style>
