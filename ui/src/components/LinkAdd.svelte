<script lang="ts">
  // "From a link…": paste links (one per line); each file is downloaded into the
  // Downloads folder, like a browser would, then added. Share links of Google Drive,
  // Dropbox and GitHub work too.
  import { api, type FileInfo } from "../lib/api";
  import { L } from "../lib/i18n.svelte";
  import { bytes } from "../lib/format";

  let { extensions, anyFile = false, multiple = true, onadd }: { extensions: string[]; anyFile?: boolean; multiple?: boolean; onadd: (files: FileInfo[]) => void } = $props();

  type Row = { id: string; url: string; state: "wait" | "run" | "done" | "error" | "cancelled"; done: number; total: number; error?: string; name?: string };

  let open = $state(false);
  let text = $state("");
  let rows = $state<Row[]>([]);
  let n = 0;

  const links = $derived(
    [...new Set(text.split(/\s+/).map((s) => s.trim()).filter(Boolean))].slice(0, multiple ? 100 : 1),
  );
  const invalid = $derived(links.filter((u) => !/^https?:\/\/[^\s/?#]+/i.test(u)));
  const running = $derived(rows.some((r) => r.state === "wait" || r.state === "run"));

  function message(code: string, detail: string): string {
    switch (code) {
      case "bad_link": return L({ ro: "Nu e un link http(s).", en: "Not an http(s) link." });
      case "network": return L({ ro: "Nu s-a putut descărca (conexiunea).", en: "Couldn't download (connection)." });
      case "http":
        return detail === "404"
          ? L({ ro: "Fișierul nu există la acest link (404).", en: "There's no file at this link (404)." })
          : detail === "401" || detail === "403"
            ? L({ ro: `Fișierul nu e public (${detail}).`, en: `The file isn't public (${detail}).` })
            : L({ ro: `Serverul a răspuns cu eroarea ${detail}.`, en: `The server answered with error ${detail}.` });
      case "page_not_file": return L({ ro: "Linkul deschide o pagină, nu un fișier. Copiază linkul de descărcare.", en: "The link opens a web page, not a file. Copy the download link." });
      case "wrong_type":
        return detail
          ? L({ ro: `Fișierul e .${detail}, nu e pentru această unealtă.`, en: `The file is .${detail}, not for this tool.` })
          : L({ ro: "Fișierul nu e pentru această unealtă.", en: "The file isn't for this tool." });
      case "too_big": return L({ ro: "Fișierul e prea mare.", en: "The file is too big." });
      case "disk_full": return L({ ro: "Nu e destul loc pe disc.", en: "Not enough disk space." });
      case "permission_denied": return L({ ro: "Nu se poate scrie în folderul Descărcări.", en: "Can't write to the Downloads folder." });
      case "empty": return L({ ro: "Fișierul e gol.", en: "The file is empty." });
      case "cancelled": return L({ ro: "Oprit.", en: "Stopped." });
      default: return code;
    }
  }

  function one(row: Row): Promise<void> {
    return new Promise((resolve) => {
      row.state = "run";
      api
        .linkDownload(row.id, row.url, anyFile ? [] : extensions, (e) => {
          if (e.type === "progress") {
            row.done = e.done;
            row.total = e.total;
          } else if (e.type === "done") {
            row.state = "done";
            row.name = e.file.name;
            onadd([e.file]);
            resolve();
          } else if (e.type === "error") {
            row.state = e.code === "cancelled" ? "cancelled" : "error";
            row.error = message(e.code, e.detail ?? "");
            resolve();
          }
        })
        .catch((err) => {
          row.state = "error";
          row.error = String(err);
          resolve();
        });
    });
  }

  async function start() {
    const fresh: Row[] = links.map((url) => ({ id: `link-${Date.now()}-${n++}`, url, state: "wait", done: 0, total: 0 }));
    rows = fresh;
    text = "";
    // Two at a time: quick for several small files, fair to slow servers.
    const queue = [...rows];
    const worker = async () => {
      for (let r = queue.shift(); r; r = queue.shift()) {
        if (r.state === "wait") await one(r);
      }
    };
    await Promise.all([worker(), worker()]);
  }

  function stop(row: Row) {
    if (row.state === "wait") row.state = "cancelled";
    else if (row.state === "run") api.linkCancel(row.id);
  }

  function close() {
    rows.forEach(stop);
    open = false;
    rows = [];
  }

  const short = (u: string) => u.replace(/^https?:\/\//i, "").replace(/^www\./i, "");
</script>

{#if !open}
  <button class="link" onclick={() => (open = true)}><i class="ti ti-link"></i>{L({ ro: "Dintr-un link…", en: "From a link…" })}</button>
{:else}
  <div class="panel">
    <p class="lbl">{multiple ? L({ ro: "Linkurile, câte unul pe rând", en: "The links, one per line" }) : L({ ro: "Linkul", en: "The link" })}</p>
    <textarea class="text mono" rows={multiple ? 3 : 1} bind:value={text} placeholder="https://…" spellcheck="false"></textarea>
    <p class="help">
      {L({
        ro: "Fișierele se descarcă în folderul Descărcări, apoi se adaugă. Merg și linkurile de partajare Google Drive, Dropbox și GitHub. Se trimite doar cererea pentru link.",
        en: "The files are downloaded into your Downloads folder, then added. Google Drive, Dropbox and GitHub share links work too. Only the request for the link is sent.",
      })}
    </p>
    {#if invalid.length}
      <p class="status"><span class="err">{L({ ro: `Nu e un link http(s): ${invalid[0]}`, en: `Not an http(s) link: ${invalid[0]}` })}</span></p>
    {/if}
    {#if rows.length}
      <ul class="rows">
        {#each rows as r (r.id)}
          <li>
            <span class="name" title={r.url}>
              {#if r.state === "done"}<i class="ti ti-check ok"></i>{:else if r.state === "error"}<i class="ti ti-alert-circle bad"></i>{:else if r.state === "run"}<i class="ti ti-loader-2 spin"></i>{:else if r.state === "cancelled"}<i class="ti ti-player-stop"></i>{:else}<i class="ti ti-clock"></i>{/if}
              {r.name ?? short(r.url)}
            </span>
            <span class="meta">
              {#if r.state === "run"}
                {r.total ? `${bytes(r.done)} / ${bytes(r.total)}` : bytes(r.done)}
              {:else if r.state === "error" || r.state === "cancelled"}
                <span class={r.state === "error" ? "err" : "secondary"}>{r.error ?? L({ ro: "Oprit.", en: "Stopped." })}</span>
              {:else if r.state === "done"}
                <span class="secondary">{L({ ro: "Adăugat", en: "Added" })}</span>
              {/if}
            </span>
            {#if r.state === "run" || r.state === "wait"}
              <button class="icon" title={L({ ro: "Oprește", en: "Stop" })} aria-label={L({ ro: "Oprește", en: "Stop" })} onclick={() => stop(r)}><i class="ti ti-x"></i></button>
            {/if}
            {#if r.state === "run" && r.total}
              <div class="bar"><div style="width: {Math.min(100, (r.done / r.total) * 100)}%"></div></div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    <div class="row end">
      <button class="ghost" onclick={close}>{running ? L({ ro: "Oprește tot", en: "Stop all" }) : L({ ro: "Închide", en: "Close" })}</button>
      <button class="primary" disabled={!links.length || invalid.length > 0 || running} onclick={start}>
        {links.length > 1 ? L({ ro: `Descarcă ${links.length}`, en: `Download ${links.length}` }) : L({ ro: "Descarcă", en: "Download" })}
      </button>
    </div>
  </div>
{/if}

<style>
  .link { display: flex; align-items: center; gap: 6px; background: none; border: none; color: var(--accent); padding: 6px 2px; cursor: pointer; font-size: 13px; }
  .panel { border: 1px solid var(--border); border-radius: var(--radius); padding: 10px 12px; margin: 6px 0 10px; background: var(--surface-1); }
  .row { display: flex; gap: 6px; align-items: center; }
  .row.end { justify-content: flex-end; margin-top: 8px; }
  .text { width: 100%; resize: vertical; box-sizing: border-box; }
  .mono { font-family: Consolas, monospace; }
  .lbl { margin: 8px 0 4px; font-weight: 600; font-size: 12px; }
  .help { font-size: 11px; color: var(--text-secondary); margin: 4px 0 6px; }
  .status { font-size: 13px; margin: 6px 0 0; }
  .rows { list-style: none; margin: 8px 0 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .rows li { display: grid; grid-template-columns: 1fr auto auto; gap: 8px; align-items: center; font-size: 13px; }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; display: flex; align-items: center; gap: 6px; min-width: 0; }
  .meta { font-size: 12px; color: var(--text-secondary); text-align: right; }
  .bar { grid-column: 1 / -1; height: 4px; border-radius: 2px; background: var(--hover); overflow: hidden; }
  .bar div { height: 100%; background: var(--accent-fill); transition: width 0.2s; }
  .icon { background: none; border: none; padding: 2px 4px; cursor: pointer; color: var(--text-secondary); }
  .ok { color: var(--success-text, var(--accent)); }
  .bad, .err { color: var(--error-text); }
  .secondary { color: var(--text-secondary); }
  .spin { display: inline-block; animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
