<script lang="ts">
  // A signature: drawn with the mouse or a pen, typed in a handwriting font, or a picture
  // of one on paper (its white background made see-through). Returns a PNG data URL,
  // trimmed to the ink. Signatures can be kept on this PC (localStorage) to reuse.
  import { L } from "../lib/i18n.svelte";

  let { onpick, onclose }: { onpick: (src: string, ratio: number) => void; onclose: () => void } = $props();

  const KEY = "mqkit.signatures";
  const FONTS = ["Segoe Script", "Ink Free", "Lucida Handwriting", "Gabriola", "Brush Script MT"];
  const INKS = ["#111827", "#1d3fa8"];

  let tab = $state<"draw" | "type" | "image">("draw");
  let ink = $state(INKS[0]);
  let typed = $state("");
  let font = $state(FONTS[0]);
  let remember = $state(true);
  let saved = $state<string[]>(load());
  let imageSrc = $state("");
  let canvas = $state<HTMLCanvasElement>();
  let drawn = $state(false);
  let drawing = false;
  let last: { x: number; y: number } | null = null;

  function load(): string[] {
    try {
      return JSON.parse(localStorage.getItem(KEY) ?? "[]").filter((s: unknown) => typeof s === "string");
    } catch {
      return [];
    }
  }
  function keep(src: string) {
    const list = [src, ...saved.filter((s) => s !== src)].slice(0, 4);
    localStorage.setItem(KEY, JSON.stringify(list));
  }
  function forget(src: string) {
    saved = saved.filter((s) => s !== src);
    localStorage.setItem(KEY, JSON.stringify(saved));
  }

  // ---------- drawing ----------
  function pos(e: PointerEvent) {
    const r = canvas!.getBoundingClientRect();
    return { x: ((e.clientX - r.left) / r.width) * canvas!.width, y: ((e.clientY - r.top) / r.height) * canvas!.height };
  }
  function down(e: PointerEvent) {
    canvas!.setPointerCapture(e.pointerId);
    drawing = true;
    last = pos(e);
    const c = canvas!.getContext("2d")!;
    c.fillStyle = ink;
    c.beginPath();
    c.arc(last.x, last.y, 2.2, 0, Math.PI * 2);
    c.fill();
    drawn = true;
  }
  function move(e: PointerEvent) {
    if (!drawing || !last) return;
    const p = pos(e);
    const c = canvas!.getContext("2d")!;
    // Pens give pressure; a mouse reports 0.5.
    c.lineWidth = 2.5 + 3 * (e.pressure || 0.5);
    c.lineCap = "round";
    c.lineJoin = "round";
    c.strokeStyle = ink;
    c.beginPath();
    c.moveTo(last.x, last.y);
    c.quadraticCurveTo(last.x, last.y, (last.x + p.x) / 2, (last.y + p.y) / 2);
    c.lineTo(p.x, p.y);
    c.stroke();
    last = p;
  }
  function up() {
    drawing = false;
    last = null;
  }
  function clear() {
    canvas?.getContext("2d")!.clearRect(0, 0, canvas.width, canvas.height);
    drawn = false;
  }

  /** The canvas cut to its ink, with a small margin. */
  function trimmed(src: HTMLCanvasElement): HTMLCanvasElement {
    const c = src.getContext("2d")!;
    const { width: w, height: h } = src;
    const d = c.getImageData(0, 0, w, h).data;
    let [x0, y0, x1, y1] = [w, h, -1, -1];
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++)
        if (d[(y * w + x) * 4 + 3] > 8) {
          if (x < x0) x0 = x;
          if (x > x1) x1 = x;
          if (y < y0) y0 = y;
          if (y > y1) y1 = y;
        }
    if (x1 < 0) return src;
    const m = 6;
    x0 = Math.max(0, x0 - m);
    y0 = Math.max(0, y0 - m);
    x1 = Math.min(w - 1, x1 + m);
    y1 = Math.min(h - 1, y1 + m);
    const out = document.createElement("canvas");
    out.width = x1 - x0 + 1;
    out.height = y1 - y0 + 1;
    out.getContext("2d")!.drawImage(src, x0, y0, out.width, out.height, 0, 0, out.width, out.height);
    return out;
  }

  function typedCanvas(): HTMLCanvasElement {
    const c = document.createElement("canvas");
    c.width = 1400;
    c.height = 360;
    const x = c.getContext("2d")!;
    x.fillStyle = ink;
    x.textBaseline = "middle";
    let size = 160;
    x.font = `${size}px "${font}"`;
    while (x.measureText(typed).width > c.width - 60 && size > 30) {
      size -= 8;
      x.font = `${size}px "${font}"`;
    }
    x.fillText(typed, 30, c.height / 2);
    return c;
  }

  // ---------- a picture of a signature ----------
  function pickImage(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;
    const url = URL.createObjectURL(file);
    const im = new Image();
    im.onload = () => {
      const scale = Math.min(1, 1600 / Math.max(im.width, im.height));
      const c = document.createElement("canvas");
      c.width = Math.round(im.width * scale);
      c.height = Math.round(im.height * scale);
      const x = c.getContext("2d")!;
      x.drawImage(im, 0, 0, c.width, c.height);
      // Paper white → see-through, so the signature sits on the page like ink.
      const d = x.getImageData(0, 0, c.width, c.height);
      for (let i = 0; i < d.data.length; i += 4) {
        const v = (d.data[i] + d.data[i + 1] + d.data[i + 2]) / 3;
        if (v > 215) d.data[i + 3] = 0;
        else if (v > 170) d.data[i + 3] = Math.round(((215 - v) / 45) * d.data[i + 3]);
      }
      x.putImageData(d, 0, 0);
      imageSrc = trimmed(c).toDataURL("image/png");
      URL.revokeObjectURL(url);
    };
    im.src = url;
  }

  const ready = $derived(tab === "draw" ? drawn : tab === "type" ? typed.trim().length > 0 : !!imageSrc);

  function use(src?: string) {
    let out = src;
    if (!out) {
      if (tab === "draw") out = trimmed(canvas!).toDataURL("image/png");
      else if (tab === "type") out = trimmed(typedCanvas()).toDataURL("image/png");
      else out = imageSrc;
      if (remember) keep(out);
    }
    const im = new Image();
    im.onload = () => onpick(out!, im.width / Math.max(1, im.height));
    im.src = out;
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label={L({ ro: "Semnătura", en: "Signature" })}>
    <div class="head">
      <h3>{L({ ro: "Semnătura", en: "Signature" })}</h3>
      <button class="icon" onclick={onclose} aria-label={L({ ro: "Închide", en: "Close" })}><i class="ti ti-x"></i></button>
    </div>

    {#if saved.length}
      <p class="lbl">{L({ ro: "Păstrate pe acest PC", en: "Kept on this PC" })}</p>
      <div class="saved">
        {#each saved as s}
          <div class="sv">
            <button class="pick" onclick={() => use(s)} title={L({ ro: "Folosește", en: "Use" })}><img src={s} alt="" /></button>
            <button class="icon del" onclick={() => forget(s)} title={L({ ro: "Uită", en: "Forget" })}><i class="ti ti-trash"></i></button>
          </div>
        {/each}
      </div>
    {/if}

    <div class="tabs" role="tablist">
      <button role="tab" class:on={tab === "draw"} aria-selected={tab === "draw"} onclick={() => (tab = "draw")}><i class="ti ti-writing-sign"></i>{L({ ro: "Desenează", en: "Draw" })}</button>
      <button role="tab" class:on={tab === "type"} aria-selected={tab === "type"} onclick={() => (tab = "type")}><i class="ti ti-typography"></i>{L({ ro: "Scrie", en: "Type" })}</button>
      <button role="tab" class:on={tab === "image"} aria-selected={tab === "image"} onclick={() => (tab = "image")}><i class="ti ti-photo"></i>{L({ ro: "Din poză", en: "From a picture" })}</button>
      <span class="grow"></span>
      {#if tab !== "image"}
        {#each INKS as c}
          <button class="ink" class:on={ink === c} style="--c:{c}" onclick={() => (ink = c)} aria-label={c}></button>
        {/each}
      {/if}
    </div>

    {#if tab === "draw"}
      <div class="pad">
        <canvas bind:this={canvas} width="1000" height="320" onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up}></canvas>
        <span class="line"></span>
        {#if !drawn}<span class="hint">{L({ ro: "Semnează aici cu mouse-ul, cu degetul sau cu un creion digital", en: "Sign here with the mouse, a finger or a pen" })}</span>{/if}
        {#if drawn}<button class="clear" onclick={clear}>{L({ ro: "Șterge", en: "Clear" })}</button>{/if}
      </div>
    {:else if tab === "type"}
      <!-- svelte-ignore a11y_autofocus -->
      <input class="typed" bind:value={typed} placeholder={L({ ro: "Numele tău", en: "Your name" })} autofocus spellcheck="false" />
      <div class="fonts">
        {#each FONTS as f}
          <button class:on={font === f} style="font-family:'{f}'; color:{ink}" onclick={() => (font = f)}>{typed || L({ ro: "Semnătura", en: "Signature" })}</button>
        {/each}
      </div>
    {:else}
      <label class="file">
        <input type="file" accept="image/png,image/jpeg,image/webp" onchange={pickImage} />
        <i class="ti ti-upload"></i>{L({ ro: "Alege o poză cu semnătura pe hârtie albă", en: "Choose a picture of the signature on white paper" })}
      </label>
      {#if imageSrc}<div class="pad img"><img src={imageSrc} alt="" /></div>{/if}
    {/if}

    <div class="foot">
      <label class="remember"><input type="checkbox" bind:checked={remember} />{L({ ro: "Păstreaz-o pe acest PC", en: "Keep it on this PC" })}</label>
      <span class="grow"></span>
      <button onclick={onclose}>{L({ ro: "Renunță", en: "Cancel" })}</button>
      <button class="primary" disabled={!ready} onclick={() => use()}>{L({ ro: "Pune pe pagină", en: "Place on the page" })}</button>
    </div>
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: rgba(10, 14, 22, 0.35); display: grid; place-items: center; z-index: 50; }
  .dialog { width: min(640px, calc(100vw - 40px)); max-height: calc(100vh - 40px); overflow: auto; background: var(--dialog); border-radius: var(--radius-lg); box-shadow: 0 12px 40px rgba(0, 0, 0, 0.28); padding: 16px 18px; display: flex; flex-direction: column; gap: 12px; }
  .head { display: flex; align-items: center; justify-content: space-between; }
  h3 { margin: 0; font-size: 16px; }
  .lbl { margin: 0; font-size: 12px; color: var(--text-3); }
  .saved { display: flex; gap: 8px; flex-wrap: wrap; }
  .sv { position: relative; }
  .pick { width: 140px; height: 54px; padding: 4px; background: #fff; border: 1px solid var(--border); border-radius: var(--radius); }
  .pick img { max-width: 100%; max-height: 100%; }
  .sv .del { position: absolute; top: -8px; right: -8px; width: 24px; height: 24px; padding: 0; border-radius: 50%; background: var(--dialog); border: 1px solid var(--border); font-size: 12px; }
  .tabs { display: flex; gap: 6px; align-items: center; }
  .tabs [role="tab"] { display: flex; gap: 6px; align-items: center; padding: 5px 12px; }
  .tabs [role="tab"].on { background: var(--accent-bg); border-color: var(--accent-border); color: var(--accent); }
  .grow { flex: 1; }
  .ink { width: 22px; height: 22px; min-width: 0; min-height: 0; padding: 0; border-radius: 50%; background: var(--c); border: 2px solid transparent; }
  .ink.on { box-shadow: 0 0 0 2px var(--dialog), 0 0 0 4px var(--c); }
  .pad { position: relative; background: #fff; border: 1px solid var(--border-strong); border-radius: var(--radius); height: 190px; }
  .pad canvas { width: 100%; height: 100%; touch-action: none; cursor: crosshair; display: block; }
  .pad .line { position: absolute; left: 24px; right: 24px; bottom: 44px; border-bottom: 1px dashed #b8bec9; pointer-events: none; }
  .pad .hint { position: absolute; inset: 0; display: grid; place-items: center; color: #8b93a1; font-size: 13px; pointer-events: none; }
  .pad .clear { position: absolute; top: 8px; right: 8px; padding: 2px 10px; font-size: 12px; }
  .pad.img { display: grid; place-items: center; }
  .pad.img img { max-width: 90%; max-height: 90%; }
  .typed { font-size: 15px; padding: 8px 10px; }
  .fonts { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .fonts button { height: 64px; font-size: 28px; background: #fff; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
  .fonts button.on { border-color: var(--accent-fill); box-shadow: 0 0 0 1px var(--accent-fill); }
  .file { display: flex; gap: 8px; align-items: center; padding: 14px; border: 1.5px dashed var(--border-dash); border-radius: var(--radius); cursor: pointer; color: var(--text-2); }
  .file input { display: none; }
  .foot { display: flex; align-items: center; gap: 8px; }
  .remember { display: flex; align-items: center; gap: 6px; font-size: 13px; }
</style>
