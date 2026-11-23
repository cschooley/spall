// Attribution everywhere (#62, docs/ux.md): who made an exchange or a
// candidate. It comes from the record (taphttp's `meta` on a replay,
// spall's `flagged_by` on a candidate), never from memory, so it survives
// a restart. Agent items carry ◆ and a label: never color alone.

export interface Attribution {
  agent: boolean;
  /** The task an agent acted under, like T-3. */
  task: string | null;
  /** `you`, or `agent · T-3`. */
  label: string;
}

const TASK = /^T-\d{1,9}$/;

const agent = (task: string | null): Attribution => ({ agent: true, task, label: task ? `agent · ${task}` : "agent" });
const YOU: Attribution = { agent: false, task: null, label: "you" };

/** An exchange's attribution from its `meta` (`{"actor": "agent", "task": "T-3"}`). */
export function ofExchange(meta: Record<string, string> | null | undefined): Attribution {
  if (meta?.actor !== "agent") return YOU;
  return agent(typeof meta.task === "string" && TASK.test(meta.task) ? meta.task : null);
}

/** A candidate's attribution from `flagged_by` (`human`, or `agent:T-3`). */
export function ofCandidate(flaggedBy: string): Attribution {
  if (!flaggedBy.startsWith("agent")) return YOU;
  const task = flaggedBy.slice("agent:".length);
  return agent(TASK.test(task) ? task : null);
}

/** Where a task opens. */
export function taskHref(task: string): string {
  return `#tasks/${encodeURIComponent(task)}`;
}

/** One audit record as an activity line (#62): when, who, what. Text only. */
export interface ActivityLine {
  time: string;
  who: Attribution;
  what: string;
}

export function activityOf(r: Record<string, unknown>): ActivityLine {
  const s = (k: string) => (typeof r[k] === "string" ? (r[k] as string) : "");
  const time = s("ts").match(/T(\d\d:\d\d:\d\d)/)?.[1] ?? "--:--:--";
  const who = s("actor") === "agent" ? ofExchange({ actor: "agent", task: s("task") }) : YOU;
  const outcome = s("outcome").replace(/_/g, " ");
  const target = [s("method"), s("target")].filter(Boolean).join(" ");
  const why = s("reason") || s("error");
  const what = [`${s("action")} ${outcome}`.trim(), target, why].filter(Boolean).join(" · ");
  return { time, who, what };
}
