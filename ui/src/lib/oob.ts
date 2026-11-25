// Out-of-band callbacks (#1): payloads you put in a request, and the hits
// a target makes on them (DNS, HTTP, SMTP). The server records and links
// them; a hit's raw request is captured data, shown as text only.
import { apiJson } from "./engagement.ts";

export interface OobPayload {
  id: string;
  host: string;
  url: string;
  labUrl: string | null;
  by: string;
  at: string;
}

export interface OobHit {
  id: string;
  payload: string;
  protocol: string;
  remote: string;
  at: string;
  rawRequest: string;
  task: string | null;
  exchange: string | null;
}

export type OobState = { configured: false } | { configured: true; domain: string; payloads: OobPayload[]; hits: OobHit[] };

const str = (v: unknown) => (typeof v === "string" ? v : "");
const opt = (v: unknown) => (typeof v === "string" ? v : null);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

export function asPayload(v: unknown): OobPayload | null {
  const o = obj(v);
  if (typeof o.id !== "string" || typeof o.host !== "string") return null;
  return { id: o.id, host: o.host, url: str(o.url), labUrl: opt(o.lab_url), by: str(o.by), at: str(o.at) };
}

export function asOob(v: unknown): OobState | string {
  if (typeof v === "string") return v;
  const o = obj(v);
  if (o.configured !== true) return { configured: false };
  return {
    configured: true,
    domain: str(o.domain),
    payloads: (Array.isArray(o.payloads) ? o.payloads : []).flatMap((p) => {
      const x = asPayload(p);
      return x ? [x] : [];
    }),
    hits: (Array.isArray(o.hits) ? o.hits : []).map((h) => {
      const x = obj(h);
      return { id: str(x.id), payload: str(x.payload), protocol: str(x.protocol), remote: str(x.remote), at: str(x.at), rawRequest: str(x.raw_request), task: opt(x.task), exchange: opt(x.exchange) };
    }),
  };
}

export async function loadOob(): Promise<OobState | string> {
  return asOob(await apiJson("/api/oob"));
}

export async function mintOob(): Promise<OobPayload | string> {
  const v = await apiJson("/api/oob", { method: "POST", body: {} });
  if (typeof v === "string") return v;
  return asPayload(v) ?? "unexpected answer from spall";
}

/** A hit as one console line. */
export function hitLine(h: OobHit): string {
  return `${h.id}  ${h.at}  ${h.protocol.padEnd(5)} from ${h.remote}  on ${h.payload}${h.exchange ? `  via ${h.exchange}` : ""}${h.task ? `  (${h.task})` : ""}`;
}
