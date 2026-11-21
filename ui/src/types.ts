// taphttp TrafficEntry, schema v5, as spall serves it on /events and
// /api/exchanges. The Rust side uses the `taphttp-types` crate; this is the
// browser's view of the same JSON. Every string here is attacker-controlled:
// render as text only.

export type Header = [name: string, value: string];
export type BodyEncoding = "utf8" | "base64";

export interface TrafficEntry {
  schema_version: number;
  id: string;
  ts: string;
  host: string;
  method: string;
  url: string;
  req_headers: Header[];
  req_body: string | null;
  req_body_encoding: BodyEncoding;
  status: number | null;
  res_headers: Header[];
  res_body: string | null;
  res_body_encoding: BodyEncoding;
  duration_ms: number | null;
  replay_of: string | null;
  // v4: who sent it (the proxy-auth username), null when unauthenticated.
  // Optional because v3 lines tailed from an old log omit it.
  source?: string | null;
  // v5: attribution a replay's caller attached ({} otherwise), and why the
  // exchange got no complete response (null when it completed). Optional
  // for the same reason as source.
  meta?: Record<string, string>;
  error?: string | null;
}
