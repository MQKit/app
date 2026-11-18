import { invoke, Channel, convertFileSrc } from "@tauri-apps/api/core";

export type Loc = { ro?: string; en?: string; [k: string]: string | undefined } | string;
export type Cond = Record<string, any>;

export interface FieldOption {
  value: any;
  label?: Loc;
  help?: Loc;
  showIf?: Cond;
  detail?: string;
}

export interface Field {
  key: string;
  type: string;
  label?: Loc;
  help?: Loc;
  /** A button under the help that opens a page (a Windows Settings page, for example). */
  helpLink?: { label: Loc; url: string };
  default?: any;
  advanced?: boolean;
  group?: string;
  showIf?: Cond;
  enabledIf?: Cond;
  options?: FieldOption[];
  optionsFrom?: { file?: string; env?: string; combine?: string };
  allowCustom?: { type: string; min?: number; max?: number; unit?: string };
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
  units?: { base: string; display: { id: string; factor?: number }[]; defaultDisplay?: string };
  suggestions?: { value: any; label: Loc }[];
  placeholder?: Loc;
  display?: string;
  syntax?: string;
}

export interface Rule {
  id: string;
  when: Cond;
  severity: "error" | "warning" | "confirm" | "info";
  fields?: string[];
  message?: Loc;
}

export interface Manifest {
  id: string;
  version: string;
  name: Loc;
  summary: Loc;
  keywords?: Record<string, string[]>;
  category: string;
  icon?: string;
  entry?: { id?: string; when?: Cond; kind: string }[];
  requires?: { engine?: string; env?: string; eq?: any; when?: Cond }[];
  /** `textEntry`: the tool also takes typed text, saved as this kind of file. */
  input: { extensions: string[]; anyFile?: boolean; multiple?: boolean; mode?: string; ordered?: boolean; minFiles?: number; itemFields?: Field[]; textEntry?: "txt" | "md" | "html" };
  output: { kind: string; extensions: string[]; replaceOriginalAllowed?: boolean; nameVariables?: { id: string; label: Loc }[] };
  analysis?: { fast?: string[] };
  ui?: { preview?: string; estimate?: boolean; inputList?: string };
  resources?: { class?: string };
  presets?: { id: string; label: Loc; help?: Loc; values: Record<string, any>; default?: boolean }[];
  groups?: { id: string; label: Loc; order?: number }[];
  fields: Field[];
  rules?: Rule[];
  outputBlock?: { defaults?: Record<string, any>; hide?: string[] };
  errors?: Record<string, Loc>;
}

export interface FileInfo {
  path: string;
  name: string;
  ext: string;
  size: number;
  modified: number;
  base?: string | null;
}

export interface OutputBlock {
  outputDir: "sameAsInput" | "subfolder" | "custom";
  subfolderName: string;
  outputDirPath: string;
  nameTemplate: string;
  onExisting: "rename" | "overwrite" | "skip";
  originals: "keep" | "replace" | "recycle";
  keepFileDate: boolean;
  whenDone: "nothing" | "openFolder" | "openFile";
}

export const api = {
  tools: () => invoke<Manifest[]>("get_tools"),
  env: () => invoke<Record<string, any>>("get_env"),
  expand: (paths: string[]) => invoke<FileInfo[]>("expand_paths", { paths }),
  findFiles: (root: string, pattern: string, recursive: boolean, hidden: boolean, extensions: string[]) =>
    invoke<{ files: FileInfo[]; matched: number; bytes: number; truncated: boolean }>("find_files", { root, pattern, recursive, hidden, extensions }),
  linkDownload: (id: string, url: string, extensions: string[], onEvent: (e: any) => void) => {
    const ch = new Channel<any>();
    ch.onmessage = onEvent;
    return invoke<void>("link_download", { id, url, extensions, onEvent: ch });
  },
  linkCancel: (id: string) => invoke<void>("link_cancel", { id }),
  stateGet: (name: string) => invoke<any>("state_get", { name }),
  stateSet: (name: string, value: any) => invoke<void>("state_set", { name, value }),
  analyze: (tool: string, files: { id: string; path: string }[], passwords?: Record<string, string>) =>
    invoke<any[]>("analyze", { tool, files, passwords }),
  estimate: (tool: string, files: { id: string; path: string; analysis: any }[], options: Record<string, any>) =>
    invoke<any>("estimate", { tool, files, options }),
  preview: (tool: string, file: { id: string; path: string; analysis: any }, options: Record<string, any>, seq: number, maxPx: number, rect?: number[]) =>
    invoke<any>("preview_image", { tool, file, options, seq, maxPx, rect }),
  startJob: (job: any, onEvent: (e: any) => void) => {
    const ch = new Channel<any>();
    ch.onmessage = onEvent;
    return invoke<string>("start_job", { job, onEvent: ch });
  },
  cancelJob: (jobId: string, afterCurrent = false) => invoke<void>("cancel_job", { jobId, afterCurrent }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  engines: () => invoke<any[]>("engines_list"),
  engineDownload: (name: string, onEvent: (e: any) => void) => {
    const ch = new Channel<any>();
    ch.onmessage = onEvent;
    return invoke<void>("engine_download", { name, onEvent: ch });
  },
  engineCancel: (name: string) => invoke<void>("engine_cancel", { name }),
  engineRemove: (name: string) => invoke<void>("engine_remove", { name }),
  checkUpdate: () => invoke<any>("check_update"),
  initialFiles: () => invoke<{ tool: string | null; files: FileInfo[] }>("initial_files"),
  diagnostics: () => invoke<string>("diagnostics"),
  /** Typed text saved as a file in Documents\\MQKit (`ext`: txt, md or html). */
  saveText: (text: string, ext: string, name: string) => invoke<FileInfo>("save_text", { text, ext, name }),
  /** Files from Explorer that arrived before the interface listened (then events only). */
  shellReady: () => invoke<{ tool: string | null; files: FileInfo[] }[]>("shell_ready"),
  /** `names`: each tool's name in the interface's language, for the menu's labels. */
  shellMenu: (enable: boolean, lang: string, names: Record<string, string>) => invoke<number>("shell_menu", { enable, lang, names }),
  appInfo: () => invoke<any>("app_info"),
};

export const fileSrc = (p: string) => convertFileSrc(p);
