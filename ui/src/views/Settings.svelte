<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { t, i18n, LANGS, type Key } from "../lib/i18n.svelte";
  import { app, saveSettings, checkForUpdate, menuNames } from "../lib/store.svelte";

  import { bytes } from "../lib/format";
  import Logo from "../components/Logo.svelte";
  import { downloads, downloadEngine, engineEvents, engineRemoved, ENGINE_NAMES, refreshEnv } from "../lib/engines.svelte";

  let info = $state<any>({});
  let engines = $state<any[]>([]);
  const load = async () => (engines = await api.engines());
  onMount(async () => {
    info = await api.appInfo();
  });
  // On opening, and whenever a download finishes or an engine is removed, here or on
  // another screen (an engine installed as another's requirement included).
  $effect(() => {
    void engineEvents.changed;
    load();
  });
  let checkMsg = $state("");
  async function checkNow() {
    checkMsg = "";
    const r = await checkForUpdate(true);
    checkMsg = r == null ? t("checkFailed") : r.available ? "" : t("upToDate");
  }
  async function toggleMenu() {
    await saveSettings();
    await api.shellMenu(app.settings.contextMenu, i18n.lang, menuNames()).catch(() => {});
  }
  /** Why a component couldn't be deleted, under its row. */
  let removeError = $state<Record<string, string>>({});
  async function remove(e: any) {
    removeError[e.name] = "";
    // A large one (an AI model, LibreOffice) is asked about: it takes long to get back.
    if ((e.installedBytes ?? 0) > 300_000_000) {
      const { ask } = await import("@tauri-apps/plugin-dialog");
      const label = ENGINE_NAMES[e.name] ?? e.name;
      if (!(await ask(t("removeBig", { name: label, size: bytes(e.installedBytes) }), { title: t("removeBigTitle"), kind: "warning" }))) return;
    }
    try {
      await api.engineRemove(e.name);
    } catch (err) {
      const k = `engineErr_${err}` as Key;
      removeError[e.name] = t(k) === k ? t("engineErr_other", { code: String(err) }) : t(k);
      return;
    }
    await refreshEnv();
    engineRemoved(e.name);
  }
  const get = (n: string) => downloadEngine(n);
  let copied = $state(false);
  async function copyDiagnostics() {
    await navigator.clipboard.writeText(await api.diagnostics());
    copied = true;
    setTimeout(() => (copied = false), 4000);
  }
  const installed = (e: any) => e.status === "installed" || downloads[e.name]?.installed === true;
</script>

<div class="page scroll">
  <h1>{t("settings")}</h1>
  <div class="grid">
    <label for="lang">{t("language")}</label>
    <select id="lang" bind:value={app.settings.language} onchange={saveSettings}>
      <option value="system">{t("langSystem")}</option>
      {#each LANGS as l}<option value={l.code}>{l.name}</option>{/each}
    </select>
    <label for="theme">{t("theme")}</label>
    <select id="theme" bind:value={app.settings.theme} onchange={saveSettings}>
      <option value="system">{t("themeSystem")}</option>
      <option value="light">{t("themeLight")}</option>
      <option value="dark">{t("themeDark")}</option>
    </select>
  </div>
  <label class="check"><input type="checkbox" bind:checked={app.settings.rememberLast} onchange={saveSettings} />{t("rememberLast")}</label>
  <label class="check"><input type="checkbox" bind:checked={app.settings.contextMenu} onchange={toggleMenu} />{t("contextMenu")}</label>
  <p class="hint muted small">{t("contextMenuHint")}</p>
  <label class="check"><input type="checkbox" bind:checked={app.settings.checkUpdates} onchange={saveSettings} />{t("checkUpdates")}</label>
  <p class="hint muted small">{t("checkUpdatesHint")} <button class="link" onclick={checkNow}>{t("checkNow")}</button> {checkMsg}</p>

  <h2>{t("components")}</h2>
  <p class="secondary small">{t("componentsHint")}</p>
  {#each engines as e}
    {@const d = downloads[e.name]}
    <div class="comp">
      <i class="ti ti-package"></i>
      <div class="cinfo">
        <div><b>{ENGINE_NAMES[e.name] ?? e.name}</b> {e.version} <span class="muted">· {e.license}</span></div>
        <div class="muted small">
          {#if d?.running}{bytes(d.done)} / {bytes(d.total || e.downloadBytes)}
          {:else if installed(e)}{t("installed")}{e.status === "installed" ? ` · ${bytes(e.installedBytes)}` : ""}
          {:else}{t("notInstalled")} · {bytes(e.downloadBytes)}{/if}
        </div>
      </div>
      {#if d?.running}
        <button class="small" onclick={() => api.engineCancel(e.name)}>{t("cancel")}</button>
      {:else if installed(e)}
        <button class="small" onclick={() => remove(e)}>{t("removeComponent")}</button>
      {:else}
        <button class="small" onclick={() => get(e.name)}>{t("engineDownload", { name: "" }).trim()}</button>
      {/if}
    </div>
    {#if removeError[e.name]}<p class="err small">{removeError[e.name]}</p>{/if}
  {/each}

  <h2>{t("about")}</h2>
  <p class="secondary small">{t("diagnosticsHint")}</p>
  <p>
    <button class="small" onclick={copyDiagnostics}><i class="ti ti-clipboard"></i> {t("copyDiagnostics")}</button>
    {#if copied}<span class="muted small" role="status"> {t("copiedDiagnostics")}</span>{/if}
  </p>
  <p class="about-app"><Logo size={36} /><span><b>{t("appName")}</b><br />{t("version", { v: info.version ?? "" })}</span></p>
  <p>
    {t("madeBy")}
    <button class="link" onclick={() => api.openUrl("https://softwarehive.ro")}>Software Hive S.R.L.</button>
  </p>
  <p class="secondary">{t("freeLine")}<br />{t("license")}</p>
  <div class="links">
    <button class="ghost" onclick={() => api.reveal(info.dataDir)}><i class="ti ti-folder"></i> {t("dataFolder")}</button>
    <button class="ghost" onclick={() => api.reveal(info.logsDir)}><i class="ti ti-file-text"></i> {t("logsFolder")}</button>
  </div>
</div>

<style>
  .page { padding: 24px 32px; flex: 1; min-height: 0; }
  /* The page scrolls across the whole width (its scrollbar at the window's edge); the text keeps a readable width. */
  .page > * { max-width: 556px; }
  h1 { font-size: 22px; margin: 0 0 16px; }
  h2 { font-size: 15px; margin: 26px 0 8px; }
  .about-app { display: flex; align-items: center; gap: 12px; }
  .grid { display: grid; grid-template-columns: 140px 240px; gap: 10px 14px; align-items: center; }
  .check { display: flex; gap: 10px; align-items: center; margin-top: 14px; }
  .hint { margin: 4px 0 0 27px; }
  p { margin: 4px 0; }
  .comp { display: flex; align-items: center; gap: 12px; border: 1px solid var(--card-stroke); background: var(--card); border-radius: var(--radius); padding: 12px 14px; margin-top: 8px; }
  .comp > .ti { font-size: 20px; color: var(--accent); }
  .cinfo { flex: 1; }
  .small { font-size: 12px; }
  .err { color: var(--error-text); margin: 4px 0 0 46px; }
  button.small { padding: 3px 12px; }
  .links { display: flex; gap: 8px; margin-top: 10px; }
  .links button { display: flex; gap: 6px; align-items: center; }
</style>
