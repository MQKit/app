// Condition expressions of contract v2, section 7.1.
import type { Cond } from "./api";

export interface CondCtx {
  values: Record<string, any>;
  /** Analysis props of the files in the batch (only analysed ones). */
  files: Record<string, any>[];
  batch: { count: number; totalBytes: number; extensions: string[] };
  env: Record<string, any>;
  item?: Record<string, any>;
}

function get(obj: any, path: string): any {
  if (obj == null) return undefined;
  if (path in obj) return obj[path];
  let cur = obj;
  for (const part of path.split(".")) {
    if (cur == null) return undefined;
    if (part === "count" && Array.isArray(cur)) return cur.length;
    cur = cur[part];
  }
  return cur;
}

const OPS = ["eq", "ne", "in", "notIn", "gt", "gte", "lt", "lte", "contains", "empty", "known"];

function compare(v: any, c: Cond): boolean {
  if ("known" in c) return (v !== undefined) === !!c.known;
  if (v === undefined) return false;
  if ("eq" in c) return v === c.eq || (typeof v === "number" && Number(c.eq) === v);
  if ("ne" in c) return v !== c.ne;
  if ("in" in c) return (c.in as any[]).includes(v);
  if ("notIn" in c) return !(c.notIn as any[]).includes(v);
  if ("gt" in c) return Number(v) > c.gt;
  if ("gte" in c) return Number(v) >= c.gte;
  if ("lt" in c) return Number(v) < c.lt;
  if ("lte" in c) return Number(v) <= c.lte;
  if ("contains" in c) return Array.isArray(v) ? v.includes(c.contains) : String(v).includes(c.contains);
  if ("empty" in c) {
    const isEmpty = v === "" || v == null || (Array.isArray(v) && v.length === 0);
    return isEmpty === !!c.empty;
  }
  return false;
}

export function evalCond(c: Cond | undefined, ctx: CondCtx): boolean {
  if (!c) return true;
  if (c.all) return (c.all as Cond[]).every((x) => evalCond(x, ctx));
  if (c.any) return (c.any as Cond[]).some((x) => evalCond(x, ctx));
  if (c.not) return !evalCond(c.not, ctx);
  if ("field" in c) return compare(ctx.values[c.field], c);
  if ("item" in c) return compare(ctx.item?.[c.item], c);
  if ("env" in c) return compare(ctx.env[c.env], c);
  if ("files" in c) return compare(get(ctx.batch, c.files), c);
  if ("file" in c) {
    const quant = c.quant ?? "any";
    const results = ctx.files.map((f) => compare(get(f, c.file), c));
    if (quant === "all") return results.length > 0 && results.every(Boolean);
    if (quant === "none") return !results.some(Boolean);
    return results.some(Boolean);
  }
  return OPS.some((o) => o in c) ? false : true;
}

/** `default` may be a value or a list of `{when, value}` cases; the first match wins. */
export function resolveDefault(def: any, ctx: CondCtx): any {
  if (Array.isArray(def) && def.length && typeof def[0] === "object" && def[0] !== null && "value" in def[0]) {
    for (const c of def) if (!c.when || evalCond(c.when, ctx)) return c.value;
    return undefined;
  }
  return def;
}
