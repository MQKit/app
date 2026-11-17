<script lang="ts">
  import { toolNumber } from "../lib/numbers";
  import { onMount, untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type Field, type FileInfo, type OutputBlock } from "../lib/api";
  import { L, t, actionLabel, errorText, fileCount, i18n } from "../lib/i18n.svelte";
  import { app, go, tool, toggleFavorite, rememberValues, toolColor } from "../lib/store.svelte";
  import { evalCond, resolveDefault, type CondCtx } from "../lib/cond";
  import { bytes, percentChange } from "../lib/format";
  import { newId, type Item } from "../lib/item";
  import FieldView from "../components/Field.svelte";
  import FileRow from "../components/FileRow.svelte";
  import OutputPanel from "../components/OutputPanel.svelte";
  import EngineNotice from "../components/EngineNotice.svelte";
  import FolderAdd from "../components/FolderAdd.svelte";
  import LinkAdd from "../components/LinkAdd.svelte";
  import BeforeAfter from "../components/BeforeAfter.svelte";
  import { downloads, downloadEngine, ENGINE_NAMES } from "../lib/engines.svelte";

  let {
    id,
    initialFiles,
    onDrop = $bindable(),
  }: { id: string; initialFiles: FileInfo[]; onDrop: ((paths: string[]) => void) | null } = $props();

  // App.svelte recreates this view (a {#key}) when the tool changes: id is read once.
  // svelte-ignore state_referenced_locally
  const m = tool(id)!;
  const fields = m.fields ?? [];
  const presets = m.presets ?? [];
  const combined = m.input.mode === "combined";

  let items = $state<Item[]>([]);
  let values = $state<Record<string, any>>({});
  let touched = new Set<string>();
  let advOpen = $state(false);
  let phase = $state<"edit" | "running" | "done">("edit");
  let estimate = $state<any>(null);
  let confirmed = $state<Record<string, boolean>>({});
  let jobId = $state("");
  let cancelling = $state(false);
  let summary = $state<any>(null);
  let jobError = $state<any>(null);
  let combinedResults = $state<any[]>([]);
  const passwords: Record<string, string> = {};

  // ---------- defaults, presets, remembered values ----------
  function locDefault(v: any) {
    return v && typeof v === "object" && ("ro" in v || "en" in v) ? L(v) : v;
  }
  function defaultOutput(): OutputBlock {
    const d = m.outputBlock?.defaults ?? {};
    return {
      outputDir: d.outputDir ?? "sameAsInput",
      subfolderName: locDefault(d.subfolderName) ?? L(m.name),
      outputDirPath: "",
      nameTemplate: locDefault(d.nameTemplate) ?? "{name}",
      onExisting: "rename",
      originals: "keep",
      keepFileDate: false,
      whenDone: "nothing",
    };
  }
  let output = $state<OutputBlock>(defaultOutput());

  const baseCtx: CondCtx = $derived({
    values,
    files: items.filter((i) => i.status === "ok" || i.status === "failed").map((i) => i.props),
    batch: {
      count: items.length,
      totalBytes: items.reduce((a, i) => a + i.info.size, 0),
      extensions: [...new Set(items.map((i) => i.info.ext))],
    },
    env: app.env,
  });
  /** The entry that will run (contract 5.2), exposed to conditions as env.resolvedEngine. */
  const resolvedEngine = $derived.by(() => {
    if (!m.entry?.length) return "default";
    const e = m.entry.find((x) => evalCond(x.when, baseCtx));
    return e ? (e.id ?? "default") : null;
  });
  const ctx: CondCtx = $derived({ ...baseCtx, env: { ...app.env, resolvedEngine } });

  function defaults(): Record<string, any> {
    const v: Record<string, any> = {};
    for (const f of fields) v[f.key] = resolveDefault(f.default, { ...ctx, values: v });
    return v;
  }

  function applyPreset(pid: string) {
    const p = presets.find((x) => x.id === pid);
    values = { ...defaults(), ...(p?.values ?? {}) };
    touched.clear();
  }

  const currentPreset = $derived(
    presets.find((p) => Object.entries(p.values).every(([k, v]) => values[k] === v))?.id ?? null,
  );

  function resetDefaults() {
    const def = presets.find((p) => p.default);
    if (def) applyPreset(def.id);
    else {
      values = defaults();
      touched.clear();
    }
  }

  onMount(() => {
    const last = app.settings.rememberLast ? app.lastValues[id] : null;
    resetDefaults();
    if (last?.values) {
      for (const f of fields) if (f.key in last.values) values[f.key] = last.values[f.key];
    }
    if (last?.output) {
      // A name that is only some language's default follows the current language
      // ("{name}_comprimat" becomes "{name}_compressed" in English); one the user typed stays.
      const d = m.outputBlock?.defaults ?? {};
      const isDefault = (saved: any, def: any) => def && typeof def === "object" && Object.values(def).includes(saved);
      const o = { ...last.output };
      if (isDefault(o.nameTemplate, d.nameTemplate)) delete o.nameTemplate;
      if (isDefault(o.subfolderName, d.subfolderName ?? m.name)) delete o.subfolderName;
      output = { ...defaultOutput(), ...o, originals: "keep" };
    }
    if (initialFiles.length) addFiles(initialFiles);
    api.engines().then((l) => (engineInfo = l));
    onDrop = async (paths: string[]) => {
      if (phase === "running") return;
      if (phase === "done") resetRun(true);
      addFiles(await api.expand(paths));
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && phase !== "running") go({ name: "home" });
      if (e.key === "o" && e.ctrlKey && phase === "edit") pick();
    };
    window.addEventListener("keydown", key);
    return () => {
      onDrop = null;
      window.removeEventListener("keydown", key);
    };
  });

  // Conditional defaults follow the analysis, for fields the user did not touch.
  $effect(() => {
    const c = ctx;
    untrack(() => {
      for (const f of fields) {
        if (Array.isArray(f.default) && !touched.has(f.key) && !currentPreset) {
          const v = resolveDefault(f.default, c);
          if (v !== undefined && values[f.key] !== v) values[f.key] = v;
        }
      }
    });
  });

  function touch(key: string) {
    touched.add(key);
  }

  // ---------- files ----------
  async function addFiles(list: FileInfo[]) {
    const exts = m.input.extensions;
    const known = new Set(items.map((i) => i.info.path.toLowerCase()));
    const fresh: Item[] = [];
    for (const f of list) {
      if (known.has(f.path.toLowerCase())) continue;
      if (!m.input.anyFile && !exts.includes(f.ext)) continue;
      if (!m.input.multiple && items.length + fresh.length >= 1) break;
      const itemDefaults: Record<string, any> = {};
      for (const fld of m.input.itemFields ?? []) itemDefaults[fld.key] = fld.default === "all" ? "" : (fld.default ?? "");
      fresh.push({ id: newId(), info: f, status: "analyzing", props: {}, item: itemDefaults });
    }
    const skippedCount = list.length - fresh.length;
    skippedNotice = skippedCount > 0 && list.length > 0 ? skippedCount : 0;
    if (!fresh.length) return;
    items = [...items, ...fresh];
    await analyze(fresh.map((f) => f.id));
  }
  let skippedNotice = $state(0);

  async function analyze(ids: string[]) {
    const targets = items.filter((i) => ids.includes(i.id));
    try {
      const res = await api.analyze(
        id,
        targets.map((i) => ({ id: i.id, path: i.info.path })),
        Object.fromEntries(ids.filter((x) => passwords[x]).map((x) => [x, passwords[x]])),
      );
      for (const r of res) {
        const it = items.find((i) => i.id === r.file);
        if (!it) continue;
        it.props = { ...(r.props ?? {}), sizeBytes: it.info.size, extension: it.info.ext };
        // A damaged file reads the same in every tool, whichever status its analysis gave.
        const corrupt = r.status === "corrupt" || r.code === "corrupt_file";
        it.status = r.status === "ok" ? "ok" : r.status === "locked" ? "locked" : r.status === "unsupported" ? "unsupported" : corrupt ? "corrupt" : "failed";
        it.code = r.code;
      }
    } catch (e) {
      for (const it of targets) {
        it.status = "failed";
        it.code = "engine_missing";
      }
    }
  }

  async function unlock(it: Item, pw: string) {
    passwords[it.id] = pw;
    it.status = "analyzing";
    await analyze([it.id]);
    if ((it.status as string) === "locked") {
      it.code = "wrong_password";
      delete passwords[it.id];
    }
  }

  function remove(it: Item) {
    items = items.filter((i) => i !== it);
    delete passwords[it.id];
  }

  function move(index: number, delta: number) {
    const j = index + delta;
    if (j < 0 || j >= items.length) return;
    const copy = [...items];
    [copy[index], copy[j]] = [copy[j], copy[index]];
    items = copy;
  }

  async function pick() {
    const sel = await open({
      multiple: m.input.multiple !== false,
      filters: m.input.anyFile ? [] : [{ name: L(m.name), extensions: m.input.extensions }],
    });
    if (!sel) return;
    addFiles(await api.expand(Array.isArray(sel) ? sel : [sel]));
  }

  // ---------- form ----------
  const visible = (f: Field) => evalCond(f.showIf, ctx);
  const basic = $derived(fields.filter((f) => !f.advanced && visible(f)));
  const advancedGroups = $derived.by(() => {
    const adv = fields.filter((f) => f.advanced && visible(f));
    const groups = [...(m.groups ?? [])].sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
    const out = groups.map((g) => ({ label: L(g.label), fields: adv.filter((f) => f.group === g.id) })).filter((g) => g.fields.length);
    const loose = adv.filter((f) => !f.group || !groups.some((g) => g.id === f.group));
    if (loose.length) out.push({ label: "", fields: loose });
    return out;
  });

  /** Only the visible plugin fields go to the plugin (contract 11.1). */
  function finalOptions() {
    const o: Record<string, any> = {};
    for (const f of fields) if (visible(f)) o[f.key] = values[f.key];
    return o;
  }

  const activeRules = $derived((m.rules ?? []).filter((r) => evalCond(r.when, ctx)));
  const errorsActive = $derived(activeRules.some((r) => r.severity === "error"));
  const needsConfirm = $derived(activeRules.filter((r) => r.severity === "confirm"));
  const okItems = $derived(items.filter((i) => i.status === "ok"));
  const notOk = $derived(items.filter((i) => i.status !== "ok" && i.status !== "analyzing"));
  const analyzing = $derived(items.some((i) => i.status === "analyzing"));
  const minFiles = $derived(m.input.minFiles ?? 1);

  // environment problems
  let engineInfo = $state<any[]>([]);
  const requiredEngines = $derived(
    (m.requires ?? []).filter((r) => r.engine && evalCond(r.when, ctx)).map((r) => r.engine) as string[],
  );
  const engineStatus = (n: string) => app.env[`engines.${n}`] ?? "unavailable";
  const toDownload = $derived(requiredEngines.filter((n) => engineStatus(n) === "downloadable"));
  const engineMissing = $derived(requiredEngines.some((n) => engineStatus(n) !== "installed"));
  const engineUnavailable = $derived(requiredEngines.some((n) => engineStatus(n) === "unavailable"));
  const engineBusy = $derived(toDownload.some((n) => downloads[n]?.running));

  /** After an engine arrives, files that could not be read are analysed again. */
  async function engineReady() {
    const again = items.filter((i) => i.status === "failed");
    for (const i of again) i.status = "analyzing";
    if (again.length) await analyze(again.map((i) => i.id));
  }

  async function downloadThenStart() {
    for (const n of toDownload) if (!(await downloadEngine(n))) return;
    await engineReady();
    start();
  }
  const wordMissing = $derived(resolvedEngine === null);
  const heifMissing = false;

  const readyExceptEngines = $derived(
    phase === "edit" &&
      !analyzing &&
      okItems.length >= minFiles &&
      !errorsActive &&
      !wordMissing &&
      needsConfirm.every((r) => confirmed[r.id]),
  );
  const canStart = $derived(readyExceptEngines && !engineMissing);
  /** Files that failed only because the engine is missing still count for the button. */
  const waitingForEngine = $derived(engineMissing && !engineUnavailable && items.length > 0 && !engineBusy);

  const startLabel = $derived(
    okItems.length === 0
      ? t(items.some((i) => i.status === "locked") ? "needUnlock" : "needFiles")
      : okItems.length < minFiles
        ? t("needMinFiles", { n: minFiles })
        : actionLabel(id, okItems.length, combined && !values.onePdfPerImage),
  );

  // ---------- estimate ----------
  let estTimer: ReturnType<typeof setTimeout> | undefined;
  let estSeq = 0;
  $effect(() => {
    if (!m.ui?.estimate || phase !== "edit") return;
    const opts = finalOptions();
    const files = okItems.map((i) => ({ id: i.id, path: i.info.path, analysis: $state.snapshot(i.props) }));
    clearTimeout(estTimer);
    if (!files.length) {
      estimate = null;
      return;
    }
    const seq = ++estSeq;
    estTimer = setTimeout(async () => {
      try {
        const e = await api.estimate(id, files, opts);
        if (seq === estSeq) estimate = e;
      } catch {
        if (seq === estSeq) estimate = null;
      }
    }, 350);
  });

  const totalIn = $derived(okItems.reduce((a, i) => a + i.info.size, 0));

  // ---------- run ----------
  async function start() {
    if (!canStart) return;
    phase = "running";
    cancelling = false;
    summary = null;
    jobError = null;
    combinedResults = [];
    const run = okItems;
    for (const i of items) {
      i.state = i.status === "ok" ? "waiting" : undefined;
      i.progress = 0;
      i.results = undefined;
      i.suggest = undefined;
      // Each run says its own warnings (an earlier run's "no animation" isn't this one's).
      i.warnings = undefined;
      i.warningDetails = undefined;
    }
    const secret = new Set(m.fields.filter((f) => f.type === "password").map((f) => f.key));
    const remembered = Object.fromEntries(Object.entries($state.snapshot(values)).filter(([k]) => !secret.has(k)));
    await rememberValues(id, remembered, { ...$state.snapshot(output), originals: "keep" });
    const job = {
      tool: id,
      toolName: L(m.name),
      locale: i18n.lang === "ro" ? "ro-RO" : "en-US",
      files: run.map((i) => ({ id: i.id, path: i.info.path, analysis: $state.snapshot(i.props), item: cleanItem(i.item), base: i.info.base ?? null })),
      options: finalOptions(),
      output: $state.snapshot(output),
      passwords: Object.fromEntries(run.filter((i) => passwords[i.id]).map((i) => [i.id, passwords[i.id]])),
    };
    try {
      jobId = await api.startJob(job, onEvent);
    } catch (e) {
      jobError = { code: "engine_failed", details: { message: String(e) } };
      phase = "done";
    }
  }

  function cleanItem(it: Record<string, any>) {
    const o: Record<string, any> = {};
    for (const [k, v] of Object.entries(it)) o[k] = v === "" ? "all" : v;
    return o;
  }

  function onEvent(e: any) {
    const target = (f: string) => (f === "*" ? items.filter((i) => i.state) : items.filter((i) => i.id === f));
    switch (e.type) {
      case "fileStart":
        for (const i of target(e.file)) i.state = "running";
        break;
      case "progress":
        for (const i of target(e.file)) {
          i.progress = e.fraction;
          i.stage = e.stageCode;
        }
        break;
      case "warning":
        for (const i of target(e.file ?? "*")) {
          if (!(i.warnings ??= []).includes(e.code)) i.warnings.push(e.code);
          if (e.details) (i.warningDetails ??= {})[e.code] = e.details;
        }
        break;
      case "fileDone":
        if (e.file === "*") {
          combinedResults = e.results ?? [];
          for (const i of target("*")) {
            i.state = e.status;
            i.code = e.code;
            i.progress = 1;
          }
          if (e.status !== "ok") jobError = { code: e.code, details: e.details };
        } else {
          for (const i of target(e.file)) {
            i.state = e.status;
            i.code = e.code;
            i.details = e.details;
            i.results = e.results;
            i.suggest = e.suggest?.options;
            i.progress = 1;
          }
        }
        break;
      case "done":
        summary = e.summary;
        if (e.error) jobError = e.error;
        if (e.cancelled) for (const i of items) if (i.state === "waiting" || i.state === "running") ((i.state = "failed"), (i.code = "cancelled"));
        phase = "done";
        afterDone(e);
        break;
    }
  }

  function allResults() {
    return [...combinedResults, ...items.flatMap((i) => i.results ?? [])].filter((r) => !r.skipped);
  }

  function afterDone(e: any) {
    const res = allResults();
    if (e.cancelled || !res.length) return;
    if (output.whenDone === "openFolder") api.reveal(res[0].path);
    if (output.whenDone === "openFile") api.openPath(res[0].path);
  }

  function cancel() {
    cancelling = true;
    api.cancelJob(jobId);
  }

  function resetRun(clear = false) {
    phase = "edit";
    summary = null;
    jobError = null;
    combinedResults = [];
    if (clear) items = [];
    for (const i of items) {
      i.state = undefined;
      i.results = undefined;
      i.progress = 0;
    }
  }

  function retrySuggested() {
    const failed = items.filter((i) => i.state === "failed" && i.suggest);
    const sug = failed[0]?.suggest ?? {};
    values = { ...values, ...sug };
    items = failed;
    resetRun();
  }

  const overall = $derived.by(() => {
    const run = items.filter((i) => i.state);
    if (!run.length) return 0;
    const total = run.reduce((a, i) => a + Math.max(i.info.size, 1), 0);
    return run.reduce((a, i) => a + Math.max(i.info.size, 1) * (i.state === "running" || i.state === "waiting" ? (i.progress ?? 0) : 1), 0) / total;
  });

  /** A list whose choices don't fit half the pane ("Pe capitole (după cuprins)") takes the whole row. */
  function longOptions(f: any): boolean {
    return f.type === "select" && f.display !== "chips" && (f.options ?? []).some((o: any) => (L(o.label) ?? "").length > 18);
  }
  const allSkipped = $derived(!!summary && !summary.ok && !summary.failed && summary.skipped > 0 && !jobError);
  const doneTitle = $derived.by(() => {
    if (!summary) return t("failedTitle");
    if (items.some((i) => i.code === "cancelled")) return t("cancelledTitle");
    if (jobError && !summary.ok) return t("failedTitle");
    // Every file skipped, none failed: already small enough, already optimal…
    if (allSkipped) return t("nothingTitle");
    // A skipped file isn't a problem (already optimal, the subtitle used for its video…):
    // each row says why.
    if (summary.failed) return summary.ok ? t("partialTitle") : t("failedTitle");
    return t("doneTitle");
  });
  const suggestAvailable = $derived(items.some((i) => i.state === "failed" && i.suggest));
  const isFav = $derived(app.favorites.includes(id));
  /** The formats the tool takes, for the empty drop zone ("PDF", "JPG, PNG, HEIC…"). */
  const acceptsList = $derived.by(() => {
    const seen = [...new Set(m.input.extensions.map((e) => (e === "jpeg" ? "jpg" : e === "tif" ? "tiff" : e).toUpperCase()))];
    return seen.length > 8 ? seen.slice(0, 8).join(", ") + "…" : seen.join(", ");
  });
