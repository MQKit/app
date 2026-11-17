import { i18n } from "./i18n.svelte";

const nf = () => new Intl.NumberFormat(i18n.lang === "ro" ? "ro-RO" : "en-US", { maximumFractionDigits: 1 });

/** SI units, like the contract: 1 MB = 1 000 000 bytes. */
export function bytes(n: number | undefined | null): string {
  if (n == null || isNaN(n)) return "";
  const f = nf();
  if (n < 1000) return `${Math.round(n)} B`;
  if (n < 1e6) return `${f.format(n / 1e3)} KB`;
  if (n < 1e9) return `${f.format(n / 1e6)} MB`;
  return `${f.format(n / 1e9)} GB`;
}

export function duration(sec: number | undefined): string {
  if (!sec) return "";
  const s = Math.round(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(r).padStart(2, "0")}` : `${m}:${String(r).padStart(2, "0")}`;
}

export function percentChange(from: number, to: number): number {
  if (!from) return 0;
  return Math.round((1 - to / from) * 100);
}

export function dateTime(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString(i18n.lang === "ro" ? "ro-RO" : "en-US", { dateStyle: "medium", timeStyle: "short" });
}

/** Lowercase without diacritics, for search. */
export function norm(s: string): string {
  return s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
}
