// Tasks waiting on a human (#56): what /api/tasks says about them, what
// each decision would do, and the call that makes it. Every string here
// can carry captured traffic (a target URL, a Host header) or an agent's
// words (a goal, a reason), so the UI renders them as text only.
import { apiJson } from "./engagement.ts";

export type Ask =
  | { kind: "host"; host: string }
  | { kind: "method"; method: string }
  | { kind: "unsafe"; method: string }
  | { kind: "confirm" }
  | { kind: "time" }
  | { kind: "budget" };

export interface Waiting {
  id: string;
  goal: string;
  role: string;
  spent: number;
  budget: number;
  timeLimit: number;
  pending: { method: string; target: string; hostHeader: string | null; reason: string; asked: string; ask: Ask };
}

const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);

function asAsk(v: unknown): Ask | null {
  const o = (v ?? {}) as Record<string, unknown>;
  switch (o.kind) {
    case "host":
      return { kind: "host", host: str(o.host) };
    case "method":
    case "unsafe":
      return { kind: o.kind, method: str(o.method) };
    case "confirm":
    case "time":
    case "budget":
      return { kind: o.kind };
    default:
      return null;
  }
}

/** The tasks in `/api/tasks` that wait on approval, oldest ask first. */
export function asWaiting(v: unknown): Waiting[] {
  const list = (v as { tasks?: unknown } | null)?.tasks;
  if (!Array.isArray(list)) return [];
  return list
    .flatMap((x): Waiting[] => {
      const t = (x ?? {}) as Record<string, unknown>;
      const p = (t.pending ?? null) as Record<string, unknown> | null;
      const c = (t.constraints ?? {}) as Record<string, unknown>;
      const ask = p ? asAsk(p.ask) : null;
      if (t.status !== "waiting_on_approval" || !p || !ask) return [];
      return [
        {
          id: str(t.id),
          goal: str(t.goal),
          role: str(t.role),
          spent: num(t.spent),
          budget: num(c.budget),
          timeLimit: num(c.time_limit_minutes),
          pending: {
            method: str(p.method),
            target: str(p.target),
            hostHeader: typeof p.host_header === "string" ? p.host_header : null,
            reason: str(p.reason),
            asked: str(p.asked),
            ask,
          },
        },
      ];
    })
    .sort((a, b) => a.pending.asked.localeCompare(b.pending.asked));
}

/** What "approve for this task" would change, or null when only "once" can (an RoE confirmation). */
export function forTaskText(w: Waiting): string | null {
  const a = w.pending.ask;
  switch (a.kind) {
    case "host":
      return `add ${a.host} to ${w.id}'s hosts`;
    case "method":
      return `let ${w.id} send ${a.method}`;
    case "unsafe":
      return `let ${w.id} send ${a.method} without asking`;
    case "time":
      return `time limit ${w.timeLimit} → ${Math.min(w.timeLimit * 2, 1440)} min`;
    case "budget":
      return `budget ${w.budget} → ${Math.min(w.budget * 2, 100000)} requests`;
    case "confirm":
      return null;
  }
}

/** What approving costs, in the task's own budget. */
export function costText(w: Waiting): string {
  return `1 request · ${w.spent} of ${w.budget} spent`;
}

export async function listWaiting(): Promise<Waiting[] | string> {
  const body = await apiJson("/api/tasks");
  return typeof body === "string" ? body : asWaiting(body);
}

export type Decision = { op: "approve_once" } | { op: "approve_for_task" } | { op: "deny"; reason: string };

/** Decide; null when done, else the server's message. */
export async function decide(id: string, d: Decision): Promise<string | null> {
  const body = await apiJson(`/api/tasks/${encodeURIComponent(id)}`, { method: "PATCH", body: d });
  return typeof body === "string" ? body : null;
}

/** One line per task, newest first, for the console's `task`. */
export function taskListLines(v: unknown): string[] {
  const list = (v as { tasks?: unknown } | null)?.tasks;
  if (!Array.isArray(list) || list.length === 0) return ["no tasks in this engagement"];
  return list.map((x) => {
    const t = (x ?? {}) as Record<string, unknown>;
    const c = (t.constraints ?? {}) as Record<string, unknown>;
    return `${str(t.id).padEnd(6)} ${str(t.status).padEnd(19)} ${str(t.role).padEnd(8)} ${num(t.spent)}/${num(c.budget)}  ${str(t.goal)}`;
  });
}

/** A task's details, for the console's `task <id>`. */
export function taskLines(v: unknown): string[] {
  const t = (v ?? {}) as Record<string, unknown>;
  const c = (t.constraints ?? {}) as Record<string, unknown>;
  const list = (k: string, empty: string) => {
    const a = c[k];
    return Array.isArray(a) && a.length ? a.filter((x) => typeof x === "string").join(", ") : empty;
  };
  const out = [
    `${str(t.id)} · ${str(t.status)} · ${str(t.role)} · asked by ${str(t.asked_by)}`,
    `goal: ${str(t.goal)}`,
    `spent ${num(t.spent)} of ${num(c.budget)} requests · time limit ${num(c.time_limit_minutes)} min`,
    `hosts: ${list("hosts", "the engagement's scope")} · methods: ${list("methods", "any the RoE allows")} · without asking: ${list("safe_methods", "none")}`,
  ];
  if (str(t.reason)) out.push(`why: ${str(t.reason)}`);
  if (str(t.note)) out.push(`note: ${str(t.note)}`);
  const p = (t.pending ?? null) as Record<string, unknown> | null;
  if (p) out.push(`waiting on approval: ${str(p.method)} ${str(p.target)} (${str(p.reason)})`);
  const log = Array.isArray(t.log) ? t.log : [];
  out.push(`log: ${log.length} entr${log.length === 1 ? "y" : "ies"}`);
  return out;
}
