import { useCallback, useEffect, useRef, useState } from "react";
import { addEntries, clearStore, emptyStore, type Store } from "./lib/store.ts";
import type { TrafficEntry } from "./types.ts";

export type Connection = "connecting" | "live" | "reconnecting" | "down";

/**
 * Follow spall's /events SSE stream. Events are batched per animation frame
 * so a burst of traffic causes one render, not hundreds.
 */
export function useTraffic(): { store: Store; connection: Connection; clear: () => void } {
  const [store, setStore] = useState<Store>(emptyStore);
  const [connection, setConnection] = useState<Connection>("connecting");
  const pending = useRef<TrafficEntry[]>([]);
  const frame = useRef<number | null>(null);

  useEffect(() => {
    const source = new EventSource("/events");
    const flush = () => {
      frame.current = null;
      const batch = pending.current;
      pending.current = [];
      setStore((s) => addEntries(s, batch));
    };
    source.onopen = () => setConnection("live");
    source.onerror = () =>
      setConnection(source.readyState === EventSource.CLOSED ? "down" : "reconnecting");
    source.onmessage = (ev: MessageEvent<string>) => {
      let entry: TrafficEntry;
      try {
        entry = JSON.parse(ev.data) as TrafficEntry;
      } catch {
        return;
      }
      pending.current.push(entry);
      frame.current ??= requestAnimationFrame(flush);
    };
    return () => {
      source.close();
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    };
  }, []);

  const clear = useCallback(() => setStore(clearStore), []);
  return { store, connection, clear };
}
