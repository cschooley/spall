import { useEffect, useState } from "react";

export interface SourceStatus {
  /** pending: not asked yet; unknown: /api/status failed. */
  source: "pending" | "lever" | "log" | "unknown";
  /** Following taphttp's feed (lever); null when tailing a log file. */
  connected: boolean | null;
}

/** Poll spall's /api/status for the status bar. */
export function useSourceStatus(intervalMs = 5000): SourceStatus {
  const [status, setStatus] = useState<SourceStatus>({ source: "pending", connected: null });
  useEffect(() => {
    let live = true;
    const poll = async () => {
      try {
        const res = await fetch("/api/status");
        const body = (await res.json()) as { source?: string; connected?: boolean | null };
        if (!live) return;
        setStatus({
          source: body.source === "lever" || body.source === "log" ? body.source : "unknown",
          connected: typeof body.connected === "boolean" ? body.connected : null,
        });
      } catch {
        if (live) setStatus({ source: "unknown", connected: null });
      }
    };
    void poll();
    const id = setInterval(poll, intervalMs);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [intervalMs]);
  return status;
}
