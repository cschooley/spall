// The active engagement and the list, for the Engagement view (#37).
// Polls so "testing now" follows the clock; refresh() after any change.

import { useCallback, useEffect, useState } from "react";
import type { EngagementInfo, EngagementSummary } from "./commands/registry.ts";
import { apiJson, asEngagement, asSummaries, todayIn } from "./lib/engagement.ts";

export interface EngagementState {
  engagement: EngagementInfo | null;
  list: EngagementSummary[];
  /** Today in the rules' time zone, for the expiry countdown. */
  today: string | null;
  error: string | null;
  loading: boolean;
}

export function useEngagement(enabled: boolean, intervalMs = 30_000): EngagementState & { refresh(): void } {
  const [state, setState] = useState<EngagementState>({ engagement: null, list: [], today: null, error: null, loading: true });
  const [nonce, setNonce] = useState(0);
  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    const load = async () => {
      const [e, l] = await Promise.all([apiJson("/api/engagement"), apiJson("/api/engagements")]);
      if (!live) return;
      const engagement = typeof e === "string" ? null : asEngagement(e);
      setState({
        engagement,
        list: typeof l === "string" ? [] : (asSummaries(l) ?? []),
        today: engagement ? todayIn(engagement.rules.time_zone, new Date()) : null,
        error: typeof e === "string" ? e : engagement ? null : "unexpected answer from spall",
        loading: false,
      });
    };
    void load();
    const id = setInterval(load, intervalMs);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [enabled, intervalMs, nonce]);
  return { ...state, refresh };
}
