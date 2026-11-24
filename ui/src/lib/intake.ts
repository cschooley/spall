// The intake (#65): plain language in, a draft plan out. The plan is the
// orchestrator's proposal: role tasks with costs, warnings, a pick list
// when it's ambiguous, and drafts only a human can act on (an extension, a
// new engagement). Nothing runs until the tester approves the plan. The
// request, warnings and goals can carry captured text: shown as text only.
import { apiJson } from "./engagement.ts";

export interface PlannedTask {
  role: string;
  goal: string;
  cost: string;
  candidate: string | null;
}

export type Proposal =
  | { kind: "extension"; engagement: string; why: string }
  | { kind: "engagement"; name: string; scope: string[]; why: string };

export interface Plan {
  id: string;
  engagement: string;
  request: string;
  status: "draft" | "approved" | "discarded";
  summary: string;
  tasks: PlannedTask[];
  warnings: string[];
  choice: { question: string; options: string[] } | null;
  proposal: Proposal | null;
  estimate: string;
  made: string[];
}

export type Answer = { kind: "no" } | { kind: "said"; text: string } | { kind: "plan"; plan: Plan };

const str = (v: unknown) => (typeof v === "string" ? v : "");
const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

function asProposal(v: unknown): Proposal | null {
  const o = obj(v);
  if (o.kind === "extension") return { kind: "extension", engagement: str(o.engagement), why: str(o.why) };
  if (o.kind === "engagement") return { kind: "engagement", name: str(o.name), scope: strs(o.scope), why: str(o.why) };
  return null;
}

export function asPlan(v: unknown): Plan | null {
  const o = obj(v);
  if (typeof o.id !== "string" || typeof o.engagement !== "string") return null;
  const status = o.status === "approved" || o.status === "discarded" ? o.status : "draft";
  const c = obj(o.choice);
  return {
    id: o.id,
    engagement: o.engagement,
    request: str(o.request),
    status,
    summary: str(o.summary),
    tasks: (Array.isArray(o.tasks) ? o.tasks : []).map((t) => {
      const p = obj(t);
      const task = obj(p.task);
      return { role: str(task.role), goal: str(task.goal), cost: str(p.cost), candidate: typeof p.candidate === "string" ? p.candidate : null };
    }),
    warnings: strs(o.warnings),
    choice: typeof c.question === "string" ? { question: c.question, options: strs(c.options) } : null,
    proposal: asProposal(o.proposal),
    estimate: str(o.estimate),
    made: strs(o.made),
  };
}

export function asAnswer(v: unknown): Answer | string {
  if (typeof v === "string") return v;
  const o = obj(v);
  if (o.kind === "no") return { kind: "no" };
  if (o.kind === "said") return { kind: "said", text: str(o.text) };
  const plan = asPlan(o.plan);
  return o.kind === "plan" && plan ? { kind: "plan", plan } : "unexpected answer from spall";
}

export async function askIntake(text: string, pick?: { question: string; answer: string }): Promise<Answer | string> {
  return asAnswer(await apiJson("/api/intake", { method: "POST", body: pick ? { text, pick } : { text } }));
}

export type PlanOp = { op: "drop"; index: number } | { op: "discard" } | { op: "approve" };

export async function changePlan(id: string, op: PlanOp): Promise<Plan | string> {
  const v = await apiJson(`/api/plans/${encodeURIComponent(id)}`, { method: "PATCH", body: op });
  if (typeof v === "string") return v;
  return asPlan(v) ?? "unexpected answer from spall";
}

export async function draftPlans(): Promise<Plan[] | string> {
  const v = await apiJson("/api/plans");
  if (typeof v === "string") return v;
  const list = obj(v).plans;
  return (Array.isArray(list) ? list : []).flatMap((p) => {
    const plan = asPlan(p);
    return plan && plan.status === "draft" ? [plan] : [];
  });
}
