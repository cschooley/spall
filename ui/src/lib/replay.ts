// Replay from the browser (#18). The rules live on the server (#22: scope,
// unsafe-method confirmation, audit); this module only reads its answers
// and keeps the UI's side of the two-press flow: the first press of an
// unsafe method gets a single-use token, the second sends it.

import type { TrafficEntry } from "../types.ts";

/** What spall's POST /api/replay accepts; edits (#19) are optional. */
export interface ReplayBody {
  id: string;
  method?: string;
  /** The whole ordered header list, replacing the captured one. */
  req_headers?: [string, string][];
  body?: string;
  body_encoding?: "utf8" | "base64";
  confirm?: string;
}

/** What spall's POST /api/fetch accepts (#74): a new GET or HEAD. */
export interface FetchBody {
  url: string;
  method?: "GET" | "HEAD";
  /** The whole ordered header list; without it taphttp sends its own user-agent. */
  req_headers?: [string, string][];
  confirm?: string;
}

export type ReplayResult =
  /** Sent; taphttp recorded the replay. */
  | { kind: "sent"; entry: TrafficEntry }
  /** Sent, and the target didn't answer; taphttp recorded the attempt. */
  | { kind: "failed"; entry: TrafficEntry }
  /** An unsafe method: send again with `token` to confirm. Nothing was sent. */
  | { kind: "confirm"; token: string; reason: string }
  /** Out of scope (or no scope). Nothing was sent. */
  | { kind: "denied"; reason: string }
  /** The confirmation was unknown, used, expired or for another request. */
  | { kind: "refused"; reason: string }
  /** Bad input, unknown exchange, no lever API, spall or taphttp down. */
  | { kind: "error"; reason: string };

/** How long the button stays armed after a "confirm" answer. */
export const ARM_MS = 5000;

export interface Armed {
  id: string;
  token: string;
  label: string;
  /** Date.now() after which the arming no longer counts. */
  until: number;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** Enough of an entry to open in the inspector. */
const isEntry = (v: unknown): v is TrafficEntry =>
  isObject(v) && typeof v.id === "string" && typeof v.method === "string" && typeof v.url === "string";

export function readReplayResponse(status: number, body: unknown): ReplayResult {
  const msg = isObject(body) && typeof body.error === "string" ? body.error : `replay failed (HTTP ${status})`;
  if (status === 200) return isEntry(body) ? { kind: "sent", entry: body } : { kind: "error", reason: "spall returned no replay" };
  if (status === 502 && isEntry(body)) return { kind: "failed", entry: body };
  if (status === 428) {
    return isObject(body) && typeof body.confirm === "string" && body.confirm
      ? { kind: "confirm", token: body.confirm, reason: msg }
      : { kind: "error", reason: "spall asked for confirmation without a token" };
  }
  // 403: the gate said no; 429: over the rules' rate ceiling (#36).
  if (status === 403 || status === 429) return { kind: "denied", reason: msg };
  if (status === 409) return { kind: "refused", reason: msg };
  return { kind: "error", reason: msg };
}

async function post(path: string, body: unknown, fetchFn: typeof fetch): Promise<ReplayResult> {
  let res: Response;
  try {
    res = await fetchFn(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    return { kind: "error", reason: "spall unreachable" };
  }
  let json: unknown = null;
  try {
    json = await res.json();
  } catch {
    // not JSON: readReplayResponse reports the status
  }
  return readReplayResponse(res.status, json);
}

export function postReplay(body: ReplayBody, fetchFn: typeof fetch = fetch): Promise<ReplayResult> {
  return post("/api/replay", body, fetchFn);
}

/** A new request through the same gate (#74); the answers read like a replay's. */
export function postFetch(body: FetchBody, fetchFn: typeof fetch = fetch): Promise<ReplayResult> {
  return post("/api/fetch", body, fetchFn);
}

/** The arming, if it still applies to this exchange now. */
export function armedFor(armed: Armed | null, id: string, now: number): Armed | null {
  return armed && armed.id === id && now < armed.until ? armed : null;
}

export function armLabel(method: string, host: string | null): string {
  return host ? `press again to send ${method} to ${host}` : `press again to send ${method}`;
}
