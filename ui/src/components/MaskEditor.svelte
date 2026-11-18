<script lang="ts">
  // Paint over what should disappear from the photo. The painted layer is kept on the
  // file (item.item.mask, a PNG) at up to 1600 px; the worker fills it in.
  import { api, fileSrc } from "../lib/api";
  import { L } from "../lib/i18n.svelte";
  import type { Item } from "../lib/item";

  let { tool, item = $bindable() }: { tool: string; item: Item } = $props();

  const natW = $derived<number>(item.props.width ?? 1000);
  const natH = $derived<number>(item.props.height ?? 750);
  // The painted layer's size: the photo's, at most 1600 px on the long side.
  const k = $derived(Math.min(1, 1600 / Math.max(natW, natH)));
  const cw = $derived(Math.max(1, Math.round(natW * k)));
  const ch = $derived(Math.max(1, Math.round(natH * k)));

  let canvas = $state<HTMLCanvasElement>();
  let boxW = $state(0);
  let brush = $state(28);
  let erasing = $state(false);
  let painted = $state(!!item.item.mask);
  let result = $state<{ path: string; seq: number } | null>(null);
  let showResult = $state(false);
  let previewing = $state(false);
  let previewError = $state("");
  let undo: ImageData[] = [];
  let last: { x: number; y: number } | null = null;
  let seq = 0;

  // A mask kept from before (the tool reopened with the same file) comes back.
  $effect(() => {
    const c = canvas;
    if (!c || !item.item.mask) return;
    const im = new Image();
    im.onload = () => c.getContext("2d")!.drawImage(im, 0, 0, c.width, c.height);
    im.src = item.item.mask;
  });

  function at(e: PointerEvent) {
    const r = canvas!.getBoundingClientRect();
    return { x: ((e.clientX - r.left) / r.width) * cw, y: ((e.clientY - r.top) / r.height) * ch };
  }
  function stroke(a: { x: number; y: number }, b: { x: number; y: number }) {
    const c = canvas!.getContext("2d")!;
    c.globalCompositeOperation = erasing ? "destination-out" : "source-over";
    c.strokeStyle = "#ff3b30";
    c.fillStyle = "#ff3b30";
    c.lineCap = "round";
    c.lineJoin = "round";
    // The brush is set in screen pixels; the layer may be larger or smaller.
    c.lineWidth = brush * (cw / Math.max(1, boxW));
    c.beginPath();
    c.moveTo(a.x, a.y);
    c.lineTo(b.x, b.y);
    c.stroke();
  }
  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    canvas!.setPointerCapture(e.pointerId);
    const c = canvas!.getContext("2d")!;
    undo.push(c.getImageData(0, 0, cw, ch));
    if (undo.length > 20) undo.shift();
    last = at(e);
    stroke(last, last);
    showResult = false;
  }
  function move(e: PointerEvent) {
    if (!last) return;
    const p = at(e);
    stroke(last, p);
    last = p;
  }
  function up() {
    if (!last) return;
    last = null;
    save();
  }
  function save() {
    const c = canvas!.getContext("2d")!;
    const d = c.getImageData(0, 0, cw, ch).data;
    let any = false;
    for (let i = 3; i < d.length; i += 4 * 7) if (d[i] > 40) { any = true; break; }
    painted = any;
    item.item.mask = any ? canvas!.toDataURL("image/png") : "";
    result = null;
  }
  function stepBack() {
    const prev = undo.pop();
    if (!prev) return;
    canvas!.getContext("2d")!.putImageData(prev, 0, 0);
    save();
  }
  function clearAll() {
    const c = canvas!.getContext("2d")!;
    undo.push(c.getImageData(0, 0, cw, ch));
    c.clearRect(0, 0, cw, ch);
    save();
  }

  async function preview() {
    if (!item.item.mask) return;
    previewing = true;
    previewError = "";
    const my = ++seq;
    try {
      const px = Math.min(1600, Math.round(Math.max(boxW, boxW * (natH / natW)) * (window.devicePixelRatio || 1)));
      const r = await api.preview(tool, { id: item.id, path: item.info.path, analysis: $state.snapshot(item.props) }, { mask: item.item.mask }, my, px);
      if (my === seq && r?.type === "preview") {
        result = { path: r.path, seq: my };
        showResult = true;
      } else if (my === seq) {
        previewError = L({ ro: "Previzualizarea nu a reușit.", en: "The preview didn't work." });
      }
    } catch {
      if (my === seq) previewError = L({ ro: "Previzualizarea nu a reușit.", en: "The preview didn't work." });
    } finally {
      if (my === seq) previewing = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => (e.ctrlKey && e.key === "z" ? (e.preventDefault(), stepBack()) : null)} />

<div class="ed">
  <div class="bar">
    <button class:on={!erasing} onclick={() => (erasing = false)}><i class="ti ti-brush"></i>{L({ ro: "Pictează", en: "Paint" })}</button>
    <button class:on={erasing} onclick={() => (erasing = true)}><i class="ti ti-eraser"></i>{L({ ro: "Șterge pictura", en: "Unpaint" })}</button>
    <label class="size" title={L({ ro: "Mărimea pensulei", en: "Brush size" })}>
      <i class="ti ti-circle-dot"></i><input type="range" min="6" max="120" bind:value={brush} />
    </label>
    <button onclick={stepBack} title={L({ ro: "Înapoi (Ctrl+Z)", en: "Undo (Ctrl+Z)" })}><i class="ti ti-arrow-back-up"></i></button>
    <button onclick={clearAll} disabled={!painted} title={L({ ro: "Șterge tot", en: "Clear all" })}><i class="ti ti-trash"></i></button>
    <span class="grow"></span>
    <button class="primary" onclick={preview} disabled={!painted || previewing}>
      <i class="ti ti-eye"></i>{previewing ? L({ ro: "Se lucrează…", en: "Working…" }) : L({ ro: "Vezi rezultatul", en: "See the result" })}
    </button>
  </div>

  <div class="photo" bind:clientWidth={boxW} style="aspect-ratio:{natW} / {natH}">
    <img src={fileSrc(item.info.path)} alt="" draggable="false" />
    <canvas bind:this={canvas} width={cw} height={ch} class:hidden={showResult}
      style="cursor:{erasing ? 'cell' : 'crosshair'}" onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up}></canvas>
    {#if result && showResult}<img class="res" src={fileSrc(result.path) + `?v=${result.seq}`} alt="" draggable="false" />{/if}
  </div>

  <div class="foot">
    {#if result}
      <button class="link" onclick={() => (showResult = !showResult)}>
        <i class="ti ti-arrows-exchange"></i>{showResult ? L({ ro: "Arată originalul", en: "Show the original" }) : L({ ro: "Arată rezultatul", en: "Show the result" })}
      </button>
    {:else}
      <span class="muted small">{L({ ro: "Pictează peste ce vrei să dispară, cu puțin în jur.", en: "Paint over what should disappear, with a little around it." })}</span>
    {/if}
    {#if previewError}<span class="err small">{previewError}</span>{/if}
  </div>
</div>

<style>
  .ed { display: flex; flex-direction: column; gap: 8px; margin-top: 10px; }
  .bar, .foot { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .bar button { display: flex; align-items: center; gap: 6px; padding: 5px 10px; }
  .bar button.on { background: var(--accent-bg); border-color: var(--accent-border); color: var(--accent); }
  .size { display: flex; align-items: center; gap: 6px; color: var(--text-2); }
  .size input { width: 110px; }
  .grow { flex: 1; }
  .photo { position: relative; width: 100%; background: repeating-conic-gradient(var(--hover) 0 25%, transparent 0 50%) 0 0 / 16px 16px; border-radius: var(--radius); overflow: hidden; user-select: none; }
  .photo img, .photo canvas { position: absolute; inset: 0; width: 100%; height: 100%; }
  .photo img { pointer-events: none; object-fit: contain; }
  .photo canvas { opacity: 0.55; touch-action: none; }
  .photo canvas.hidden { opacity: 0; pointer-events: none; }
  .link { display: flex; align-items: center; gap: 6px; background: none; border: none; color: var(--accent); padding: 4px 2px; cursor: pointer; font-size: 13px; }
  .small { font-size: 12px; }
  .err { color: var(--error-text); }
</style>
