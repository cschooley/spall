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
  if (t.runner === "spall") out.push(`run by spall · ${usageText(asUsage(t.usage, c.tokens))}`);
  if (str(t.reason)) out.push(`why: ${str(t.reason)}`);
  if (str(t.note)) out.push(`note: ${str(t.note)}`);
  const p = (t.pending ?? null) as Record<string, unknown> | null;
  if (p) out.push(`waiting on approval: ${str(p.method)} ${str(p.target)} (${str(p.reason)})`);
  const log = Array.isArray(t.log) ? t.log : [];
  out.push(`log: ${log.length} entr${log.length === 1 ? "y" : "ies"}`);
  return out;
}

/** A task in full, as `GET /api/tasks/{id}` (or a list row, without its log). */
export interface Task {
  id: string;
  goal: string;
  role: string;
  status: string;
  reason: string | null;
  spent: number;
  budget: number;
  timeLimit: number;
  hosts: string[];
  methods: string[];
  safeMethods: string[];
  rate: number | null;
  stop: { serverErrors: number | null; throttled: number | null };
  plan: { text: string; done: boolean }[];
  askedBy: string;
  note: string;
  created: string;
  started: string | null;
  ended: string | null;
  log: Record<string, unknown>[];
  /** Log entries the server has (lists leave the log out). */
  logLen: number;
  /** The request it waits on, in the server's shape (for asWaiting). */
  pendingRaw: unknown;
  /** Who runs its agent loop: an outside MCP client, or spall itself (#63). */
  runner: "external" | "spall";
  /** What its model has used, against its token cap; spall-run tasks only. */
  usage: Usage;
  /** The agent's report on its work (#81): model-written, so text only. */
  report: { text: string; at: string } | null;
}

export interface Usage {
  calls: number;
  inputTokens: number;
  outputTokens: number;
  modelMs: number;
  /** The task's model token cap (in and out). */
  cap: number;
}

/** The server's default token cap, for a task that doesn't say. */
const DEFAULT_TOKENS = 2_000_000;

