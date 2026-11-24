// The Findings view's queue (#44): which candidates show, in what order,
// and where the selection goes after a decision. Pure: no React, no fetch.

import { stateOf, type Candidate, type State } from "./candidates.ts";

export type Tab = "to triage" | "confirmed" | "promoted" | "decided" | "all";
export const TABS: readonly Tab[] = ["to triage", "confirmed", "promoted", "decided", "all"];

const DECIDED: readonly State[] = ["rejected", "accepted risk", "duplicate"];

export function inTab(c: Candidate, tab: Tab): boolean {
  const s = stateOf(c);
  if (tab === "all") return true;
  if (tab === "promoted") return c.promoted !== null && !c.suppressed;
  if (tab === "decided") return DECIDED.includes(s);
  if (tab === "confirmed") return s === "confirmed" || s === "fixed";
  return s === "to triage" || s === "suspected";
}

export function tabCounts(list: Candidate[]): Record<Tab, number> {
  return {
    "to triage": list.filter((c) => inTab(c, "to triage")).length,
    confirmed: list.filter((c) => inTab(c, "confirmed")).length,
    promoted: list.filter((c) => inTab(c, "promoted")).length,
    decided: list.filter((c) => inTab(c, "decided")).length,
    all: list.length,
  };
}

/** Host of a URL, lowercased, without port; "" if there isn't one. */
export function hostOf(url: string | null): string {
  if (!url) return "";
  const m = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/?#]*@)?(\[[^\]]*\]|[^:/?#]*)/i.exec(url);
  return (m?.[1] ?? "").toLowerCase();
}

/**
 * The queue's filter: `rule:` (prefix of the rule id), `host:` (suffix),
 * `state:` (to-triage, confirmed, rejected, accepted, duplicate),
 * `level:`, `cwe:`, and free text over id, rule, message, URL and
 * parameter. Every term must match; any case.
 */
export function matchesFindingQuery(c: Candidate, query: string): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const state = stateOf(c).replace(/ /g, "-");
  const host = hostOf(c.url);
  const text = [c.id, c.ruleId, c.message, c.url ?? "", c.parameter ?? ""].join(" ").toLowerCase();
  return terms.every((t) => {
    const i = t.indexOf(":");
    const [k, v] = i > 0 ? [t.slice(0, i), t.slice(i + 1)] : ["", t];
    switch (k) {
      case "rule":
        return c.ruleId.toLowerCase().startsWith(v) || c.ruleId.toLowerCase().includes(v);
      case "host":
        return host === v || host.endsWith(`.${v}`) || host.includes(v);
      case "state":
        return state.startsWith(v);
      case "level":
        return (c.level ?? "") === v;
      case "cwe":
        return c.cwes.includes(Number(v.replace(/^cwe-/, "")));
      default:
        return text.includes(t);
    }
  });
}

/** The queue as shown: in the tab, matching the query, newest first (as listed). */
export function queue(list: Candidate[], tab: Tab, query: string): Candidate[] {
  return list.filter((c) => inTab(c, tab) && matchesFindingQuery(c, query));
}

/**
 * Where the selection goes when `id` leaves the queue: the next one down,
 * else the one above, else nothing. `before` is the queue it was in.
 */
export function nextAfter(before: Candidate[], after: Candidate[], id: string): string | null {
  const at = before.findIndex((c) => c.id === id);
  if (after.some((c) => c.id === id)) return id;
  const still = new Set(after.map((c) => c.id));
  for (let i = at + 1; i < before.length; i++) if (still.has(before[i]!.id)) return before[i]!.id;
  for (let i = at - 1; i >= 0; i--) if (still.has(before[i]!.id)) return before[i]!.id;
  return after[0]?.id ?? null;
}

/** `#findings/c-1` → "c-1"; null for anything else. */
export function candidateFromHash(hash: string): string | null {
  const m = /^#?findings\/([A-Za-z0-9_-]{1,64})$/.exec(hash);
  return m ? m[1]! : null;
}

/** Candidates another can be marked a duplicate of: not itself, not decided. */
export function duplicateTargets(list: Candidate[], id: string): Candidate[] {
  return list.filter((c) => c.id !== id && !c.suppressed).sort((a, b) => Number(b.status === "confirmed") - Number(a.status === "confirmed"));
}

/** The candidate `c` duplicates: by `flux/duplicateOf` (a correlationGuid) once decided, else the one it was flagged anyway against (#78). */
export function originalOf(c: Candidate, all: Candidate[]): Candidate | null {
  const guid = c.suppressed?.disposition === "duplicate" ? c.suppressed.duplicateOf : null;
  const o = guid ? all.find((x) => x.guid === guid) : c.possibleDuplicateOf ? all.find((x) => x.id === c.possibleDuplicateOf) : undefined;
  return o && o.id !== c.id ? o : null;
}

/** A queue row: the candidate, and its original if it's a duplicate (#78). */
export interface Grouped {
  c: Candidate;
  original: Candidate | null;
  /** Listed right under its original (which is in the queue too). */
  nested: boolean;
}

/**
 * The queue with each duplicate under its original when both are in it
 * (#78); otherwise in place, still naming its original. Order otherwise
 * kept, so j/k walk it as shown.
 */
export function groupDuplicates(shown: Candidate[], all: Candidate[]): Grouped[] {
  const ids = new Set(shown.map((c) => c.id));
  const orig = new Map(shown.map((c) => [c.id, originalOf(c, all)] as const));
  const nests = (c: Candidate) => {
    const o = orig.get(c.id);
    return !!o && ids.has(o.id) && !orig.get(o.id);
  };
  const out: Grouped[] = [];
  for (const c of shown) {
    if (nests(c)) continue;
    out.push({ c, original: orig.get(c.id) ?? null, nested: false });
    for (const d of shown) if (nests(d) && orig.get(d.id)?.id === c.id) out.push({ c: d, original: c, nested: true });
  }
  return out;
}
