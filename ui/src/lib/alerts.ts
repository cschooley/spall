// Tester alerts (#86): something needs you, or something you started
// finished. The server sends kinds and ids only; the words are built here
// from those, so a desktop notification can't carry an engagement's hosts,
// paths or findings (they'd show on a lock screen or a shared screen).
import { apiJson } from "./engagement.ts";

export type AlertKind = "waiting" | "done" | "failed" | "halted" | "oob_hit" | "quad_over";

export interface Alert {
  seq: number;
  at: string;
  kind: AlertKind;
  task: string | null;
  item: string | null;
}

const KINDS: readonly AlertKind[] = ["waiting", "done", "failed", "halted", "oob_hit", "quad_over"];
const ID = /^[TI]-\d{1,10}$/;

export function asAlerts(v: unknown): { alerts: Alert[]; latest: number } | string {
  if (typeof v === "string") return v;
  const o = (v ?? {}) as Record<string, unknown>;
  const list = Array.isArray(o.alerts) ? o.alerts : [];
  return {
    latest: typeof o.latest === "number" ? o.latest : 0,
    alerts: list.flatMap((x) => {
      const a = (x ?? {}) as Record<string, unknown>;
      const kind = KINDS.find((k) => k === a.kind);
      if (!kind || typeof a.seq !== "number") return [];
      const id = (v: unknown) => (typeof v === "string" && ID.test(v) ? v : null);
      return [{ seq: a.seq, at: typeof a.at === "string" ? a.at : "", kind, task: id(a.task), item: id(a.item) }];
    }),
  };
}

/** The alert in words: kinds and ids only. */
export function alertText(a: Alert): string {
  const t = a.task ?? "a task";
  switch (a.kind) {
    case "waiting":
      return `${t} is waiting on your approval`;
    case "done":
      return `${t} is done`;
    case "failed":
      return `${t} failed: open it for the reason`;
    case "halted":
      return `${t} was halted`;
    case "oob_hit":
      return `Out-of-band callback ${a.item ?? ""}${a.task ? ` (${a.task})` : ""}`.replace(/ +/g, " ").trim();
    case "quad_over":
      return "Quad is over: the scorecard is up";
  }
}

/** Where the alert opens: the task, or the Engagement view. */
export function alertHash(a: Alert): string {
  if (a.kind === "oob_hit" || a.kind === "quad_over" || !a.task) return "engagement";
  return `tasks/${a.task}`;
}

export async function alertsAfter(seq: number): Promise<{ alerts: Alert[]; latest: number } | string> {
  return asAlerts(await apiJson(`/api/alerts?after=${seq}`));
}
