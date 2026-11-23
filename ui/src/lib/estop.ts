// The agent e-stop (#57): what /api/status says about it, and the two
// calls. Engaging halts every agent task for good; clearing only lets new
// tasks run. Text from the server is shown as text.
import { apiJson } from "./engagement.ts";

export interface EStop {
  since: string;
  by: string;
  reason: string;
}

/** `/api/status`'s `estop`: null when clear (or unreadable: the server is the authority). */
export function asEStop(v: unknown): EStop | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const s = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : "");
  return { since: s("since"), by: s("by"), reason: s("reason") };
}

/** Engage it; what happened, as a line for the console. */
export async function engageEStop(reason = ""): Promise<{ ok: boolean; text: string }> {
  const body = await apiJson("/api/estop", { method: "POST", body: reason ? { reason } : {} });
  if (typeof body === "string") return { ok: false, text: `e-stop: ${body}` };
  const halted = Array.isArray((body as { halted?: unknown }).halted) ? (body as { halted: unknown[] }).halted.length : 0;
  return { ok: true, text: `e-stop engaged: ${halted} agent task${halted === 1 ? "" : "s"} halted for good. Human traffic is unaffected.` };
}

/** Clear it: new tasks may run; halted ones never do. */
export async function clearEStop(): Promise<{ ok: boolean; text: string }> {
  const body = await apiJson("/api/estop/clear", { method: "POST", body: {} });
  if (typeof body === "string") return { ok: false, text: `e-stop: ${body}` };
  return { ok: true, text: "e-stop cleared: new tasks may be approved. Halted tasks stay halted." };
}

/** When it was engaged, as `HH:MM` local time, or the raw value. */
export function sinceText(since: string): string {
  const d = new Date(since);
  if (Number.isNaN(d.getTime())) return since;
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
