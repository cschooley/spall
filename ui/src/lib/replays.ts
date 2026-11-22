// Originals and their replays (#21). taphttp records each replay as a new
// exchange with `replay_of` pointing at what was replayed, so a replay of
// a replay forms a chain back to the first original.

import { changes, draftOf } from "./edit.ts";
import type { TrafficEntry } from "../types.ts";

/** Replays of `id` across sources (history, live), once each, newest first. */
export function replaysOf(id: string, sources: readonly (readonly TrafficEntry[])[]): TrafficEntry[] {
  const byId = new Map<string, TrafficEntry>();
  for (const list of sources) for (const e of list) if (e.replay_of === id && !byId.has(e.id)) byId.set(e.id, e);
  return [...byId.values()].sort((a, b) => Date.parse(b.ts) - Date.parse(a.ts));
}

const MAX_CHAIN = 50;

/**
 * Ids from the first original to `entry`, oldest first. `missing` is the
 * first id that couldn't be looked up. A loop (bad data) stops the walk.
 */
export function chainOf(
  entry: TrafficEntry,
  lookup: (id: string) => TrafficEntry | undefined,
): { chain: string[]; missing: string | null } {
  const chain = [entry.id];
  const seen = new Set(chain);
  let cur = entry;
  while (cur.replay_of && chain.length < MAX_CHAIN) {
    const id = cur.replay_of;
    if (seen.has(id)) break;
    const prev = lookup(id);
    if (!prev) return { chain: chain.reverse(), missing: id };
    chain.push(id);
    seen.add(id);
    cur = prev;
  }
  return { chain: chain.reverse(), missing: null };
}

/** Headers the HTTP client adds on send; their appearance isn't an edit. */
const ADDED_ON_SEND = new Set(["content-length", "host"]);

/** What a replay changed relative to what it replayed: "method PUT · 2 headers · body". */
export function editSummary(original: TrafficEntry | null, replay: TrafficEntry): string {
  if (!original) return "";
  const draft = draftOf(replay);
  const c = changes(original, { ...draft, body: replay.req_body_encoding === "utf8" ? (replay.req_body ?? "") : null });
  const headers = c.headers.filter((changed, i) => changed && !ADDED_ON_SEND.has(draft.headers[i]![0].toLowerCase())).length + c.removed;
  const body =
    replay.req_body_encoding !== original.req_body_encoding || (replay.req_body ?? "") !== (original.req_body ?? "");
  const parts: string[] = [];
  if (replay.method !== original.method) parts.push(`method ${replay.method}`);
  if (headers) parts.push(`${headers} header${headers === 1 ? "" : "s"}`);
  if (body) parts.push("body");
  return parts.length ? parts.join(" · ") : "as captured";
}
