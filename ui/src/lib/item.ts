import type { FileInfo } from "./api";

export interface Item {
  id: string;
  info: FileInfo;
  status: "analyzing" | "ok" | "locked" | "unsupported" | "corrupt" | "failed";
  props: Record<string, any>;
  code?: string;
  /** What the worker said about the error (filled into its message). */
  details?: Record<string, any>;
  item: Record<string, any>;
  // during and after a run
  state?: "waiting" | "running" | "ok" | "failed" | "skipped";
  progress?: number;
  stage?: string;
  /** `originalKept`: the original stayed although it was to be recycled or replaced (why). */
  results?: { path: string; name: string; bytes: number; skipped?: boolean; originalKept?: "no_recycle_bin" | "file_locked" }[];
  suggest?: Record<string, any>;
  /** Warning codes the worker sent for this file. */
  warnings?: string[];
  /** Their details, by code (filled into the message). */
  warningDetails?: Record<string, any>;
}

let n = 0;
export const newId = () => `f${++n}`;
