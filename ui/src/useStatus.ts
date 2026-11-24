import { useEffect, useState } from "react";
import { testingOf } from "./lib/engagement.ts";
import { asEStop, type EStop } from "./lib/estop.ts";

export interface SourceStatus {
  /** pending: not asked yet; unknown: /api/status failed. */
  source: "pending" | "lever" | "log" | "unknown";
  /** Following taphttp's feed (lever); null when tailing a log file. */
  connected: boolean | null;
  /** Where clients reach taphttp's proxy (display only). */
  proxy: string | null;
  /** Whether spall can replay (lever API mode). */
  replay: boolean;
  /** Hosts replay may send to; empty means none. */
  scope: string[];
  /** Every host is in scope (SPALL_SCOPE=* or `scope all on`). */
  scopeAll: boolean;
  /** The active engagement (#35); null without the lever API. */
  engagement: { id: string; name: string } | null;
  /** Whether the rules of engagement allow testing now (#36), and why not. */
  testing: { allowed: boolean; reason: string | null } | null;
  /** Candidates waiting for a decision (#47); null without the lever API. */
  toTriage: number | null;
  /** The flux crate and spec spall is built with (#16). */
  flux: { crate: string; spec: string; rules: string } | null;
  /** Promoted candidates, and whether an unredacted export is allowed (#51). */
  export: { promoted: number; unredactedAllowed: boolean } | null;
  /** Tasks waiting on a human's approval (#56); null without the lever API. */
  waitingApproval: number | null;
  /** The agent e-stop (#57), while engaged. */
  estop: EStop | null;
  /** The model spall's own agent host runs (#63); null without one. */
  agentModel: string | null;
}

const UNKNOWN = { connected: null, proxy: null, replay: false, scope: [], scopeAll: false, engagement: null, testing: null, toTriage: null, flux: null, export: null, estop: null, waitingApproval: null, agentModel: null };

/** Poll spall's /api/status for the status bar; a new `nonce` polls now. */
export function useSourceStatus(intervalMs = 5000, nonce = 0): SourceStatus {
  const [status, setStatus] = useState<SourceStatus>({ source: "pending", ...UNKNOWN });
  useEffect(() => {
    let live = true;
    const poll = async () => {
      try {
        const res = await fetch("/api/status");
        const body = (await res.json()) as {
          source?: string;
          connected?: boolean | null;
          proxy?: string;
          replay?: boolean;
          scope?: unknown;
          scope_all?: boolean;
          engagement?: { id?: unknown; name?: unknown } | null;
          testing?: unknown;
          to_triage?: unknown;
          flux?: { crate?: unknown; spec?: unknown; rules?: unknown } | null;
          export?: { promoted?: unknown; unredacted_allowed?: unknown } | null;
          estop?: unknown;
          waiting_approval?: unknown;
          agent_host?: { model?: unknown } | null;
        };
        const ex = body.export;
        const fx = body.flux;
        const e = body.engagement;
        if (!live) return;
        setStatus({
          source: body.source === "lever" || body.source === "log" ? body.source : "unknown",
          connected: typeof body.connected === "boolean" ? body.connected : null,
          proxy: typeof body.proxy === "string" ? body.proxy : null,
          replay: body.replay === true,
          scope: Array.isArray(body.scope) ? body.scope.filter((s): s is string => typeof s === "string") : [],
          scopeAll: body.scope_all === true,
          engagement: e && typeof e.id === "string" && typeof e.name === "string" ? { id: e.id, name: e.name } : null,
          testing: testingOf(body.testing),
          toTriage: typeof body.to_triage === "number" && Number.isInteger(body.to_triage) && body.to_triage >= 0 ? body.to_triage : null,
          flux:
            fx && typeof fx.crate === "string" && typeof fx.spec === "string" && typeof fx.rules === "string" ? { crate: fx.crate, spec: fx.spec, rules: fx.rules } : null,
          export: ex && typeof ex.promoted === "number" ? { promoted: ex.promoted, unredactedAllowed: ex.unredacted_allowed === true } : null,
          estop: asEStop(body.estop),
          waitingApproval:
            typeof body.waiting_approval === "number" && Number.isInteger(body.waiting_approval) && body.waiting_approval >= 0 ? body.waiting_approval : null,
          agentModel: typeof body.agent_host?.model === "string" ? body.agent_host.model : null,
        });
      } catch {
        if (live) setStatus({ source: "unknown", ...UNKNOWN });
      }
    };
    void poll();
    const id = setInterval(poll, intervalMs);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [intervalMs, nonce]);
  return status;
}
