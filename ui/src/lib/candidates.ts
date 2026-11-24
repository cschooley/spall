// Candidates (#42-#46) as the UI sees them. The server stores flux
// findings (SARIF results); this reads the parts the UI shows, defensively:
// every string in a finding came from captured traffic or a tester, and is
// rendered as text only.

import { apiJson } from "./engagement.ts";

export type Level = "none" | "note" | "warning" | "error";
export const LEVELS: readonly Level[] = ["error", "warning", "note", "none"];
export type Status = "candidate" | "suspected" | "confirmed" | "fixed";
export type Disposition = "falsePositive" | "acceptedRisk" | "duplicate";

export interface Suppressed {
  disposition: Disposition | null;
  justification: string;
  /** For a duplicate: the original's correlationGuid. */
  duplicateOf: string | null;
}

export interface Evidence {
  method: string | null;
  target: string | null;
  status: number | null;
  noResponse: boolean;
}

export interface Candidate {
  id: string;
  created: string;
  updated: string;
  flaggedBy: string;
  ruleId: string;
  level: Level | null;
  rank: number | null;
  message: string;
  status: Status | null;
  method: string | null;
  url: string | null;
  parameter: string | null;
  cwes: number[];
  /** Exchanges backing it, decisive one first. */
  exchanges: string[];
  guid: string | null;
  /** The decision in effect, if any. */
  suppressed: Suppressed | null;
  confirmedBy: string[];
  evidence: Evidence | null;
  /** When a human promoted it for export (#50); null if not. */
  promoted: string | null;
  /** Flagged anyway although it looked like this candidate (#78). */
  possibleDuplicateOf: string | null;
  /** Each later time the same issue was proposed or flagged (#78). */
  seenAgain: Sighting[];
}

/** One more time an issue was seen (#78). */
export interface Sighting {
  by: string;
  at: string;
  exchanges: string[];
}

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const oneOf = <T extends string>(v: unknown, all: readonly T[]): T | null => (all.includes(v as T) ? (v as T) : null);

const STATUSES: readonly Status[] = ["candidate", "suspected", "confirmed", "fixed"];
const DISPOSITIONS: readonly Disposition[] = ["falsePositive", "acceptedRisk", "duplicate"];

export function asCandidate(v: unknown): Candidate | null {
  const c = obj(v);
  const f = obj(c.finding);
  const id = str(c.id);
  const ruleId = str(f.ruleId);
  if (!id || ruleId === null) return null;
  const props = obj(f.properties);
  const loc = obj(Array.isArray(f.locations) ? f.locations.find((l) => Array.isArray(obj(l).logicalLocations)) : null);
  const method = Array.isArray(loc.logicalLocations) ? str(obj(loc.logicalLocations.find((l) => obj(l).kind === "httpMethod")).name) : null;
  const url = str(obj(obj(loc.physicalLocation).artifactLocation).uri);
  const taxa = Array.isArray(f.taxa) ? f.taxa : [];
  const cwes = taxa.flatMap((t) => {
    const o = obj(t);
    const n = Number(o.id);
    return obj(o.toolComponent).name === "CWE" && Number.isInteger(n) && n > 0 ? [n] : [];
  });
  const inEffect = (Array.isArray(f.suppressions) ? f.suppressions : []).map(obj).find((s) => s.status === undefined || s.status === "accepted");
  const sp = obj(inEffect?.properties);
  const req = obj(f.webRequest);
  const res = obj(f.webResponse);
  const rank = typeof f.rank === "number" && Number.isFinite(f.rank) ? f.rank : null;
  return {
    id,
    created: str(c.created) ?? "",
    updated: str(c.updated) ?? "",
    flaggedBy: str(c.flagged_by) ?? "",
    ruleId,
    level: oneOf(f.level, LEVELS),
    rank,
    message: str(obj(f.message).text) ?? "",
    status: oneOf(props["flux/status"], STATUSES),
    method,
    url,
    parameter: str(props["spall/parameter"]),
    cwes,
    exchanges: strs(props["flux/replayIds"]),
    guid: str(f.correlationGuid),
    suppressed: inEffect
      ? { disposition: oneOf(sp["flux/disposition"], DISPOSITIONS), justification: str(inEffect.justification) ?? "", duplicateOf: str(sp["flux/duplicateOf"]) }
      : null,
    confirmedBy: strs(props["flux/confirmedBy"]),
    promoted: str(c.promoted),
    possibleDuplicateOf: str(c.possible_duplicate_of),
    seenAgain: (Array.isArray(c.seen_again) ? c.seen_again : []).flatMap((x) => {
      const o = obj(x);
      const by = str(o.by);
      const at = str(o.at);
      return by !== null && at !== null ? [{ by, at, exchanges: strs(o.exchanges) }] : [];
    }),
    evidence:
      f.webRequest || f.webResponse
        ? {
            method: str(req.method),
            target: str(req.target),
            status: typeof res.statusCode === "number" ? res.statusCode : null,
            noResponse: res.noResponseReceived === true,
          }
        : null,
  };
}