/** 950, 9.1k, 182k, 2.0M. */
function compact(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`;
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

function duration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/** A task's model use in one line. */
export function usageText(u: Usage): string {
  if (u.calls === 0) return "no model calls yet";
  const tokens = u.inputTokens + u.outputTokens;
  return `${u.calls} model call${u.calls === 1 ? "" : "s"} · ${compact(tokens)} of ${compact(u.cap)} tokens (${compact(u.inputTokens)} in, ${compact(u.outputTokens)} out) · ${duration(u.modelMs)}`;
}

function asUsage(v: unknown, cap: unknown): Usage {
  const u = (v ?? {}) as Record<string, unknown>;
  return {
    calls: num(u.calls),
    inputTokens: num(u.input_tokens),
    outputTokens: num(u.output_tokens),
    modelMs: num(u.model_ms),
    cap: typeof cap === "number" && Number.isFinite(cap) ? cap : DEFAULT_TOKENS,
  };
}

const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const opt = (v: unknown) => (typeof v === "string" ? v : null);
const optNum = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);

export function asTask(v: unknown): Task | null {
  const t = (v ?? null) as Record<string, unknown> | null;
  if (!t || typeof t.id !== "string") return null;
  const c = (t.constraints ?? {}) as Record<string, unknown>;
  const stop = (c.stop ?? {}) as Record<string, unknown>;
  const log = Array.isArray(t.log) ? (t.log.filter((e) => e && typeof e === "object") as Record<string, unknown>[]) : [];
  return {
    id: t.id,
    goal: str(t.goal),
    role: str(t.role),
    status: str(t.status),
    reason: opt(t.reason),
    spent: num(t.spent),
    budget: num(c.budget),
    timeLimit: num(c.time_limit_minutes),
    hosts: strs(c.hosts),
    methods: strs(c.methods),
    safeMethods: strs(c.safe_methods),
    rate: optNum(c.rate_limit),
    stop: { serverErrors: optNum(stop.server_errors), throttled: optNum(stop.throttled) },
    plan: Array.isArray(t.plan) ? t.plan.map((p) => ({ text: str((p as Record<string, unknown>)?.text), done: (p as Record<string, unknown>)?.done === true })) : [],
    askedBy: str(t.asked_by),
    note: str(t.note),
    created: str(t.created),
    started: opt(t.started),
    ended: opt(t.ended),
    log,
    logLen: typeof t.log_len === "number" ? t.log_len : log.length,
    pendingRaw: t.pending ?? null,
    runner: t.runner === "spall" ? "spall" : "external",
    usage: asUsage(t.usage, c.tokens),
    report: asReport(t.report),
  };
}

function asReport(v: unknown): Task["report"] {
  const r = (v ?? null) as Record<string, unknown> | null;
  if (!r || typeof r !== "object" || typeof r.text !== "string") return null;
  return { text: r.text, at: str(r.at) };
}

export function asTasks(v: unknown): Task[] {
  const list = (v as { tasks?: unknown } | null)?.tasks;
  return Array.isArray(list) ? list.flatMap((x) => asTask(x) ?? []) : [];
}

/** The status as a chip: label and class. */
export function chipOf(status: string): { label: string; cls: string } {
  switch (status) {
    case "running":
      return { label: "Running", cls: "running" };
    case "waiting_on_approval":
      return { label: "Needs approval", cls: "waiting" };
    case "halted":
      return { label: "Halted", cls: "halted" };
    case "draft":
      return { label: "Draft", cls: "draft" };
    case "approved":
      return { label: "Approved", cls: "approved" };
    case "done":
      return { label: "Done", cls: "done" };
    case "failed":
      return { label: "Failed", cls: "halted" };
    default:
      return { label: status || "?", cls: "" };
  }
}

export const finished = (status: string) => status === "done" || status === "halted" || status === "failed";

const timeOf = (ts: string) => ts.match(/T(\d\d:\d\d:\d\d)/)?.[1] ?? "--:--:--";

/** The request log: every request the task sent or tried, oldest first. */
export interface RequestRow {
  time: string;
  method: string;
  target: string;
  status: number | null;
  outcome: string;
  exchange: string | null;
  check: string | null;
}

export function requestRows(t: Task): RequestRow[] {
  return t.log
    .filter((e) => e.kind === "request")
    .map((e) => ({
      time: timeOf(str(e.ts)),
      method: str(e.method),
      target: str(e.target),
      status: optNum(e.status),
      outcome: str(e.outcome),
      exchange: opt(e.exchange),
      check: opt(e.check),
    }));
}

/** The steering panel: your notes, the agent's turns, and what happened, oldest first. */
export interface Message {
  time: string;
  who: "you" | "agent" | "sys";
  text: string;
}

export function messages(t: Task): Message[] {
  const out: Message[] = [];
  for (const e of t.log) {
    const time = timeOf(str(e.ts));
    switch (e.kind) {
      case "note":
        out.push({ time, who: str(e.by) === "human" ? "you" : "agent", text: str(e.text) });
        break;
      case "turn":
        out.push({ time, who: "agent", text: str(e.text) });
        break;
      case "status":
        out.push({ time, who: "sys", text: `${str(e.from)} → ${str(e.to)}${str(e.reason) ? `: ${str(e.reason)}` : ""}` });
        break;
      case "approval":
        out.push({ time, who: "sys", text: `${str(e.outcome)}: ${str(e.what)}` });
        break;
    }
  }
  return out;
}

export interface NewTask {
  goal: string;
  role: string;
  budget?: number;
  timeLimit?: number;
  hosts?: string[];
  methods?: string[];
  note?: string;
  /** Run by spall's own agent host; an outside MCP client when left out. */
  runner?: "spall";
  /** Model token cap, for a spall-run task. */
  tokens?: number;
}

/** POST /api/tasks: the new draft, or the server's message. */
export async function createTask(n: NewTask): Promise<Task | string> {
  const constraints: Record<string, unknown> = {};
  if (n.budget !== undefined) constraints.budget = n.budget;
  if (n.timeLimit !== undefined) constraints.time_limit_minutes = n.timeLimit;
  if (n.hosts?.length) constraints.hosts = n.hosts;
  if (n.methods?.length) constraints.methods = n.methods;
  if (n.tokens !== undefined) constraints.tokens = n.tokens;
  const ask: Record<string, unknown> = { goal: n.goal, role: n.role };
  if (n.runner) ask.runner = n.runner;
  const body = await apiJson("/api/tasks", { method: "POST", body: { ...ask, constraints, note: n.note ?? "" } });
  if (typeof body === "string") return body;
  return asTask(body) ?? "unexpected answer from spall";
}

/** PATCH a task (approve, halt, note); null when done, else the server's message. */
export async function changeTask(id: string, op: Record<string, unknown>): Promise<string | null> {
  const body = await apiJson(`/api/tasks/${encodeURIComponent(id)}`, { method: "PATCH", body: op });
  return typeof body === "string" ? body : null;
}

/** A task id from `#tasks/T-3`. */
export function taskFromHash(hash: string): string | null {
  return /^#?tasks\/(T-\d{1,9})$/.exec(hash)?.[1] ?? null;
}
