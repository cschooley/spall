import { useEffect, useRef, useState } from "react";
import { chainOf } from "./lib/replays.ts";
import type { TrafficEntry } from "./types.ts";

/**
 * Replays of one exchange from taphttp's history (`replay_of=<id>`, #21).
 * Live replays are merged in by the caller, so the list grows as they
 * arrive. `complete` is false when there's no history API (log-file mode):
 * then only loaded replays are known.
 */
export function useReplayHistory(id: string | null, mode: "wait" | "fetch" | "off") {
  const [state, setState] = useState<{ id: string | null; items: TrafficEntry[]; error: string | null }>({
    id: null,
    items: [],
    error: null,
  });
  useEffect(() => {
    if (!id || mode !== "fetch") return;
    const ctl = new AbortController();
    (async () => {
      try {
        const res = await fetch(`/api/exchanges?replay_of=${encodeURIComponent(id)}&limit=500`, { signal: ctl.signal });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        setState({ id, items: (await res.json()) as TrafficEntry[], error: null });
      } catch (e) {
        if ((e as Error).name !== "AbortError") setState({ id, items: [], error: `replays: ${(e as Error).message}` });
      }
    })();
    return () => ctl.abort();
  }, [id, mode]);
  const current = state.id === id;
  return {
    items: current ? state.items : [],
    complete: mode === "fetch",
    error: current ? state.error : null,
  };
}

/**
 * The chain from the first original to `entry` (ids, oldest first),
 * looking up ancestors that aren't loaded. `missing` is set when one
 * couldn't be found.
 */
export function useChain(entry: TrafficEntry | null, find: (id: string) => Promise<TrafficEntry | null>) {
  const [state, setState] = useState<{ for: string | null; chain: string[]; missing: string | null }>({
    for: null,
    chain: [],
    missing: null,
  });
  // The lookup changes whenever traffic arrives; the chain doesn't. Keep the
  // latest in a ref so the walk runs per exchange, not per render.
  const findRef = useRef(find);
  useEffect(() => {
    findRef.current = find;
  }, [find]);
  useEffect(() => {
    if (!entry?.replay_of) return;
    let live = true;
    (async () => {
      const known = new Map<string, TrafficEntry>([[entry.id, entry]]);
      let cur: TrafficEntry | null = entry;
      // Fetch ancestors one by one, then let chainOf do the (pure) walk.
      for (let i = 0; i < 50 && cur?.replay_of && !known.has(cur.replay_of); i++) {
        const prev: TrafficEntry | null = await findRef.current(cur.replay_of);
        if (!prev) break;
        known.set(prev.id, prev);
        cur = prev;
      }
      const { chain, missing } = chainOf(entry, (id) => known.get(id));
      if (live) setState({ for: entry.id, chain, missing });
    })();
    return () => {
      live = false;
    };
  }, [entry]);
  return state.for === entry?.id ? state : { chain: entry ? [entry.id] : [], missing: null };
}

/**
 * The exchange a replay was replayed from, for Compare (#20). `undefined`
 * while looking, null if it can't be found.
 */
export function useOriginal(entry: TrafficEntry | null, find: (id: string) => Promise<TrafficEntry | null>) {
  const [state, setState] = useState<{ for: string | null; original: TrafficEntry | null }>({ for: null, original: null });
  const findRef = useRef(find);
  useEffect(() => {
    findRef.current = find;
  }, [find]);
  const id = entry?.replay_of ?? null;
  useEffect(() => {
    if (!id) return;
    let live = true;
    void findRef.current(id).then((original) => {
      if (live) setState({ for: id, original });
    });
    return () => {
      live = false;
    };
  }, [id]);
  if (!id) return null;
  return state.for === id ? state.original : undefined;
}
