<script lang="ts">
  // The PDF as pages to write on: its form fields as boxes to fill, and text, a
  // signature, a picture, ticks, crosses, white-out and highlights placed by hand.
  // Everything is kept on the file (item.item.form / item.item.edits) as fractions of
  // the page as shown, from its top-left corner; the worker draws it into the PDF.
  import { api, fileSrc } from "../lib/api";
  import { L, i18n, localeOf } from "../lib/i18n.svelte";
  import type { Item } from "../lib/item";
  import SignaturePad from "./SignaturePad.svelte";

  type Edit = { id: number; type: string; page: number; x: number; y: number; w: number; h: number; text?: string; size?: number; color?: string; src?: string; ratio?: number };

  // "redact": only black boxes to draw (the redaction tool), no form fields.
  let { tool, item = $bindable(), mode = "fill" }: { tool: string; item: Item; mode?: string } = $props();

  const pageCount = $derived<number>(item.props.pageCount ?? 1);
  const sizes = $derived<[number, number][]>(item.props.pages ?? []);
  const fields = $derived<any[]>(item.props.fields ?? []);
  let page = $state(1);
  let img = $state<{ path: string; seq: number } | null>(null);
  let loading = $state(false);
  let boxW = $state(0);
  let selected = $state<number | null>(null);
  let signFor = $state<{ x: number; y: number; w: number; h: number } | null | undefined>(undefined);
  let nextId = 1;
  let seq = 0;

  if (!Array.isArray(item.item.edits)) item.item.edits = [];
  if (!item.item.form || typeof item.item.form !== "object") item.item.form = {};
  const edits = $derived<Edit[]>(item.item.edits);
  for (const e of item.item.edits) nextId = Math.max(nextId, (e.id ?? 0) + 1);

  // The page as shown: its proportions, and its size on screen.
  const ratio = $derived(sizes[page - 1] ? sizes[page - 1][1] / sizes[page - 1][0] : 1.414);
  const pageH = $derived(boxW * ratio);

  $effect(() => {
    const p = page;
    const w = boxW;
    if (!w) return;
    const my = ++seq;
    loading = true;
    const t = setTimeout(async () => {
      try {
        const px = Math.min(1600, Math.round(Math.max(w, w * ratio) * (window.devicePixelRatio || 1)));
        const r = await api.preview(tool, { id: item.id, path: item.info.path, analysis: $state.snapshot(item.props) }, { page: p }, my, px);
        if (my === seq && r?.type === "preview") img = { path: r.path, seq: my };
      } catch {
        if (my === seq) img = null;
      } finally {
        if (my === seq) loading = false;
      }
    }, 120);
    return () => clearTimeout(t);
  });

  // ---------- adding ----------
  function add(e: Omit<Edit, "id" | "page">) {
    const n = { id: nextId++, page, ...e };
    item.item.edits = [...item.item.edits, n];
    selected = n.id;
  }
  const at = (w: number, h: number) => ({ x: 0.5 - w / 2, y: 0.4 - h / 2, w, h });
  const square = (w: number) => w / ratio;
  function addText(text = "") {
    add({ type: "text", ...at(0.3, 0.025), text, size: 0.016, color: "#111827" });
  }
  function addDate() {
    addText(new Intl.DateTimeFormat(localeOf(i18n.lang), { dateStyle: "medium" }).format(new Date()));
  }
  function addMark(type: "check" | "cross") {
    add({ type, ...at(0.035, square(0.035) ), color: type === "check" ? "#111827" : "#111827" });
  }
  function addBox(type: "whiteout" | "highlight" | "redact") {
    add({ type, ...at(0.3, 0.025), color: type === "whiteout" ? "#ffffff" : type === "redact" ? "#000000" : "#ffe14d" });
  }
  function placePicture(type: "signature" | "image", src: string, r: number, into?: { x: number; y: number; w: number; h: number } | null) {
    if (into) {
      // Fitted into a signature field, centered.
      let w = into.w;
      let h = (w / r) / ratio;
      if (h > into.h) {
        h = into.h;
        w = h * ratio * r;
      }
      add({ type, x: into.x + (into.w - w) / 2, y: into.y + (into.h - h) / 2, w, h, src, ratio: r });
    } else {
      const w = type === "signature" ? 0.26 : 0.35;
      add({ type, ...at(w, (w / r) / ratio), src, ratio: r });
    }
  }
  function pickImage(e: Event) {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    const url = URL.createObjectURL(file);
    const im = new Image();
    im.onload = () => {
      // Kept at most 2000 px: enough for print at the sizes placed on a page.
      const s = Math.min(1, 2000 / Math.max(im.width, im.height));
      const c = document.createElement("canvas");
      c.width = Math.round(im.width * s);
      c.height = Math.round(im.height * s);
      c.getContext("2d")!.drawImage(im, 0, 0, c.width, c.height);
      const png = file.type === "image/png" || file.type === "image/webp";
      placePicture("image", c.toDataURL(png ? "image/png" : "image/jpeg", 0.92), im.width / im.height);
      URL.revokeObjectURL(url);
    };
    im.src = url;
  }

  function update(id: number, patch: Partial<Edit>) {
    item.item.edits = item.item.edits.map((e: Edit) => (e.id === id ? { ...e, ...patch } : e));
  }
  function remove(id: number) {
    item.item.edits = item.item.edits.filter((e: Edit) => e.id !== id);
    if (selected === id) selected = null;
  }
  const sel = $derived(edits.find((e) => e.id === selected) ?? null);

  // ---------- moving and resizing ----------
  let drag: { id: number; mode: "move" | "size"; sx: number; sy: number; o: Edit } | null = null;
  function startDrag(ev: PointerEvent, e: Edit, mode: "move" | "size") {
    if (ev.button !== 0) return;
    ev.stopPropagation();
    selected = e.id;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    drag = { id: e.id, mode, sx: ev.clientX, sy: ev.clientY, o: { ...e } };
  }
  function onDrag(ev: PointerEvent) {
    if (!drag || !boxW) return;
    const dx = (ev.clientX - drag.sx) / boxW;
    const dy = (ev.clientY - drag.sy) / pageH;
    const o = drag.o;
    if (drag.mode === "move") {
      update(drag.id, { x: clamp(o.x + dx, -o.w * 0.5, 1 - o.w * 0.5), y: clamp(o.y + dy, -o.h * 0.5, 1 - o.h * 0.5) });
    } else {
      let w = Math.max(0.01, o.w + dx);
      let h = Math.max(0.006, o.h + dy);
      // Pictures and marks keep their shape.
      if (o.ratio) h = (w / o.ratio) / ratio;
      else if (o.type === "check" || o.type === "cross") h = w / ratio;
      // Text grows with its box when made taller.
      const size = o.type === "text" && o.size ? Math.max(0.006, (o.size * h) / o.h) : o.size;
      update(drag.id, { w, h, size });
    }
  }
  function endDrag() {
    drag = null;
  }
  const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));

  /** Text boxes grow to their lines as they are typed. */
  function typed(e: Edit, text: string) {
    const lines = Math.max(1, text.split("\n").length);
    const h = Math.max(e.h, ((e.size ?? 0.016) * 1.25 * lines) + 0.006);
    update(e.id, { text, h });
  }

  function keydown(ev: KeyboardEvent) {
    const tag = (ev.target as HTMLElement)?.tagName;
    if (selected != null && (ev.key === "Delete" || ev.key === "Backspace") && tag !== "TEXTAREA" && tag !== "INPUT") {
      ev.preventDefault();
      remove(selected);
    }
  }

  // ---------- form fields ----------
  const pageFields = $derived(mode === "redact" ? [] : fields.filter((f) => f.page === page));
  const form = $derived<Record<string, any>>(item.item.form);
  function fieldValue(f: any) {
    return f.name in form ? form[f.name] : f.value;
  }
  function setField(f: any, v: any) {
    item.item.form = { ...item.item.form, [f.name]: v };
  }
  const filledCount = $derived(Object.keys(form).length);
  const fieldCount = $derived(new Set(fields.filter((f) => f.type !== "signature").map((f) => f.name)).size);
