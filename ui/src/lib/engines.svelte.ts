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
};

/** Download state per engine, shared by every screen. */
export const downloads = $state<Record<string, { done: number; total: number; error?: string; running: boolean }>>({});

export async function refreshEnv() {
  app.env = await api.env();
}

/** Starts (or joins) a download; resolves true when installed. */
export function downloadEngine(name: string): Promise<boolean> {
  return new Promise((resolve) => {
    const cur = downloads[name];
    if (cur?.running) return resolve(false);
    downloads[name] = { done: 0, total: 0, running: true };
    api.engineDownload(name, async (e) => {
      const d = downloads[name];
      if (e.type === "progress") {
        d.done = e.done;
        d.total = e.total;
      } else if (e.type === "done") {
        d.running = false;
        await refreshEnv();
        resolve(true);
      } else if (e.type === "error") {
        d.running = false;
        d.error = e.code;
        await refreshEnv();
        resolve(false);
      }
    });
  });
}
