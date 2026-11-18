<script lang="ts">
  // "From a folder…": every file under a folder that matches a pattern (`*.pdf`,
  // `docs/**/*.md`, `*.{jpg,png}`), counted live before anything is added.
  import { api, type FileInfo } from "../lib/api";
  import { L } from "../lib/i18n.svelte";
  import { bytes } from "../lib/format";

  let { extensions, anyFile = false, onadd }: { extensions: string[]; anyFile?: boolean; onadd: (files: FileInfo[]) => void } = $props();

  const defaultPattern = () => (anyFile || !extensions.length ? "*" : extensions.length === 1 ? `*.${extensions[0]}` : `*.{${extensions.join(",")}}`);

  let open = $state(false);
  let folder = $state("");
  let pattern = $state(defaultPattern());
  let recursive = $state(true);
  let hidden = $state(false);
  let busy = $state(false);
  let error = $state("");
  let result = $state<{ files: FileInfo[]; matched: number; bytes: number; truncated: boolean } | null>(null);
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function browse() {
    const { open: dialog } = await import("@tauri-apps/plugin-dialog");
    const picked = await dialog({ directory: true, multiple: false });
    if (typeof picked === "string") {
      folder = picked;
      search();
    }
  }

  /** Searches again a moment after the last change; an older answer never replaces a newer one. */
  function search() {
    clearTimeout(timer);
    result = null;
    error = "";
    if (!folder.trim()) return;
    timer = setTimeout(async () => {
      const mine = ++seq;
      busy = true;
      try {
        const r = await api.findFiles(folder.trim(), pattern, recursive, hidden, anyFile ? [] : extensions);
        if (mine === seq) result = r;
      } catch (e) {
        if (mine === seq) error = String(e) === "not_a_folder" ? L({ ro: "Dosarul nu există.", en: "The folder doesn't exist." }) : String(e);
      } finally {
        if (mine === seq) busy = false;
      }
    }, 300);
  }

  function add() {
    if (result?.files.length) onadd(result.files);
    open = false;
    result = null;
  }
</script>

{#if !open}
  <button class="link" onclick={() => (open = true)}><i class="ti ti-folder-search"></i>{L({ ro: "Dintr-un folder, după un tipar…", en: "From a folder, by a pattern…" })}</button>
{:else}
  <div class="panel">
    <p class="lbl">{L({ ro: "Dosarul", en: "The folder" })}</p>
    <div class="row">
      <input class="text" bind:value={folder} oninput={search} placeholder="D:\Documente\Proiect" spellcheck="false" />
      <button onclick={browse}>{L({ ro: "Alege…", en: "Choose…" })}</button>
    </div>
    <p class="lbl">{L({ ro: "Tiparul", en: "The pattern" })}</p>
    <input class="text mono" bind:value={pattern} oninput={search} spellcheck="false" />
    <p class="help">
      {L({
        ro: "* orice nume, ** orice subfoldere, {a,b} una din variante, ; între mai multe. Ex.: *.pdf · rapoarte/**/*.md · factura-2026*.{pdf,docx}",
        en: "* any name, ** any subfolders, {a,b} one of them, ; between several. E.g. *.pdf · reports/**/*.md · invoice-2026*.{pdf,docx}",
      })}
    </p>
    <label class="toggle"><input type="checkbox" bind:checked={recursive} onchange={search} /> {L({ ro: "Și din subfoldere", en: "Subfolders too" })}</label>
    <label class="toggle"><input type="checkbox" bind:checked={hidden} onchange={search} /> {L({ ro: "Și din folderele ascunse", en: "Hidden folders too" })}</label>
    <p class="status">
      {#if busy}
        <i class="ti ti-loader-2 spin"></i> {L({ ro: "Caut…", en: "Searching…" })}
      {:else if error}
        <span class="err">{error}</span>
      {:else if result}
        {result.files.length
          ? L({ ro: `${result.files.length} fișiere, ${bytes(result.bytes)}`, en: `${result.files.length} files, ${bytes(result.bytes)}` })
          : L({ ro: "Niciun fișier potrivit.", en: "No matching files." })}
        {#if result.matched > result.files.length && !result.truncated}
          <br /><span class="secondary">{L({ ro: `Alte ${result.matched - result.files.length} se potrivesc tiparului, dar nu sunt pentru această unealtă.`, en: `${result.matched - result.files.length} more match the pattern but aren't for this tool.` })}</span>
        {/if}
        {#if result.truncated}
          <br /><span class="secondary">{L({ ro: "Căutarea s-a oprit (prea multe fișiere); alege un folder sau un tipar mai strâns.", en: "The search stopped (too many files); choose a narrower folder or pattern." })}</span>
        {/if}
      {/if}
    </p>
    <div class="row end">
      <button class="ghost" onclick={() => (open = false)}>{L({ ro: "Renunță", en: "Cancel" })}</button>
      <button class="primary" disabled={!result?.files.length || busy} onclick={add}>
        {L({ ro: `Adaugă ${result?.files.length ?? 0}`, en: `Add ${result?.files.length ?? 0}` })}
      </button>
    </div>
  </div>
{/if}

<style>
  .link { display: flex; align-items: center; gap: 6px; background: none; border: none; color: var(--accent); padding: 6px 2px; cursor: pointer; font-size: 13px; }
  .panel { border: 1px solid var(--border); border-radius: var(--radius); padding: 10px 12px; margin: 6px 0 10px; background: var(--surface-1); }
  .row { display: flex; gap: 6px; align-items: center; }
  .row .text { flex: 1; min-width: 0; }
  .row.end { justify-content: flex-end; margin-top: 8px; }
  .text { width: 100%; }
  .mono { font-family: Consolas, monospace; }
  .lbl { margin: 8px 0 4px; font-weight: 600; font-size: 12px; }
  .help { font-size: 11px; color: var(--text-secondary); margin: 4px 0 6px; }
  .toggle { display: flex; align-items: center; gap: 8px; font-size: 13px; padding: 2px 0; cursor: pointer; }
  .status { font-size: 13px; margin: 8px 0 0; min-height: 18px; }
  .err { color: var(--error-text); }
  .secondary { color: var(--text-secondary); font-size: 12px; }
  .spin { display: inline-block; animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
