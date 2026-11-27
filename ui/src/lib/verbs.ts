// Verbs on objects (#67, docs/ux.md "Verbs on objects"): buttons that draft
// a pre-filled agent task from the exchange or candidate they sit on. Each is
// a registry command (src/commands.rs, src/verbs.rs) and so also a palette
// entry, a console command and an MCP tool. A verb only drafts: the task
// runs when a human approves it in Tasks. The server says the exact cost;
// the hint here is shown on the button, before anything is drafted.

import { apiJson } from "./engagement.ts";

export type VerbOn = "exchange" | "candidate";

export interface Verb {
  name: string;
  label: string;
  on: VerbOn;
  role: string;
  /** The cost, shown first. `n` is a candidate's evidence count. */
  hint(n: number): string;
}

/** Cheapest risk first, as src/verbs.rs VERBS (a test holds the two together). */
export const VERBS: readonly Verb[] = [
  { name: "why", label: "Why is this interesting?", on: "exchange", role: "reporter", hint: () => "no requests" },
  { name: "explain", label: "Explain for devs", on: "candidate", role: "reporter", hint: () => "no requests" },
  { name: "map_auth", label: "Map auth", on: "exchange", role: "mapper", hint: () => "~40 requests, safe methods" },
  { name: "test_for", label: "Test for…", on: "exchange", role: "tester", hint: () => "~100 requests; unsafe ones asked for" },
  { name: "retest", label: "Retest", on: "candidate", role: "tester", hint: (n) => `~${Math.min(50, Math.max(5, 3 * n))} requests, the evidence's methods` },
  { name: "variants", label: "Find variants", on: "candidate", role: "tester", hint: () => "~60 requests; unsafe ones asked for" },
  { name: "escalate", label: "Escalate impact", on: "candidate", role: "tester", hint: () => "~30 requests, every one asked for" },
  { name: "poc", label: "Build a PoC", on: "candidate", role: "tester", hint: () => "~30 requests, every one asked for; a sandbox proof" },
];

/** `test_for`'s WSTG categories (src/verbs.rs CATEGORIES). */
export const CATEGORIES: readonly [string, string][] = [
  ["ATHZ", "authorization (IDOR, missing checks)"],
  ["INPV", "injection and input handling"],
  ["ATHN", "authentication"],
  ["SESS", "session management"],
  ["BUSL", "business logic"],
  ["CONF", "configuration and deployment"],
];

export function verbsOn(on: VerbOn): readonly Verb[] {
  return VERBS.filter((v) => v.on === on);
}

export interface Drafted {
  id: string;
  role: string;
  cost: string;
}

/** What POST /api/verbs/{name} answered: the draft, or why not. */
export function asDrafted(v: unknown): Drafted | string {
  const o = v as { task?: { id?: unknown; role?: unknown }; cost?: unknown } | null;
  if (typeof v === "string") return v;
  if (!o || typeof o.task?.id !== "string" || typeof o.cost !== "string") return "unexpected answer from spall";
  return { id: o.task.id, role: typeof o.task.role === "string" ? o.task.role : "", cost: o.cost };
}

/** Draft verb `name`'s task for object `id`. */
export async function runVerb(name: string, id: string, category?: string): Promise<Drafted | string> {
  return asDrafted(await apiJson(`/api/verbs/${encodeURIComponent(name)}`, { method: "POST", body: category ? { id, category } : { id } }));
}

/** The console's line for a draft. */
export function draftedLine(name: string, d: Drafted): string {
  return `${name}: drafted ${d.id} (${d.role}, ${d.cost}); approve it in Tasks (4) to run it`;
}
