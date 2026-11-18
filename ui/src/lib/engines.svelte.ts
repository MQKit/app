import { api } from "./api";
import { app } from "./store.svelte";

export const ENGINE_NAMES: Record<string, string> = {
  ffmpeg: "FFmpeg",
  libreoffice: "LibreOffice",
  pdf2docx: "pdf2docx",
  pymupdf: "PyMuPDF",
  onnxruntime: "ONNX Runtime",
  isnet: "IS-Net",
  "birefnet-lite": "BiRefNet",
  "esrgan-general": "Real-ESRGAN",
  "esrgan-x4plus": "Real-ESRGAN x4plus",
  "whisper-small": "Whisper small",
  "whisper-turbo": "Whisper large-v3-turbo",
  python: "Python",
  kokoro: "Kokoro",
  "kokoro-model": "Kokoro (model)",
  "kokoro-voices": "Kokoro (voices)",
};

/** Download state per engine, shared by every screen. `installed` stays true after a
 *  download finishes, until the engine is removed: screens that still hold an older list
 *  show it installed instead of offering the download again. */
export const downloads = $state<Record<string, { done: number; total: number; error?: string; running: boolean; installed?: boolean }>>({});

/** Counts finished downloads (and removals): screens that list engines reload when it
 *  changes, so an engine installed from another screen, or as another's requirement,
 *  shows up without leaving the page. */
export const engineEvents = $state({ changed: 0 });

/** Everyone waiting for a download already running (another screen started it). */
const waiting: Record<string, ((ok: boolean) => void)[]> = {};

export async function refreshEnv() {
  app.env = await api.env();
}

/** Starts a download, or joins the one running; resolves true when installed. */
export function downloadEngine(name: string): Promise<boolean> {
  return new Promise((resolve) => {
    (waiting[name] ??= []).push(resolve);
    if (downloads[name]?.running) return;
    downloads[name] = { done: 0, total: 0, running: true };
    const finish = async (ok: boolean, code?: string) => {
      const d = downloads[name];
      d.installed = ok;
      d.error = code;
      d.running = false;
      await refreshEnv();
      engineEvents.changed++;
      for (const r of waiting[name].splice(0)) r(ok);
    };
    api.engineDownload(name, (e) => {
      if (e.type === "progress") {
        downloads[name].done = e.done;
        downloads[name].total = e.total;
      } else if (e.type === "done") {
        finish(true);
      } else if (e.type === "error") {
        finish(false, e.code);
      }
    });
  });
}

/** After an engine was removed: no longer shown as installed anywhere. */
export function engineRemoved(name: string) {
  if (downloads[name]) downloads[name].installed = false;
  engineEvents.changed++;
}
