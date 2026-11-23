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