</script>

<svelte:window onkeydown={keydown} />

<div class="editor">
  <div class="bar" role="toolbar" aria-label={L({ ro: "Adaugă pe pagină", en: "Add to the page" })}>
    {#if mode === "redact"}
    <button onclick={() => addBox("redact")}><i class="ti ti-square-filled"></i><span>{L({ ro: "Casetă neagră", en: "Black box" })}</span></button>
    <span class="muted small">{L({ ro: "Trage casete peste ce trebuie ascuns; cuvintele și numerele alese în dreapta se găsesc singure.", en: "Draw boxes over what must be hidden; the words and numbers chosen on the right are found by themselves." })}</span>
    {:else}
    <button onclick={() => addText()} title={L({ ro: "Text", en: "Text" })}><i class="ti ti-typography"></i><span>{L({ ro: "Text", en: "Text" })}</span></button>
    <button onclick={() => (signFor = null)} title={L({ ro: "Semnătură", en: "Signature" })}><i class="ti ti-writing-sign"></i><span>{L({ ro: "Semnătură", en: "Signature" })}</span></button>
    <label class="btn" title={L({ ro: "Imagine", en: "Picture" })}>
      <input type="file" accept="image/png,image/jpeg,image/webp" onchange={pickImage} />
      <i class="ti ti-photo"></i><span>{L({ ro: "Imagine", en: "Picture" })}</span>
    </label>
    <button onclick={addDate} title={L({ ro: "Data de azi", en: "Today's date" })}><i class="ti ti-calendar-event"></i></button>
    <button onclick={() => addMark("check")} title={L({ ro: "Bifă", en: "Tick" })}><i class="ti ti-check"></i></button>
    <button onclick={() => addMark("cross")} title={L({ ro: "X", en: "Cross" })}><i class="ti ti-x"></i></button>
    <button onclick={() => addBox("whiteout")} title={L({ ro: "Acoperă cu alb", en: "White-out" })}><i class="ti ti-eraser"></i></button>
    <button onclick={() => addBox("highlight")} title={L({ ro: "Evidențiază", en: "Highlight" })}><i class="ti ti-highlight"></i></button>
    {/if}
  </div>

  {#if sel}
    <div class="selbar">
      {#if sel.type !== "signature" && sel.type !== "image" && sel.type !== "redact"}
        <input type="color" value={sel.color ?? "#111827"} oninput={(e) => update(sel.id, { color: (e.target as HTMLInputElement).value })} aria-label={L({ ro: "Culoare", en: "Color" })} />
      {/if}
      {#if sel.type === "text"}
        <button onclick={() => update(sel.id, { size: Math.max(0.006, (sel.size ?? 0.016) / 1.15), h: Math.max(0.01, sel.h / 1.15) })} title={L({ ro: "Mai mic", en: "Smaller" })}>A−</button>
        <button onclick={() => update(sel.id, { size: (sel.size ?? 0.016) * 1.15, h: sel.h * 1.15 })} title={L({ ro: "Mai mare", en: "Larger" })}>A+</button>
      {/if}
      <span class="grow"></span>
      <button onclick={() => remove(sel.id)}><i class="ti ti-trash"></i>{L({ ro: "Șterge", en: "Delete" })}</button>
    </div>
  {/if}

  <div class="page-wrap" bind:clientWidth={boxW}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="page" style="height:{pageH}px" onpointerdown={() => (selected = null)} onpointermove={onDrag} onpointerup={endDrag}>
      {#if img}<img src={fileSrc(img.path) + `?v=${img.seq}`} alt="" draggable="false" />{/if}
      {#if loading && !img}<div class="loading">{L({ ro: "Se încarcă pagina…", en: "Loading the page…" })}</div>{/if}

      {#each pageFields as f}
        {@const st = `left:${f.rect[0] * 100}%;top:${f.rect[1] * 100}%;width:${f.rect[2] * 100}%;height:${f.rect[3] * 100}%`}
        {@const fs = Math.max(8, Math.min(f.rect[3] * pageH * 0.62, 15))}
        {#if f.type === "text"}
          {#if f.multiline}
            <textarea class="fld" style="{st};font-size:{fs}px" value={fieldValue(f) ?? ""} disabled={f.readOnly} maxlength={f.maxLen}
              oninput={(e) => setField(f, (e.target as HTMLTextAreaElement).value)} onpointerdown={(e) => e.stopPropagation()} title={f.name}></textarea>
          {:else}
            <input class="fld" style="{st};font-size:{fs}px" type={f.password ? "password" : "text"} value={fieldValue(f) ?? ""} disabled={f.readOnly} maxlength={f.maxLen}
              oninput={(e) => setField(f, (e.target as HTMLInputElement).value)} onpointerdown={(e) => e.stopPropagation()} title={f.name} />
          {/if}
        {:else if f.type === "check"}
          {@const on = fieldValue(f) === true || (typeof fieldValue(f) === "string" && fieldValue(f) !== "Off")}
          <button class="fld box" class:on style={st} disabled={f.readOnly} aria-pressed={on} title={f.name}
            onpointerdown={(e) => e.stopPropagation()} onclick={() => setField(f, !on)}>{#if on}<i class="ti ti-check"></i>{/if}</button>
        {:else if f.type === "radio"}
          {@const on = fieldValue(f) === f.on}
          <button class="fld box round" class:on style={st} disabled={f.readOnly} aria-pressed={on} title={f.name}
            onpointerdown={(e) => e.stopPropagation()} onclick={() => setField(f, f.on)}>{#if on}<span class="dot"></span>{/if}</button>
        {:else if f.type === "choice"}
          <select class="fld" style="{st};font-size:{fs}px" value={fieldValue(f) ?? ""} disabled={f.readOnly} title={f.name}
            onpointerdown={(e) => e.stopPropagation()} onchange={(e) => setField(f, (e.target as HTMLSelectElement).value)}>
            <option value=""></option>
            {#each f.options ?? [] as o}<option value={o[0]}>{o[1]}</option>{/each}
          </select>
        {:else if f.type === "signature"}
          <button class="fld sign" style={st} onpointerdown={(e) => e.stopPropagation()}
            onclick={() => (signFor = { x: f.rect[0], y: f.rect[1], w: f.rect[2], h: f.rect[3] })}><i class="ti ti-writing-sign"></i>{L({ ro: "Semnează aici", en: "Sign here" })}</button>
        {/if}
      {/each}

      {#each edits.filter((e) => e.page === page) as e (e.id)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="ed {e.type}" class:sel={selected === e.id}
          style="left:{e.x * 100}%;top:{e.y * 100}%;width:{e.w * 100}%;height:{e.h * 100}%;--c:{e.color ?? '#111827'}"
          onpointerdown={(ev) => startDrag(ev, e, "move")}>
          {#if e.type === "text"}
            <!-- svelte-ignore a11y_autofocus -->
            <textarea value={e.text} autofocus={!e.text} spellcheck="true" style="font-size:{(e.size ?? 0.016) * pageH}px;color:{e.color}"
              oninput={(ev) => typed(e, (ev.target as HTMLTextAreaElement).value)}
              onpointerdown={(ev) => { ev.stopPropagation(); selected = e.id; }}></textarea>
            <span class="grip" onpointerdown={(ev) => startDrag(ev, e, "move")} title={L({ ro: "Mută", en: "Move" })}><i class="ti ti-arrows-move"></i></span>
          {:else if e.type === "signature" || e.type === "image"}
            <img src={e.src} alt="" draggable="false" />
          {:else if e.type === "check"}
            <svg viewBox="0 0 100 100" preserveAspectRatio="none"><polyline points="15,50 40,80 85,18" /></svg>
          {:else if e.type === "cross"}
            <svg viewBox="0 0 100 100" preserveAspectRatio="none"><path d="M18 18 L82 82 M18 82 L82 18" /></svg>
          {/if}
          {#if selected === e.id}<span class="handle" onpointerdown={(ev) => startDrag(ev, e, "size")}></span>{/if}
        </div>
      {/each}
    </div>
  </div>

  <div class="nav">
    <button class="icon" disabled={page <= 1} onclick={() => ((page -= 1), (selected = null))} aria-label={L({ ro: "Pagina anterioară", en: "Previous page" })}><i class="ti ti-chevron-left"></i></button>
    <span>{L({ ro: "Pagina", en: "Page" })} {page} / {pageCount}</span>
    <button class="icon" disabled={page >= pageCount} onclick={() => ((page += 1), (selected = null))} aria-label={L({ ro: "Pagina următoare", en: "Next page" })}><i class="ti ti-chevron-right"></i></button>
    <span class="grow"></span>
    {#if fieldCount}<span class="muted small">{L({ ro: "Câmpuri completate", en: "Fields filled" })}: {filledCount} / {fieldCount}</span>{/if}
  </div>
</div>

{#if signFor !== undefined}
  <SignaturePad onclose={() => (signFor = undefined)} onpick={(src, r) => { placePicture("signature", src, r, signFor); signFor = undefined; }} />
{/if}

<style>
  .editor { display: flex; flex-direction: column; gap: 8px; margin-top: 10px; }
  .bar, .selbar, .nav { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .bar { gap: 4px; }
  .bar button, .bar .btn { display: flex; align-items: center; gap: 6px; padding: 5px 9px; min-width: 0; }
  .bar .btn { border: 1px solid var(--control-stroke); border-radius: var(--radius); background: var(--control); cursor: pointer; font-size: 13px; }
  .bar .btn input { display: none; }
  .selbar { padding: 6px 8px; border-radius: var(--radius); background: var(--accent-bg); }
  .selbar input[type="color"] { width: 32px; height: 28px; padding: 1px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); }
  .selbar button { display: flex; gap: 6px; align-items: center; padding: 3px 10px; }
  .grow { flex: 1; }
  .small { font-size: 12px; }
  .page-wrap { width: 100%; }
  .page { position: relative; width: 100%; background: #fff; box-shadow: 0 1px 4px rgba(0, 0, 0, 0.18); user-select: none; overflow: hidden; }
  .page > img { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; }
  .loading { position: absolute; inset: 0; display: grid; place-items: center; color: #8b93a1; font-size: 13px; }

  .fld { position: absolute; box-sizing: border-box; margin: 0; padding: 0 3px; border: 1px solid rgba(31, 102, 217, 0.45); border-radius: 2px;
         background: rgba(31, 102, 217, 0.09); color: #111; font-family: Arial, sans-serif; min-height: 0; height: auto; outline: none; }
  .fld:focus { background: rgba(255, 255, 255, 0.9); border-color: var(--accent-fill); }
  .fld:disabled { background: rgba(0, 0, 0, 0.04); border-color: rgba(0, 0, 0, 0.15); }
  textarea.fld { resize: none; padding: 2px 3px; line-height: 1.15; }
  .fld.box { display: grid; place-items: center; padding: 0; font-size: 12px; color: #111; }
  .fld.box.round { border-radius: 50%; }
  .fld.box .dot { width: 50%; height: 50%; border-radius: 50%; background: #111; }
  .fld.sign { display: flex; align-items: center; justify-content: center; gap: 6px; border-style: dashed; color: var(--accent); font-size: 12px; overflow: hidden; }

  .ed { position: absolute; box-sizing: border-box; cursor: move; touch-action: none; }
  .ed.sel { outline: 1.5px solid var(--accent-fill); outline-offset: 1px; }
  .ed:not(.sel):hover { outline: 1px dashed rgba(31, 102, 217, 0.6); }
  .ed textarea { width: 100%; height: 100%; border: 0; padding: 0; margin: 0; background: transparent; resize: none; overflow: hidden;
                 line-height: 1.25; font-family: "Segoe UI", Arial, sans-serif; outline: none; cursor: text; white-space: pre; }
  .ed img { width: 100%; height: 100%; display: block; pointer-events: none; }
  .ed svg { width: 100%; height: 100%; display: block; overflow: visible; }
  .ed svg polyline, .ed svg path { fill: none; stroke: var(--c); stroke-width: 12; stroke-linecap: round; stroke-linejoin: round; vector-effect: none; }
  .ed.whiteout { background: var(--c); box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08); }
  .ed.redact { background: #000; opacity: 0.85; }
  .ed.highlight { background: var(--c); opacity: 0.4; mix-blend-mode: multiply; }
  .grip { position: absolute; top: -20px; left: -2px; font-size: 13px; padding: 1px 3px; border-radius: 4px; background: var(--accent-fill); color: #fff; cursor: move; display: none; }
  .ed.sel .grip { display: block; }
  .handle { position: absolute; right: -6px; bottom: -6px; width: 11px; height: 11px; border-radius: 3px; background: var(--accent-fill); border: 2px solid #fff; cursor: nwse-resize; }
  .nav span { font-size: 13px; }
</style>
