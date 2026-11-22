import { useCallback, useEffect, useRef, useState } from "react";
import { apiParams, type Query } from "./lib/query.ts";
import type { TrafficEntry } from "./types.ts";

export const PAGE_SIZE = 200;

/** One empty list, so "off" returns the same items every render (a fresh
 * [] each time made everything derived from it look changed, #21). */
const NO_ITEMS: TrafficEntry[] = [];

export interface History {
  /** Newest first, as /api/exchanges returns them. */
  items: TrafficEntry[];
  loading: boolean;
  hasMore: boolean;
  /** spall is tailing a log file: no history API, live rows only. */
  unavailable: boolean;
  error: string | null;
  loadOlder: () => void;
}

type PageResult =
  | { kind: "page"; page: TrafficEntry[] }
  | { kind: "unavailable" }
  | { kind: "error"; message: string }
  | { kind: "aborted" };

/** One page of /api/exchanges. Pure: no React state. */
async function loadPage(params: [string, string][], before: string | null, signal: AbortSignal): Promise<PageResult> {
  const qs = new URLSearchParams(params);
  qs.set("limit", String(PAGE_SIZE));
  if (before) qs.set("before", before);
  try {
    const res = await fetch(`/api/exchanges?${qs}`, { signal });
    if (res.status === 503) return { kind: "unavailable" };
    if (!res.ok) return { kind: "error", message: `history: HTTP ${res.status}` };
    return { kind: "page", page: (await res.json()) as TrafficEntry[] };
  } catch (e) {
    return (e as Error).name === "AbortError" ? { kind: "aborted" } : { kind: "error", message: (e as Error).message };
  }
}

/**
 * History for a filter, from spall's /api/exchanges (taphttp's sqlite),
 * paged backwards with `before=<oldest id>`. Changing the query restarts
 * from the newest page and cancels any request in flight.
 */
export function useHistory(query: Query, mode: "wait" | "fetch" | "off"): History {
  const [items, setItems] = useState<TrafficEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inflight = useRef<AbortController | null>(null);
  const key = JSON.stringify(apiParams(query));

  // A new query starts over. Reset during render (React's pattern for state
  // derived from props); effects only set state from async results.
  const [shownKey, setShownKey] = useState(key);
  if (shownKey !== key) {
    setShownKey(key);
    setItems([]);
    setHasMore(false);
    setLoading(true);
  }

  const apply = useCallback((r: PageResult, reset: boolean) => {
    if (r.kind === "aborted") return;
    setLoading(false);
    if (r.kind === "unavailable") {
      setUnavailable(true);
      setHasMore(false);
      return;
    }
    if (r.kind === "error") {
      setError(r.message);
      return;
    }
    setUnavailable(false);
    setError(null);
    setItems((prev) => (reset ? r.page : prev.concat(r.page)));
    setHasMore(r.page.length === PAGE_SIZE);
  }, []);

  const start = useCallback(
    (before: string | null) => {
      inflight.current?.abort();
      const ctrl = new AbortController();
      inflight.current = ctrl;
      return loadPage(JSON.parse(key) as [string, string][], before, ctrl.signal);
    },
    [key],
  );

  useEffect(() => {
    // "off": spall tails a log file, so there is no history API to ask (and
    // asking would only log a 503). "wait": /api/status hasn't answered yet.
    if (mode !== "fetch") return;
    start(null).then((r) => apply(r, true));
    return () => inflight.current?.abort();
  }, [start, apply, mode]);

  const loadOlder = useCallback(() => {
    if (loading || !hasMore || items.length === 0) return;
    setLoading(true);
    start(items[items.length - 1]!.id).then((r) => apply(r, false));
  }, [loading, hasMore, items, start, apply]);

  if (mode === "off") return { items: NO_ITEMS, loading: false, hasMore: false, unavailable: true, error: null, loadOlder };
  return { items, loading: loading && mode === "fetch", hasMore, unavailable, error, loadOlder };
}