export function asCandidates(v: unknown): Candidate[] {
  const list = obj(v).candidates;
  return Array.isArray(list) ? list.flatMap((x) => asCandidate(x) ?? []) : [];
}

/** One word for where a candidate stands. */
export type State = "to triage" | "confirmed" | "rejected" | "accepted risk" | "duplicate" | "fixed" | "suspected";

export function stateOf(c: Candidate): State {
  if (c.suppressed) {
    return c.suppressed.disposition === "falsePositive" ? "rejected" : c.suppressed.disposition === "duplicate" ? "duplicate" : "accepted risk";
  }
  if (c.status === "confirmed" || c.status === "fixed" || c.status === "suspected") return c.status;
  return "to triage";
}

/** "WSTG-INPV-05 · GET https://a.test/x · sort" */
export function where(c: Candidate): string {
  return [c.method && c.url ? `${c.method} ${c.url}` : (c.url ?? ""), c.parameter ? `parameter ${c.parameter}` : ""].filter(Boolean).join(" · ");
}

/** What the tester gives when flagging (POST /api/candidates). */
export interface FlagInput {
  exchange: string;
  rule: string;
  parameter?: string;
  level?: Level;
  rank?: number;
  message?: string;
  replays?: string[];
  cwe?: number;
  /** Flag it although it looks like an existing candidate (#78). */
  force?: boolean;
}

/** The request body: empty optional fields left out. */
export function flagBody(i: FlagInput): Record<string, unknown> {
  const b: Record<string, unknown> = { exchange: i.exchange, rule: i.rule };
  if (i.parameter?.trim()) b.parameter = i.parameter.trim();
  if (i.level) b.level = i.level;
  if (i.rank !== undefined && Number.isFinite(i.rank)) b.rank = i.rank;
  if (i.message?.trim()) b.message = i.message.trim();
  if (i.replays?.length) b.replays = i.replays;
  if (i.cwe) b.cwe = i.cwe;
  if (i.force) b.force = true;
  return b;
}

export type FlagResult =
  | { kind: "created"; candidate: Candidate }
  /** `similar`: the same rule on the same place in other words (#78); flag anyway with `force`. Otherwise the same fingerprint. */
  | { kind: "exists"; candidate: Candidate; message: string; similar: boolean }
  | { kind: "error"; message: string };

/** Flag an exchange. A matching fingerprint comes back as `exists`. */
export async function flag(i: FlagInput): Promise<FlagResult> {
  let res: Response;
  try {
    res = await fetch("/api/candidates", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(flagBody(i)) });
  } catch (e) {
    return { kind: "error", message: (e as Error).message };
  }
  const body: unknown = await res.json().catch(() => null);
  const o = obj(body);
  if (res.status === 201) {
    const c = asCandidate(body);
    return c ? { kind: "created", candidate: c } : { kind: "error", message: "unexpected answer from spall" };
  }
  if (res.status === 409) {
    const c = asCandidate(o.existing);
    if (c) return { kind: "exists", candidate: c, message: str(o.error) ?? "already flagged", similar: o.match === "similar" };
  }
  return { kind: "error", message: str(o.error) ?? `HTTP ${res.status}` };
}

/** A triage or edit step (PATCH /api/candidates/<id>). */
export type Op =
  | { op: "confirm" }
  | { op: "reject" | "accept"; justification: string }
  | { op: "duplicate"; of: string; justification: string }
  | { op: "reopen" }
  | { op: "edit"; level?: Level; rank?: number; message?: string }
  | { op: "evidence"; exchanges: string[] }
  | { op: "promote" }
  | { op: "unpromote" };

/** Can be promoted: confirmed (or suspected) and not suppressed. */
export function promotable(c: Candidate): boolean {
  return (c.status === "confirmed" || c.status === "suspected") && !c.suppressed;
}

/** One thing redaction changes: where, and what kind of secret. */
export interface RedactionChange {
  at: string;
  kind: string;
}

export interface RedactionPreview {
  changes: RedactionChange[];
  /** The redacted request target, if there is one. */
  target: string | null;
}

