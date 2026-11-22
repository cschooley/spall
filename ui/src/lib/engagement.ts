// The engagement (#35) as the UI sees it: shape checks for spall's
// /api/engagement answers, and the one fetch helper the console uses.
// Engagement details are tester-entered, but still rendered as text only.

import type { EngagementInfo, EngagementSummary } from "../commands/registry.ts";
import { asRules } from "./roe.ts";

const str = (v: unknown) => (typeof v === "string" ? v : "");
const optStr = (v: unknown) => (typeof v === "string" && v !== "" ? v : null);
const strings = (v: unknown) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);

/** An engagement from the API, or null if it isn't one. */
export function asEngagement(v: unknown): EngagementInfo | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (typeof o.id !== "string" || typeof o.name !== "string") return null;
  return {
    id: o.id,
    name: o.name,
    client: str(o.client),
    starts: optStr(o.starts),
    ends: optStr(o.ends),
    expires: optStr(o.expires),
    scope: strings(o.scope),
    notes: str(o.notes),
    rules: asRules(o.rules),
    testingNow: testingOf(o.testing_now),
  };
}

/** `{allowed, reason}` from the API, or null. */
export function testingOf(v: unknown): { allowed: boolean; reason: string | null } | null {
  if (!v || typeof v !== "object") return null;
  const o = v as { allowed?: unknown; reason?: unknown };
  return typeof o.allowed === "boolean" ? { allowed: o.allowed, reason: typeof o.reason === "string" ? o.reason : null } : null;
}

export function asSummaries(v: unknown): EngagementSummary[] | null {
  if (!Array.isArray(v)) return null;
  return v.flatMap((x) => {
    const e = asEngagement(x);
    return e ? [{ id: e.id, name: e.name, client: e.client, active: (x as { active?: unknown }).active === true }] : [];
  });
}

/** Fetch JSON from spall; a string is an error message to show. */
export async function apiJson(path: string, init?: { method: string; body: unknown }): Promise<unknown> {
  try {
    const res = await fetch(
      path,
      init ? { method: init.method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(init.body) } : undefined,
    );
    const body: unknown = await res.json().catch(() => null);
    if (!res.ok) {
      const msg = (body as { error?: unknown } | null)?.error;
      return typeof msg === "string" ? msg : `HTTP ${res.status}`;
    }
    return body;
  } catch {
    return "spall unreachable";
  }
}
