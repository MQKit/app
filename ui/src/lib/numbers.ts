/**
 * Every tool's permanent number, so people can name a tool in any language ("ID:12").
 *
 * The same numbers are on the website (website/src/data/tools.ts, `num`).
 * Never change a number and never reuse one: a new tool takes the next free number,
 * and a retired tool's number stays retired.
 */
export const TOOL_NUMBERS: Record<string, number> = {
  // The ten tools of version 1.0, the most used.
  "pdf-compress": 1,
  "pdf-to-word": 2,
  "pdf-merge": 3,
  "images-to-pdf": 4,
  "word-to-pdf": 5,
  "pdf-to-images": 6,
  "image-compress": 7,
  "image-convert": 8,
  "heic-to-jpg": 9,
  "video-to-mp3": 10,
  // PDF
  "pdf-split": 11,
  "pdf-pages": 12,
  "pdf-password": 13,
  "pdf-stamp": 14,
  ocr: 15,
  "pdf-to-excel": 16,
  "pdf-to-pptx": 17,
  // Images
  "image-resize": 18,
  "ai-upscale": 19,
  "remove-background": 20,
  raw: 21,
  avif: 22,
  svg: 23,
  icons: 24,
  "design-files": 25,
  // Video
  "video-compress": 26,
  "video-convert": 27,
  "video-gif": 28,
  "media-trim": 29,
  "media-merge": 30,
  subtitles: 31,
  // Audio
  "audio-convert": 32,
  transcribe: 33,
  tts: 34,
  // Documents
  "office-to-pdf": 35,
  "html-to-pdf": 36,
  markdown: 37,
  tabular: 38,
  ebooks: 39,
  mail: 40,
  // Other
  archives: 41,
  fonts: 42,
  "meta-strip": 43,
  "cad-3d": 44,
  // Version 1.2
  "qr-code": 45,
  "pdf-edit": 46,
  "pdf-redact": 47,
  "photo-erase": 48,
  "images-to-gif": 49,
  "video-crop": 50,
  "voice-record": 51,
  "music-split": 52,
  "music-info": 53,
};

export function toolNumber(id: string): number | undefined {
  return TOOL_NUMBERS[id];
}

/** "ID:12", "id 12", "#12" or just "12" typed in the search box. */
export function numberQuery(q: string): number | undefined {
  const m = q.trim().match(/^(?:id\s*:?\s*|#)?(\d{1,3})$/i);
  return m ? Number(m[1]) : undefined;
}
