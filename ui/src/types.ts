// taphttp TrafficEntry, schema v3, as spall serves it on /events and
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
}
