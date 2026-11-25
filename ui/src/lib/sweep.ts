// sweep and checks (#75): the console's side. The server runs the sweep
// (every request gated and recorded) and answers with an estimate or a
// summary; checks are data, the engagement's own imported by the tester.
import { apiJson } from "./engagement.ts";

export interface CheckInfo {
  name: string;
  rule: string;
  title: string;
  method: string;
  paths: number;
  source: "built_in" | "engagement";
}

export interface SweepHit {
  path: string;
  exchange: string;
  status: number | null;
  size: number;
}

export type SweepAnswer =
  | { kind: "estimate"; check: string; rule: string; planned: number; cost: string }
  | { kind: "done"; check: string; rule: string; base: string; planned: number; sent: number; hits: SweepHit[]; misses: number; skipped: [string, string][]; stopped: string | null };

const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" ? v : 0);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

export function asChecks(v: unknown): CheckInfo[] | string {
  if (typeof v === "string") return v;
  const list = obj(v).checks;
  return (Array.isArray(list) ? list : []).map((c) => {
    const o = obj(c);
    return { name: str(o.name), rule: str(o.rule), title: str(o.title), method: str(o.method), paths: Array.isArray(o.paths) ? o.paths.length : 0, source: o.source === "engagement" ? "engagement" : "built_in" };
  });
}

export function asSweep(v: unknown): SweepAnswer | string {
  if (typeof v === "string") return v;
  const o = obj(v);
  if (typeof o.cost === "string") return { kind: "estimate", check: str(o.check), rule: str(o.rule), planned: num(o.planned), cost: o.cost };
  if (!Array.isArray(o.hits)) return "unexpected answer from spall";
  return {
    kind: "done",
    check: str(o.check),
    rule: str(o.rule),
    base: str(o.base),
    planned: num(o.planned),
    sent: num(o.sent),
    hits: o.hits.map((h) => {
      const x = obj(h);
      return { path: str(x.path), exchange: str(x.exchange), status: typeof x.status === "number" ? x.status : null, size: num(x.size) };
    }),
    misses: num(o.misses),
    skipped: (Array.isArray(o.skipped) ? o.skipped : []).flatMap((s) => (Array.isArray(s) && s.length === 2 ? [[str(s[0]), str(s[1])] as [string, string]] : [])),
    stopped: typeof o.stopped === "string" ? o.stopped : null,
  };
}

export async function loadChecks(): Promise<CheckInfo[] | string> {
  return asChecks(await apiJson("/api/checks"));
}

export async function runSweep(base: string, check: string, yes: boolean): Promise<SweepAnswer | string> {
  return asSweep(await apiJson("/api/sweep", { method: "POST", body: yes ? { base, check, yes: true } : { base, check } }));
}

export async function changeChecks(change: { op: "add"; text: string } | { op: "remove"; name: string }): Promise<CheckInfo[] | string> {
  return asChecks(await apiJson("/api/checks", { method: "POST", body: change }));
}

/** A finished sweep as console lines. */
export function sweepLines(s: Extract<SweepAnswer, { kind: "done" }>): string[] {
  const out = [`sweep ${s.check} on ${s.base}: ${s.sent} of ${s.planned} sent, ${s.hits.length} hit(s), ${s.misses} miss(es)${s.stopped ? `; stopped: ${s.stopped}` : ""}`];
  for (const h of s.hits) out.push(`  hit ${h.path}  ${h.status ?? "ERR"}  ${h.size} B  open ${h.exchange} · flag ${h.exchange} --rule ${s.rule}`);
  for (const [p, why] of s.skipped.slice(0, 10)) out.push(`  skipped ${p}: ${why}`);
  if (s.skipped.length > 10) out.push(`  … ${s.skipped.length - 10} more skipped`);
  return out;
}