/** What an export would strip from candidate `id` (#49). */
export async function redactionPreview(id: string): Promise<RedactionPreview | string> {
  const body = await apiJson(`/api/candidates/${encodeURIComponent(id)}/redaction`);
  if (typeof body === "string") return body;
  const o = obj(body);
  const changes = Array.isArray(o.changes)
    ? o.changes.flatMap((x) => {
        const c = obj(x);
        return typeof c.at === "string" && typeof c.kind === "string" ? [{ at: c.at, kind: c.kind }] : [];
      })
    : [];
  return { changes, target: str(obj(obj(o.finding).webRequest).target) };
}

export type ExportResult = { kind: "file"; name: string; bytes: Uint8Array<ArrayBuffer> } | { kind: "error"; message: string };

/** `spall-acme-20261123.sarif` from a Content-Disposition header; a safe default otherwise. */
export function exportName(disposition: string | null): string {
  const m = /filename="([A-Za-z0-9._-]{1,120})"/.exec(disposition ?? "");
  return m ? m[1]! : "spall-export.sarif";
}

/** Export the promoted candidates as a flux report (#51). */
export async function exportFindings(unredacted: boolean): Promise<ExportResult> {
  let res: Response;
  try {
    res = await fetch("/api/export", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(unredacted ? { unredacted: true } : {}) });
  } catch (e) {
    return { kind: "error", message: (e as Error).message };
  }
  if (!res.ok) {
    const b = obj(await res.json().catch(() => null));
    return { kind: "error", message: str(b.error) ?? `HTTP ${res.status}` };
  }
  return { kind: "file", name: exportName(res.headers.get("content-disposition")), bytes: new Uint8Array(await res.arrayBuffer()) };
}

export async function change(id: string, op: Op): Promise<Candidate | string> {
  const body = await apiJson(`/api/candidates/${encodeURIComponent(id)}`, { method: "PATCH", body: op });
  if (typeof body === "string") return body;
  return asCandidate(body) ?? "unexpected answer from spall";
}

export async function listCandidates(exchange?: string): Promise<Candidate[] | string> {
  const body = await apiJson(exchange ? `/api/candidates?exchange=${encodeURIComponent(exchange)}` : "/api/candidates");
  return typeof body === "string" ? body : asCandidates(body);
}

/** Query parameter names in a URL, for the flag form's suggestions. */
export function paramNames(url: string): string[] {
  const q = url.indexOf("?");
  if (q < 0) return [];
  const names = url
    .slice(q + 1)
    .split("#")[0]!
    .split("&")
    .map((p) => {
      const k = p.split("=")[0] ?? "";
      try {
        return decodeURIComponent(k.replace(/\+/g, " "));
      } catch {
        return k;
      }
    })
    .filter(Boolean);
  return [...new Set(names)];
}

const FLAG_USAGE = "usage: flag <exchange-id> --rule <id> [--param P] [--level error|warning|note|none] [--rank 0-100] [--message M] [--replay <id>]... [--cwe N] [--anyway]";

/** `flag` console arguments. */
export function parseFlagArgs(args: string[]): FlagInput | string {
  const [exchange, ...rest] = args;
  if (!exchange || exchange.startsWith("--")) return FLAG_USAGE;
  const i: FlagInput = { exchange, rule: "" };
  for (let n = 0; n < rest.length; n += 2) {
    const k = rest[n]!;
    if (k === "--anyway") {
      i.force = true;
      n -= 1;
      continue;
    }
    const v = rest[n + 1];
    if (v === undefined) return `${k}: needs a value. ${FLAG_USAGE}`;
    if (k === "--rule") i.rule = v;
    else if (k === "--param") i.parameter = v;
    else if (k === "--level") {
      const l = oneOf(v, LEVELS);
      if (!l) return "--level: error, warning, note or none";
      i.level = l;
    } else if (k === "--rank") {
      const r = Number(v);
      if (!Number.isFinite(r) || r < 0 || r > 100) return "--rank: 0-100";
      i.rank = r;
    } else if (k === "--message") i.message = v;
    else if (k === "--replay") i.replays = [...(i.replays ?? []), v];
    else if (k === "--cwe") {
      const c = Number(v.replace(/^cwe-/i, ""));
      if (!Number.isInteger(c) || c <= 0) return "--cwe: a CWE number";
      i.cwe = c;
    } else return `unknown option ${k}. ${FLAG_USAGE}`;
  }
  if (!i.rule) return `--rule is required (see rules). ${FLAG_USAGE}`;
  return i;
}
