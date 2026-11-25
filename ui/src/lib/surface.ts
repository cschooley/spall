// The Surface (#95, #94): endpoints seen in traffic or documented in an
// imported API description, where each stands in scope, and their coverage
// per WSTG category: found (a candidate), tested (a sweep, an agent's or
// your mark), or untested. Every string here came from captured traffic:
// text only.
import { apiJson } from "./engagement.ts";

export interface Category {
  code: string;
  name: string;
}

export interface Cell {
  state: "found" | "tested";
  candidates: string[];
  by: string[];
  how: string[];
}

export interface SurfaceRow {
  key: string;
  host: string;
  method: string;
  template: string;
  count: number;
  statuses: number[];
  params: string[];
  firstSeen: string;
  lastSeen: string;
  example: string | null;
  byYou: number;
  tasks: string[];
  documented: boolean;
  scope: "in" | "out" | "unknown";
  coverage: Record<string, Cell>;
}

export interface SurfaceListing {
  categories: Category[];
  rows: SurfaceRow[];
  dropped: number;
}

const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

export function asSurface(v: unknown): SurfaceListing | string {
  if (typeof v === "string") return v;
  const o = obj(v);
  if (!Array.isArray(o.endpoints)) return "unexpected answer from spall";
  const categories = (Array.isArray(o.categories) ? o.categories : []).map((c) => ({ code: str(obj(c).code), name: str(obj(c).name) })).filter((c) => c.code);
  const rows = o.endpoints.map((e): SurfaceRow => {
    const x = obj(e);
    const coverage: Record<string, Cell> = {};
    for (const [k, c] of Object.entries(obj(x.coverage))) {
      const cc = obj(c);
      if (cc.state !== "found" && cc.state !== "tested") continue;
      coverage[k] = { state: cc.state, candidates: strs(cc.candidates), by: strs(cc.by), how: strs(cc.how) };
    }
    const scope = x.scope === "in" || x.scope === "out" ? x.scope : "unknown";
    return {
      key: str(x.key),
      host: str(x.host),
      method: str(x.method),
      template: str(x.template),
      count: num(x.count),
      statuses: Array.isArray(x.statuses) ? x.statuses.filter((s): s is number => typeof s === "number") : [],
      params: strs(x.params),
      firstSeen: str(x.first_seen),
      lastSeen: str(x.last_seen),
      example: typeof x.example === "string" ? x.example : null,
      byYou: num(x.by_you),
      tasks: strs(x.tasks),
      documented: x.documented === true,
      scope,
      coverage,
    };
  });
  return { categories, rows, dropped: num(o.dropped) };
}

export async function loadSurface(all: boolean): Promise<SurfaceListing | string> {
  return asSurface(await apiJson(`/api/surface${all ? "?scope=all" : ""}`));
}

/** Mark an endpoint tested for a category (audited); null, or why not. */
export async function markCoverage(key: string, category: string, note: string): Promise<string | null> {
  const r = await apiJson("/api/coverage", { method: "POST", body: { key, category, note } });
  return typeof r === "string" ? r : null;
}

/**
 * The Surface filter: words, all of which must match. `untested:ATHZ` (no
 * cell for that category), `tested:INPV`, `found:SESS` (or `found` for
 * any), `host:<text>`, `method:POST`, `documented`, `agent` (an agent task
 * sent to it), anything else a substring of host + template.
 */
export function filterRows(rows: readonly SurfaceRow[], query: string): SurfaceRow[] {
  const words = query.trim().split(/\s+/).filter(Boolean);
  const test = (r: SurfaceRow, w: string): boolean => {
    const m = /^(untested|tested|found|host|method):(.*)$/i.exec(w);
    if (m) {
      const k = m[1] ?? "";
      const v = m[2] ?? "";
      const val = v.toUpperCase();
      switch (k.toLowerCase()) {
        case "untested":
          return !r.coverage[val];
        case "tested":
          return r.coverage[val]?.state === "tested";
        case "found":
          return r.coverage[val]?.state === "found";
        case "host":
          return r.host.toLowerCase().includes(v.toLowerCase());
        case "method":
          return r.method === val || (r.method === "ANY" && val !== "");
      }
    }
    if (w === "found") return Object.values(r.coverage).some((c) => c.state === "found");
    if (w === "documented") return r.documented;
    if (w === "agent") return r.tasks.length > 0;
    return `${r.host}${r.template}`.toLowerCase().includes(w.toLowerCase());
  };
  return rows.filter((r) => words.every((w) => test(r, w)));
}

/** Per category: how many endpoints have a finding, are tested, or neither. */
export function totals(rows: readonly SurfaceRow[], categories: readonly Category[]): Record<string, { found: number; tested: number; untested: number }> {
  const out: Record<string, { found: number; tested: number; untested: number }> = {};
  for (const c of categories) out[c.code] = { found: 0, tested: 0, untested: 0 };
  for (const r of rows)
    for (const c of categories) {
      const t = out[c.code];
      if (t) t[r.coverage[c.code]?.state ?? "untested"] += 1;
    }
  return out;
}

/** The one-character mark for a cell: text, so it reads without color. */
export function mark(cell: Cell | undefined): string {
  return cell?.state === "found" ? "!" : cell?.state === "tested" ? "✓" : "·";
}

/** A row for the console. */
export function rowLine(r: SurfaceRow, categories: readonly Category[]): string {
  const marks = categories.map((c) => mark(r.coverage[c.code])).join("");
  const who = [r.byYou ? "you" : "", ...r.tasks.map((t) => `◆${t}`), r.documented ? "doc" : ""].filter(Boolean).join(",");
  return `${r.method.padEnd(6)} ${r.host}${r.template}  ${r.count}× ${marks}${who ? `  ${who}` : ""}`;
}
