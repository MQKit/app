<script lang="ts">
  // "Record…": the microphone, through the window. The recording is saved in
  // Documents\MQKit when it stops, then added like any other file. Nothing leaves the PC.
  // (The Windows default microphone is used; it is chosen in the Windows sound settings.)
  import { api, type FileInfo } from "../lib/api";
  import { L } from "../lib/i18n.svelte";
  import { onDestroy } from "svelte";

  let { onadd }: { onadd: (files: FileInfo[]) => void } = $props();

  let phase = $state<"idle" | "starting" | "recording" | "paused" | "saving">("idle");
  let error = $state("");
  let seconds = $state(0);
  let level = $state(0);

  let stream: MediaStream | null = null;
  let rec: MediaRecorder | null = null;
  let chunks: Blob[] = [];
  let ctx: AudioContext | null = null;
  let raf = 0;
  let timer: ReturnType<typeof setInterval> | undefined;

  async function start() {
    error = "";
    phase = "starting";
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        // The cleanup happens after (noise, level): the raw voice is kept as it is.
        audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 },
      });
    } catch (e: any) {
      phase = "idle";
      error = e?.name === "NotAllowedError"
        ? L({ ro: "Accesul la microfon e oprit. Pornește-l în Setări Windows › Confidențialitate › Microfon.", en: "Microphone access is off. Turn it on in Windows Settings › Privacy › Microphone." })
        : L({ ro: "Nu am găsit niciun microfon.", en: "No microphone was found." });
      return;
    }
    const type = ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/webm"].find((t) => MediaRecorder.isTypeSupported(t)) ?? "";
    rec = new MediaRecorder(stream, { mimeType: type, audioBitsPerSecond: 128000 });
    chunks = [];
    rec.ondataavailable = (e) => e.data.size && chunks.push(e.data);
    rec.onstop = save;
    rec.start(1000);
    seconds = 0;
    timer = setInterval(() => phase === "recording" && (seconds += 1), 1000);
    meter();
    phase = "recording";
  }

  /** The input level, for the bar that shows the microphone hears something. */
  function meter() {
    ctx = new AudioContext();
    const an = ctx.createAnalyser();
    an.fftSize = 1024;
    ctx.createMediaStreamSource(stream!).connect(an);
    const buf = new Float32Array(an.fftSize);
    const tick = () => {
      an.getFloatTimeDomainData(buf);
      let peak = 0;
      for (const v of buf) peak = Math.max(peak, Math.abs(v));
      level = Math.min(1, peak * 1.4);
      raf = requestAnimationFrame(tick);
    };
    tick();
  }

  function pause() {
    if (rec?.state === "recording") {
      rec.pause();
      phase = "paused";
    } else if (rec?.state === "paused") {
      rec.resume();
      phase = "recording";
    }
  }

  function stop() {
    if (!rec) return;
    phase = "saving";
    rec.stop();
  }

  function release() {
    clearInterval(timer);
    cancelAnimationFrame(raf);
    stream?.getTracks().forEach((t) => t.stop());
    ctx?.close().catch(() => {});
    stream = null;
    ctx = null;
    level = 0;
  }

  async function save() {
    const type = rec?.mimeType ?? "audio/webm";
    release();
    rec = null;
    try {
      const blob = new Blob(chunks, { type });
      const ext = type.includes("ogg") ? "ogg" : type.includes("mp4") ? "mp4" : "webm";
      const f = await api.saveRecording(new Uint8Array(await blob.arrayBuffer()), ext, L({ ro: "Înregistrare", en: "Recording" }));
      onadd([f]);
    } catch {
      error = L({ ro: "Înregistrarea nu a putut fi salvată în Documente\\MQKit.", en: "The recording couldn't be saved in Documents\\MQKit." });
    }
    chunks = [];
    phase = "idle";
  }

  // Leaving the tool stops the microphone (the recording so far is kept).
  onDestroy(() => {
    if (rec && rec.state !== "inactive") rec.stop();
    else release();
  });

  const clock = $derived(`${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`);
</script>

{#if phase === "idle" || phase === "starting"}
  <button class="link" onclick={start} disabled={phase === "starting"}><i class="ti ti-microphone"></i>{L({ ro: "Înregistrează…", en: "Record…" })}</button>
{:else}
  <div class="panel" role="status">
    <span class="dot" class:paused={phase === "paused"}></span>
    <span class="clock">{clock}</span>
    <span class="meter" aria-hidden="true"><span style="width:{Math.round(level * 100)}%"></span></span>
    <span class="grow"></span>
    <button onclick={pause} disabled={phase === "saving"}>
      <i class="ti {phase === 'paused' ? 'ti-player-play' : 'ti-player-pause'}"></i>{phase === "paused" ? L({ ro: "Continuă", en: "Resume" }) : L({ ro: "Pauză", en: "Pause" })}
    </button>
    <button class="primary" onclick={stop} disabled={phase === "saving"}><i class="ti ti-player-stop"></i>{L({ ro: "Gata", en: "Stop" })}</button>
  </div>
{/if}
{#if error}<p class="err">{error}</p>{/if}

<style>
  .link { display: flex; align-items: center; gap: 6px; background: none; border: none; color: var(--accent); padding: 6px 2px; cursor: pointer; font-size: 13px; }
  .panel { display: flex; align-items: center; gap: 10px; width: 100%; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--surface-1); margin-top: 6px; }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: #d92d20; animation: blink 1s infinite; }
  .dot.paused { animation: none; background: var(--text-3); }
  @keyframes blink { 50% { opacity: 0.3; } }
  .clock { font-variant-numeric: tabular-nums; min-width: 40px; }
  .meter { width: 90px; height: 6px; border-radius: 3px; background: var(--hover); overflow: hidden; }
  .meter span { display: block; height: 100%; background: #12b76a; transition: width 60ms linear; }
  .grow { flex: 1; }
  .panel button { display: flex; align-items: center; gap: 6px; padding: 4px 12px; }
  .err { color: var(--error-text); font-size: 12px; margin: 4px 0 0; width: 100%; }
</style>
