import { useEffect, useState } from "react";

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
}

const UNKNOWN = { connected: null, proxy: null, replay: false, scope: [], scopeAll: false };

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
        };
        if (!live) return;
        setStatus({
          source: body.source === "lever" || body.source === "log" ? body.source : "unknown",
          connected: typeof body.connected === "boolean" ? body.connected : null,
          proxy: typeof body.proxy === "string" ? body.proxy : null,
          replay: body.replay === true,
          scope: Array.isArray(body.scope) ? body.scope.filter((s): s is string => typeof s === "string") : [],
          scopeAll: body.scope_all === true,
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
