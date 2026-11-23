// spall's rule catalog (#41): WSTG v4.2 test ids mapped to CWE, from
// /api/rules. The server owns the catalog; the UI lists and searches it.

export interface Rule {
  id: string;
  title: string;
  cwe: number | null;
  help: string | null;
}

export interface Catalog {
  wstg: string;
  attribution: string;
  rules: Rule[];
}

import { apiJson } from "./engagement.ts";

export const OTHER = "spall/other";

let cached: Catalog | null = null;

/** The catalog from /api/rules, fetched once per page (it's static). */
export async function loadCatalog(): Promise<Catalog | string> {
  if (cached) return cached;
  const body = await apiJson("/api/rules");
  if (typeof body === "string") return body;
  cached = asCatalog(body);
  return cached ?? "unexpected answer from spall";
}

export function asCatalog(v: unknown): Catalog | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (!Array.isArray(o.rules)) return null;
  const rules = o.rules.flatMap((x: unknown): Rule[] => {
    const r = x as Record<string, unknown> | null;
    if (!r || typeof r.id !== "string" || typeof r.title !== "string") return [];
    return [
      {
        id: r.id,
        title: r.title,
        cwe: typeof r.cwe === "number" && Number.isInteger(r.cwe) ? r.cwe : null,
        help: typeof r.help === "string" ? r.help : null,
      },
    ];
  });
  return { wstg: typeof o.wstg === "string" ? o.wstg : "", attribution: typeof o.attribution === "string" ? o.attribution : "", rules };
}

/** Rules matching every word of `text` (id, title or `cwe-n`), any case. */
export function searchRules(rules: Rule[], text: string): Rule[] {
  const words = text.toLowerCase().split(/\s+/).filter(Boolean);
  return rules.filter((r) => {
    const hay = `${r.id} ${r.title} cwe-${r.cwe ?? ""}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** "WSTG-INPV-05 Testing for SQL Injection (CWE-89)" */
export function ruleLine(r: Rule): string {
  return `${r.id} ${r.title}${r.cwe !== null ? ` (CWE-${r.cwe})` : ""}`;
}
