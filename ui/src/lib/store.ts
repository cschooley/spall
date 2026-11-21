// The console's list of exchanges: ordered, deduped by id, capped.
// Immutable updates so React re-renders only when something changed.

import type { TrafficEntry } from "../types.ts";

export const MAX_ROWS = 5000;

export interface Store {
  items: TrafficEntry[];
  /** Every id ever shown (including cleared ones), minus evictions. */
  seen: ReadonlySet<string>;
}

export function emptyStore(): Store {
  return { items: [], seen: new Set() };
}

export function addEntries(store: Store, entries: TrafficEntry[], cap = MAX_ROWS): Store {
  const fresh = entries.filter((e) => typeof e?.id === "string" && !store.seen.has(e.id));
  if (fresh.length === 0) return store;
  const seen = new Set(store.seen);
  const unique: TrafficEntry[] = [];
  for (const e of fresh) {
    if (seen.has(e.id)) continue;
    seen.add(e.id);
    unique.push(e);
  }
  let items = store.items.concat(unique);
  if (items.length > cap) {
    for (const old of items.slice(0, items.length - cap)) seen.delete(old.id);
    items = items.slice(items.length - cap);
  }
  return { items, seen };
}

/** Hide everything but keep ids, so an SSE reconnect does not re-add them. */
export function clearStore(store: Store): Store {
  return { items: [], seen: store.seen };
}
