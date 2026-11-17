<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Manifest, OutputBlock } from "../lib/api";
  import { L, t } from "../lib/i18n.svelte";

  let {
    output = $bindable(),
    manifest,
    sampleName,
    disabled = false,
  }: { output: OutputBlock; manifest: Manifest; sampleName: string; disabled?: boolean } = $props();

  let openPanel = $state(false);
  const hide = $derived(manifest.outputBlock?.hide ?? []);
  const replaceAllowed = $derived(!!manifest.output.replaceOriginalAllowed);
  const vars = $derived(["name", "date", ...(manifest.output.nameVariables ?? []).map((v) => v.id)]);

  const summary = $derived.by(() => {
    if (output.originals === "replace") return t("origReplace");
    if (output.outputDir === "subfolder") return t("saveSubfolder", { name: output.subfolderName.replace("{name}", stem(sampleName)) || L(manifest.name) });
    if (output.outputDir === "custom" && output.outputDirPath) return t("saveIn", { path: output.outputDirPath });
    return t("saveNextTo");
  });

  function stem(n: string) {
    const i = n.lastIndexOf(".");
    return i > 0 ? n.slice(0, i) : n;
  }

  const preview = $derived.by(() => {
    const ext = manifest.output.extensions[0] ?? "";
    let s = (output.nameTemplate || "{name}")
      .replaceAll("{name}", stem(sampleName) || "document")
      .replaceAll("{date}", new Date().toISOString().slice(0, 10))
      .replaceAll("{page}", "1")
      .replaceAll("{n}", "1")
      .replaceAll("{total}", "12")
      .replaceAll("{pages}", "12");
    return `${s}.${ext}`;
  });

  async function chooseFolder() {
    const d = await open({ directory: true });
    if (typeof d === "string") {
      output.outputDirPath = d;
      output.outputDir = "custom";
    }
  }
</script>

<div class="out">
  <div class="line">
    <i class="ti ti-folder"></i>
    <span class="sum" title={summary}>{summary}</span>
    <button class="link" {disabled} onclick={() => (openPanel = !openPanel)}>{openPanel ? t("close") : t("change")}</button>
  </div>

  {#if openPanel}
    <div class="panel">
      {#if output.originals !== "replace"}
        <p class="lbl">{t("where")}</p>
        <div class="radios">
          <label><input type="radio" bind:group={output.outputDir} value="sameAsInput" />{t("whereSame")}</label>
          <label>
            <input type="radio" bind:group={output.outputDir} value="subfolder" />{t("whereSub")}
            {#if output.outputDir === "subfolder"}
              <input type="text" class="sub" bind:value={output.subfolderName} placeholder={L(manifest.name)} />
            {/if}
          </label>
          <label>
            <input type="radio" bind:group={output.outputDir} value="custom" />{t("whereCustom")}
            <button class="small" onclick={chooseFolder}>{t("chooseFolder")}</button>
          </label>
          {#if output.outputDir === "custom" && output.outputDirPath}
            <div class="path muted" title={output.outputDirPath}>{output.outputDirPath}</div>
          {/if}
        </div>

        <p class="lbl">{t("nameLabel")}</p>
        <input type="text" class="tpl" bind:value={output.nameTemplate} spellcheck="false" />
        <div class="vars">
          {#each vars as v}
            <button class="var" onclick={() => (output.nameTemplate = (output.nameTemplate || "") + `{${v}}`)}>{`{${v}}`}</button>
          {/each}
        </div>
        <p class="muted small">{t("namePreview", { name: preview })}</p>

        <p class="lbl">{t("ifExists")}</p>
        <select bind:value={output.onExisting}>
          <option value="rename">{t("existsRename")}</option>
          <option value="overwrite">{t("existsOverwrite")}</option>
          <option value="skip">{t("existsSkip")}</option>
        </select>
      {/if}

      {#if replaceAllowed && !hide.includes("originals")}
        <p class="lbl">{t("originalLabel")}</p>
        <select bind:value={output.originals}>
          <option value="keep">{t("origKeep")}</option>
          <option value="replace">{t("origReplace")}</option>
        </select>
      {/if}

      <label class="check"><input type="checkbox" bind:checked={output.keepFileDate} />{t("keepDate")}</label>

      <p class="lbl">{t("whenDone")}</p>
      <select bind:value={output.whenDone}>
        <option value="nothing">{t("doneNothing")}</option>
        <option value="openFolder">{t("doneFolder")}</option>
        <option value="openFile">{t("doneFile")}</option>
      </select>
    </div>
  {/if}
</div>

<style>
  .out {
    margin-top: 14px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-2);
  }
  .line .ti {
    font-size: 16px;
  }
  .sum {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .panel {
    margin-top: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    background: var(--surface-2);
  }
  .panel .lbl {
    margin-top: 10px;
    font-size: 12px;
    color: var(--text-3);
  }
  .panel .lbl:first-child {
    margin-top: 0;
  }
  .panel select,
  .tpl {
    width: 100%;
  }
  .radios {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .radios label {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .radios input[type="radio"] {
    accent-color: var(--accent-fill);
    margin: 0;
  }
  .sub {
    flex: 1;
    height: 28px;
  }
  .small {
    padding: 2px 10px;
    font-size: 12px;
  }
  .path {
    font-size: 12px;
    padding-left: 22px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vars {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
    margin-top: 6px;
  }
  .var {
    font-size: 11px;
    padding: 1px 6px;
    font-family: Consolas, monospace;
    color: var(--text-2);
  }
  p.small {
    font-size: 12px;
    margin: 4px 0 0;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }
</style>
