<script lang="ts">
  import { api, type Field, type FieldOption } from "../lib/api";
  import { L, t, i18n } from "../lib/i18n.svelte";
  import { evalCond, type CondCtx } from "../lib/cond";

  let {
    field,
    value = $bindable(),
    ctx,
    invalid = false,
    onchange,
  }: { field: Field; value: any; ctx: CondCtx; invalid?: boolean; onchange?: () => void } = $props();

  const CUSTOM = "__custom__";

  /** A file chosen for an option (a watermark picture, a subtitle…), kept as its path. */
  async function pickFile() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const accept = ((field as any).accept ?? []) as string[];
    const picked = await open({ multiple: false, directory: false, filters: accept.length ? [{ name: L(field.label), extensions: accept }] : undefined });
    if (typeof picked === "string") {
      value = picked;
      changed();
    }
  }
  let showSecret = $state(false);

  function changed() {
    onchange?.();
  }

  const options = $derived.by<FieldOption[]>(() => {
    if (field.optionsFrom?.file) {
      const prop = field.optionsFrom.file;
      const lists = ctx.files.map((f) => f[prop]).filter(Array.isArray) as FieldOption[][];
      if (field.optionsFrom.combine === "single") return ctx.files.length === 1 ? (lists[0] ?? []) : [];
      return lists[0] ?? [];
    }
    if (field.optionsFrom?.env) return (ctx.env[field.optionsFrom.env] as FieldOption[]) ?? [];
    return (field.options ?? []).filter((o) => evalCond(o.showIf, ctx));
  });

  const optLabel = (o: FieldOption) => (o.label ? L(o.label) : String(o.value));

  /** A "multi" field: several options on at once, the value is their list. */
  function toggleMulti(v: any) {
    const list = Array.isArray(value) ? [...value] : [];
    const i = list.indexOf(v);
    if (i >= 0) list.splice(i, 1);
    else list.push(v);
    value = list;
    changed();
  }
  const isChips = $derived(field.display === "chips");

  // select with allowCustom or suggestions: a custom value outside the list
  const list = $derived<FieldOption[]>(
    field.suggestions ? field.suggestions.map((s) => ({ value: s.value, label: s.label })) : options,
  );
  const hasCustom = $derived(!!field.allowCustom || !!field.suggestions);
  let customMode = $state(false);
  $effect(() => {
    if (hasCustom && value !== undefined && !list.some((o) => o.value === value)) customMode = true;
  });
  const selectValue = $derived(customMode ? CUSTOM : String(value));

  function onSelect(e: Event) {
    const v = (e.target as HTMLSelectElement).value;
    if (v === CUSTOM) {
      customMode = true;
      return;
    }
    customMode = false;
    const o = list.find((o) => String(o.value) === v);
    value = o ? o.value : v;
    changed();
  }

  // number with units (bytes shown as MB / KB)
  const units = $derived(field.units?.display ?? []);
  let unitId = $state("");
  $effect(() => {
    if (!unitId && units.length) {
      const def = field.units?.defaultDisplay ?? units[0].id;
      // the largest unit that keeps the value >= 1
      const fit = [...units].sort((a, b) => (b.factor ?? 1) - (a.factor ?? 1)).find((u) => (value ?? 0) / (u.factor ?? 1) >= 1);
      unitId = fit?.id ?? def;
    }
  });
  const factor = $derived(units.find((u) => u.id === unitId)?.factor ?? 1);
  const shown = $derived(value == null ? "" : Math.round(((value as number) / factor) * 100) / 100);
  function onNumber(e: Event) {
    const n = parseFloat((e.target as HTMLInputElement).value.replace(",", "."));
    if (!isNaN(n)) {
      value = Math.round(n * factor);
      changed();
    }
  }

  const unitLabel = (u: string) => (u === "%" ? "%" : ` ${u}`);
  // Names the control for screen readers (the label is a paragraph above it).
  const uid = Math.random().toString(36).slice(2, 8);
  const lid = $derived(`fld-${field.key}-${uid}`);
</script>