</script>

<div class="tool">
  <div class="head">
    <button class="crumb" onclick={() => go({ name: "home" })} disabled={phase === "running"}>
      <i class="ti ti-arrow-left"></i><span>{t("allTools")}</span>
    </button>
    <span class="muted">/</span>
    <span class="htile" style="--c:{toolColor(m)}"><i class="ti ti-{m.icon ?? 'file'}"></i></span>
    <span class="title">{L(m.name)}</span>
    {#if toolNumber(id)}<span class="tid" title={t("toolIdHelp")}>{t("toolId", { n: toolNumber(id) ?? 0 })}</span>{/if}
    <span class="grow"></span>
    <button class="icon star" class:on={isFav} title={isFav ? t("favoriteRemove") : t("favoriteAdd")} onclick={() => toggleFavorite(id)}>
      <i class="ti {isFav ? 'ti-star-filled' : 'ti-star'}"></i>
    </button>
  </div>

  <div class="panes">
    <!-- left: files -->
    <section class="left scroll">
      {#if items.length || phase !== "edit"}
        <p class="lbl">{t("filesCount", { n: items.length })}</p>
      {/if}
      {#each items as it, idx (it.id)}
        <FileRow
          bind:item={items[idx]}
          manifest={m}
          index={idx}
          count={items.length}
          editable={phase === "edit"}
          onremove={() => remove(it)}
          onmove={(d) => move(idx, d)}
          onunlock={(pw) => unlock(it, pw)}
        />
      {/each}
      {#if phase === "edit"}
        {#if items.length}
          <button class="add" onclick={pick}><i class="ti ti-plus"></i>{t("addFiles")}</button>
        {:else}
          <!-- Nothing yet: the whole pane invites the files. -->
          <button class="dropzone" onclick={pick}>
            <span class="dz-icon" style="--c:{toolColor(m)}"><i class="ti ti-cloud-upload"></i></span>
            <span class="dz-title">{t("dropTitle")}</span>
            <span class="dz-or">{t("dropOr")}</span>
            {#if !m.input.anyFile && m.input.extensions.length}
              <span class="dz-types">{t("accepts", { list: acceptsList })}</span>
            {/if}
          </button>
        {/if}
        <div class="more-src">
          {#if m.input.multiple !== false}
            <FolderAdd extensions={m.input.extensions} anyFile={!!m.input.anyFile} onadd={addFiles} />
          {/if}
          {#if m.input.multiple !== false || !items.length}
            <LinkAdd extensions={m.input.extensions} anyFile={!!m.input.anyFile} multiple={m.input.multiple !== false} onadd={addFiles} />
          {/if}
        </div>
        {#if skippedNotice}
          <div class="note info"><i class="ti ti-info-circle"></i><span>{i18n.lang === "ro" ? `${skippedNotice} fișiere nu se potrivesc cu unealta și nu au fost adăugate.` : `${skippedNotice} files don't fit this tool and were not added.`}</span></div>
        {/if}
      {/if}
      {#if output.originals === "replace"}
        <div class="note warn"><i class="ti ti-alert-triangle"></i><span>{t("originalsReplace")}</span></div>
      {:else}
        <p class="safe"><i class="ti ti-shield-check"></i><span>{t("originalsSafe")}</span></p>
      {/if}
    </section>

    <!-- right: options or result -->
    <section class="right scroll">
      {#if phase === "done"}
        <div class="done">
          <div class="big {summary?.ok || allSkipped ? 'ok' : 'bad'}">
            <i class="ti {summary?.ok || allSkipped ? 'ti-circle-check' : 'ti-alert-triangle'}"></i>
          </div>
          <h2>{doneTitle}</h2>
          {#if summary?.ok}
            <p class="secondary">
              {fileCount(combinedResults.length ? items.filter((i) => i.state === "ok").length : summary.ok)} · {bytes(summary.inputBytes)} → <b>{bytes(summary.outputBytes)}</b>
              {#if id.includes("compress") && summary.inputBytes && summary.outputBytes && summary.outputBytes < summary.inputBytes}
                <span class="save">(−{percentChange(summary.inputBytes, summary.outputBytes)}%)</span>
              {/if}
            </p>
          {/if}
          {#if jobError}
            <div class="note error"><i class="ti ti-alert-circle"></i><span>{errorText(jobError.code, m.errors, jobError.details)}</span></div>
          {/if}
          {#if combinedResults.length}
            {#each combinedResults.filter((r) => !r.skipped) as r}
              <div class="cres">
                <i class="ti ti-file-check" style="color:var(--ok-text)"></i>
                <span class="rname" title={r.path}>{r.name}</span>
                <span class="muted">{bytes(r.bytes)}</span>
              </div>
            {/each}
          {/if}
          <div class="actions">
            {#if allResults().length}
              <button class="primary" onclick={() => api.reveal(allResults()[0].path)}><i class="ti ti-folder-open"></i> {t("openFolder")}</button>
              {#if allResults().length === 1}
                <button onclick={() => api.openPath(allResults()[0].path)}><i class="ti ti-external-link"></i> {t("open")}</button>
              {/if}
            {/if}
            {#if suggestAvailable}
              <button onclick={retrySuggested}><i class="ti ti-refresh"></i> {t("retryWith")}</button>
            {/if}
          </div>
          <div class="actions">
            <button class="ghost" onclick={() => resetRun()}><i class="ti ti-adjustments"></i> {t("again")}</button>
            <button class="ghost" onclick={() => resetRun(true)}><i class="ti ti-files"></i> {t("newFiles")}</button>
          </div>
        </div>
      {:else}
        <fieldset disabled={phase === "running"}>
          {#if engineUnavailable}
            <div class="note error block"><i class="ti ti-alert-circle"></i><span>{t("engineMissing")}</span></div>
          {/if}
          {#each toDownload as n}
            <EngineNotice name={n} size={engineInfo.find((e) => e.name === n)?.downloadBytes ?? 0} ondone={engineReady} />
          {/each}
          {#if wordMissing}
            <div class="note error block"><i class="ti ti-alert-circle"></i><span>{t("noEntry")}</span></div>
          {/if}
          {#if heifMissing}
            <div class="note error block">
              <i class="ti ti-alert-circle"></i>
              <span>{t("heifMissing")} <button class="link" onclick={() => api.openUrl("ms-windows-store://pdp/?productid=9PMMSR1CGPWG")}>{t("openStore")}</button></span>
            </div>
          {/if}

          {#if presets.length}
            <p class="lbl">{t("forWhat")}</p>
            <div class="chips">
              {#each presets as p}
                <button class="chip" class:on={currentPreset === p.id} title={L(p.help)} onclick={() => applyPreset(p.id)}>{L(p.label)}</button>
              {/each}
              {#if !currentPreset}<span class="chip on custom">{t("custom")}</span>{/if}
            </div>
          {/if}

          <div class="fields">
            {#each basic as f (f.key)}
              <div class:wide={f.type === "range" || f.type === "toggle" || f.display === "chips" || f.type === "pages" || f.type === "color" || longOptions(f) || basic.length === 1}>
                <FieldView field={f} bind:value={values[f.key]} {ctx} onchange={() => touch(f.key)} invalid={activeRules.some((r) => r.severity === "error" && r.fields?.includes(f.key))} />
              </div>
            {/each}
          </div>

          {#if advancedGroups.length}
            <button class="adv-btn" class:open={advOpen} onclick={() => (advOpen = !advOpen)}>
              <span><i class="ti ti-adjustments"></i>{t("moreOptions")}</span>
              <i class="ti {advOpen ? 'ti-chevron-up' : 'ti-chevron-down'}"></i>
            </button>
            {#if advOpen}
              <div class="adv">
                {#each advancedGroups as g}
                  {#if g.label}<p class="group">{g.label}</p>{/if}
                  {#each g.fields as f (f.key)}
                    <div class="advf">
                      <FieldView field={f} bind:value={values[f.key]} {ctx} onchange={() => touch(f.key)} />
                    </div>
                  {/each}
                {/each}
                <button class="link small" onclick={resetDefaults}>{t("resetDefaults")}</button>
              </div>
            {/if}
          {/if}

          {#if activeRules.length}
            <div class="rules">
              {#each activeRules as r (r.id)}
                <div class="note {r.severity === 'error' ? 'error' : r.severity === 'info' ? 'info' : 'warn'}">
                  <i class="ti {r.severity === 'info' ? 'ti-info-circle' : 'ti-alert-triangle'}"></i>
                  <span>
                    {L(r.message)}
                    {#if r.severity === "confirm"}
                      <label class="confirm"><input type="checkbox" bind:checked={confirmed[r.id]} />{t("understood")}</label>
                    {/if}
                  </span>
                </div>
              {/each}
            </div>
          {/if}

          {#if m.ui?.preview === "beforeAfter" && phase === "edit"}
            <BeforeAfter tool={id} {items} options={finalOptions()} />
          {/if}

          <OutputPanel bind:output manifest={m} sampleName={items[0]?.info.name ?? ""} disabled={phase === "running"} />
        </fieldset>
      {/if}
    </section>
  </div>

  <footer class="foot">
    {#if phase === "running"}
      <div class="prog">
        <div class="pbar"><div style="width:{Math.round(overall * 100)}%"></div></div>
        <span class="secondary">{cancelling ? t("cancelling") : `${t("working")} ${Math.round(overall * 100)}%`}</span>
      </div>
      <button onclick={cancel} disabled={cancelling}><i class="ti ti-player-stop"></i> {t("cancel")}</button>
    {:else if phase === "edit"}
      <div class="secondary est">
        {#if okItems.length}
          {bytes(totalIn)}
          {#if estimate?.outputBytes != null}
            <i class="ti ti-arrow-right"></i>
            <b class="primary-text">{estimate.confidence === "high" ? "" : t("approx") + " "}{bytes(estimate.outputBytes)}</b>
          {:else}
            {t("total")}
          {/if}
          {#if notOk.length}
            <span class="warn-text"> · {i18n.lang === "ro" ? `${notOk.length} sărite` : `${notOk.length} skipped`}</span>
          {/if}
        {/if}
      </div>
      {#if engineBusy}
        <button class="primary" disabled>
          {toDownload.some((n) => downloads[n]?.total && downloads[n].done < downloads[n].total)
            ? t("engineDownloadingShort", { name: toDownload.map((n) => ENGINE_NAMES[n] ?? n).join(", ") })
            : t("engineVerifying")}
        </button>
      {:else if waitingForEngine}
        <button class="primary" onclick={downloadThenStart}>
          {t("engineDownloadAndStart", { name: toDownload.map((n) => ENGINE_NAMES[n] ?? n).join(", ") })}
        </button>
      {:else}
        <button class="primary" disabled={!canStart} onclick={start}>{startLabel}</button>
      {/if}
    {:else}
      <div></div>
      <button onclick={() => go({ name: "home" })}>{t("allTools")}</button>
    {/if}
  </footer>
</div>

<style>
  .tool {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--layer-stroke);
  }
  .crumb {
    border: 0;
    background: transparent;
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    padding: 4px 6px;
  }
  .crumb .ti {
    font-size: 16px;
  }
  .title {
    font-weight: 600;
    font-size: 15px;
  }
  .htile {
    width: 26px;
    height: 26px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--c) 13%, transparent);
    color: var(--c);
  }
  .htile .ti {
    font-size: 16px;
  }
  .tid {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
    border: 1px solid var(--card-stroke);
    border-radius: 999px;
    padding: 0 7px;
    line-height: 18px;
    cursor: default;
  }
  .dropzone {
    width: 100%;
    min-height: 220px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 24px 16px;
    background: var(--layer-solid);
    border: 1.5px dashed var(--border-dash);
    border-radius: var(--radius-lg);
    color: var(--text-2);
    text-align: center;
  }
  .dropzone:hover:not(:disabled) {
    background: var(--layer-solid);
    border-color: var(--accent-border);
  }
  .dz-icon {
    width: 52px;
    height: 52px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    margin-bottom: 8px;
    background: color-mix(in srgb, var(--c) 12%, transparent);
    color: var(--c);
  }
  .dz-icon .ti {
    font-size: 28px;
  }
  .dz-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .dz-or {
    font-size: 13px;
  }
  .dz-types {
    margin-top: 10px;
    font-size: 12px;
    color: var(--text-3);
  }
  .more-src {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 18px;
    margin-top: 6px;
  }
  .safe {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 14px 2px 0;
    font-size: 12.5px;
    line-height: 17px;
    color: var(--text-3);
  }
  .safe .ti {
    font-size: 15px;
    color: var(--ok-text);
    flex: none;
    margin-top: 1px;
  }
  .grow {
    flex: 1;
  }
  .star {
    font-size: 17px;
  }
  .star.on {
    color: #e0a400;
  }
  .panes {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
  }
  .left {
    border-right: 1px solid var(--layer-stroke);
    padding: 18px;
    background: var(--surface-2);
  }
  .right {
    padding: 18px 22px;
    container-type: inline-size;
  }
  .add {
    width: 100%;
    background: transparent;
    border: 1.5px dashed var(--border-dash);
    color: var(--text-2);
    padding: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin-bottom: 10px;
  }
  .add .ti {
    font-size: 14px;
  }
  .left .note {
    margin-top: 8px;
  }
  fieldset {
    border: 0;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
  .chip.custom {
    border: 1px dashed var(--accent-border);
    cursor: default;
  }
  .fields {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 12px;
    margin-bottom: 12px;
  }
  .fields .wide {
    grid-column: 1 / -1;
  }
  /* A narrow window: one option per row, so no choice is cut off. */
  @container (max-width: 400px) {
    .fields {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .adv-btn {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    border: 1px solid var(--card-stroke);
    background: var(--card);
    padding: 9px 12px;
    border-radius: var(--radius);
  }
  .adv-btn span {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .adv-btn .ti {
    font-size: 16px;
  }
  .adv-btn.open {
    border-radius: var(--radius) var(--radius) 0 0;
  }
  .adv {
    border: 1px solid var(--card-stroke);
    background: var(--card);
    border-top: 0;
    border-radius: 0 0 var(--radius) var(--radius);
    padding: 10px 12px;
  }
  .group {
    margin: 4px 0 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .advf {
    margin-bottom: 10px;
  }
  .small {
    font-size: 12px;
  }
  .rules {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 12px;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    font-weight: 600;
    cursor: pointer;
  }
  .block {
    margin-bottom: 12px;
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-top: 1px solid var(--layer-stroke);
    min-height: 62px;
  }
  .est {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .primary-text {
    color: var(--text);
  }
  .warn-text {
    color: var(--warn-text);
  }
  .prog {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .pbar {
    flex: 1;
    max-width: 360px;
    height: 6px;
    border-radius: 3px;
    background: var(--hover);
    overflow: hidden;
  }
  .pbar div {
    height: 100%;
    background: var(--accent-fill);
    transition: width 0.2s;
  }
  .done {
    text-align: center;
    padding: 24px 8px;
  }
  .big .ti {
    font-size: 48px;
  }
  .big.ok .ti {
    color: var(--ok-text);
  }
  .big.bad .ti {
    color: var(--warn-text);
  }
  .done h2 {
    margin: 6px 0 4px;
    font-size: 20px;
  }
  .save {
    color: var(--ok-text);
    font-weight: 600;
  }
  .done .note {
    text-align: left;
    margin: 10px 0;
  }
  .cres {
    display: flex;
    align-items: center;
    gap: 8px;
    justify-content: center;
    margin-top: 8px;
  }
  .rname {
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: center;
    flex-wrap: wrap;
    margin-top: 16px;
  }
  .actions button {
    display: flex;
    align-items: center;
    gap: 6px;
  }
</style>