<div class="field" class:invalid>
  {#if field.type === "toggle"}
    <label class="toggle">
      <input type="checkbox" bind:checked={value} onchange={changed} />
      <span>{L(field.label)}</span>
    </label>
    {#if field.help}<p class="help">{L(field.help)}</p>{/if}
    {#if field.helpLink}<button type="button" class="help-link" onclick={() => api.openUrl(field.helpLink!.url)}>{L(field.helpLink.label)} ↗</button>{/if}
  {:else}
    {#if field.label}<p class="lbl" id={lid}>{L(field.label)}</p>{/if}

    {#if field.type === "select" && isChips}
      <div class="chips" role="group" aria-labelledby={lid}>
        {#each options as o}
          <button class="chip" aria-pressed={value === o.value} class:on={value === o.value} onclick={() => ((value = o.value), changed())}>{optLabel(o)}</button>
        {/each}
      </div>
    {:else if field.type === "multi"}
      <div class="chips" role="group" aria-labelledby={lid}>
        {#each options as o}
          <button class="chip" aria-pressed={Array.isArray(value) && value.includes(o.value)} class:on={Array.isArray(value) && value.includes(o.value)} onclick={() => toggleMulti(o.value)}>{optLabel(o)}</button>
        {/each}
      </div>
    {:else if field.type === "select" || (field.type === "number" && field.suggestions)}
      <select aria-labelledby={lid} value={selectValue} onchange={onSelect}>
        {#each list as o}
          <option value={String(o.value)}>{optLabel(o)}{o.detail ? ` · ${o.detail}` : ""}</option>
        {/each}
        {#if hasCustom}
          <option value={CUSTOM}>{t("otherValue")}</option>
        {/if}
      </select>
      {#if customMode}
        <div class="custom">
          {#if units.length}
            <input aria-labelledby={lid} type="number" min="0" step="any" value={shown} oninput={onNumber} />
            <select aria-labelledby={lid} bind:value={unitId}>
              {#each units as u}<option value={u.id}>{u.id}</option>{/each}
            </select>
          {:else}
            <input aria-labelledby={lid}
              type="number"
              min={field.allowCustom?.min}
              max={field.allowCustom?.max}
              {value}
              oninput={(e) => {
                const n = parseFloat((e.target as HTMLInputElement).value);
                if (!isNaN(n)) ((value = n), changed());
              }}
            />
            {#if field.allowCustom?.unit}<span class="muted">{field.allowCustom.unit}</span>{/if}
          {/if}
        </div>
      {/if}
    {:else if field.type === "number"}
      <div class="custom">
        {#if units.length}
          <input aria-labelledby={lid} type="number" min="0" step="any" value={shown} oninput={onNumber} />
          <select aria-labelledby={lid} bind:value={unitId}>
            {#each units as u}<option value={u.id}>{u.id}</option>{/each}
          </select>
        {:else}
          <input aria-labelledby={lid} type="number" min={field.min} max={field.max} step={field.step ?? 1} bind:value oninput={changed} />
          {#if field.unit}<span class="muted">{field.unit}</span>{/if}
        {/if}
      </div>
    {:else if field.type === "range"}
      <div class="range">
        <input aria-labelledby={lid} type="range" min={field.min ?? 0} max={field.max ?? 100} step={field.step ?? 1} bind:value oninput={changed} />
        <span class="val">{value}{field.unit ? unitLabel(field.unit) : ""}</span>
      </div>
    {:else if field.type === "color"}
      <div class="custom">
        <input aria-labelledby={lid} type="color" bind:value oninput={changed} class="color" />
        <input aria-labelledby={lid} type="text" bind:value oninput={changed} class="hex" maxlength="9" />
      </div>
    {:else if field.type === "pages"}
      <!-- "all" is the default: shown as an empty box with "all" as the hint -->
      <input aria-labelledby={lid}
        type="text"
        class="text"
        value={value === "all" ? "" : value}
        oninput={(e) => {
          const v = (e.target as HTMLInputElement).value.trim();
          value = v === "" ? "all" : v;
          changed();
        }}
        placeholder={L(field.placeholder)}
        spellcheck="false"
      />
    {:else if field.type === "file"}
      <div class="custom secret">
        <input aria-labelledby={lid} type="text" class="text" readonly value={value ? String(value).split(/[\/]/).pop() : ""} placeholder={L({ ro: "Niciun fișier ales", en: "No file chosen" })} title={value ?? ""} />
        <button type="button" onclick={pickFile}>{L({ ro: "Alege…", en: "Choose…" })}</button>
      </div>
    {:else if field.type === "password"}
      <!-- Never remembered: ToolView leaves password fields out of the saved values. -->
      <div class="custom secret">
        <input aria-labelledby={lid} type={showSecret ? "text" : "password"} class="text" bind:value oninput={changed} autocomplete="new-password" spellcheck="false" />
        <button type="button" class="icon" title={L({ ro: "Arată parola", en: "Show password" })} onclick={() => (showSecret = !showSecret)}>
          <i class="ti {showSecret ? 'ti-eye-off' : 'ti-eye'}"></i>
        </button>
      </div>
    {:else}
      <input aria-labelledby={lid}
        type="text"
        class="text"
        bind:value
        oninput={changed}
        placeholder={L(field.placeholder)}
        spellcheck="false"
      />
    {/if}
    {#if field.help}<p class="help">{L(field.help)}</p>{/if}
    {#if field.helpLink}<button type="button" class="help-link" onclick={() => api.openUrl(field.helpLink!.url)}>{L(field.helpLink.label)} ↗</button>{/if}
  {/if}
</div>

<style>
  .field {
    min-width: 0;
  }
  .field select,
  .field .text {
    width: 100%;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    padding: 2px 0;
  }
  .range {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .range input {
    flex: 1;
  }
  .val {
    min-width: 52px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .custom {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 6px;
  }
  .secret {
    margin-top: 0;
  }
  .secret input {
    flex: 1;
    min-width: 0;
  }
  .custom input[type="number"] {
    width: 110px;
  }
  .custom select {
    width: 80px;
  }
  .color {
    width: 40px;
    height: 34px;
    padding: 2px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--surface-1);
  }
  .hex {
    width: 100px;
  }
  .help {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--text-3);
  }
  .help-link {
    margin: 4px 0 0;
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    font-size: 12px;
    color: var(--accent);
    cursor: pointer;
  }
  .help-link:hover {
    text-decoration: underline;
  }
  .invalid select,
  .invalid input {
    border-color: var(--error-text);
  }
</style>
