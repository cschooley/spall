// Edit before replay (#19). A draft is the captured request (method,
// ordered headers, a text body) with the tester's changes; this module
// validates it and turns it into what POST /api/replay takes: only the
// fields that changed, so an untouched draft replays as captured. The URL
// never changes: replay always goes to the original target. The server
// validates again and runs the gate (#22) on the effective request.

import type { Header, TrafficEntry } from "../types.ts";
import type { ReplayBody } from "./replay.ts";

export interface Draft {
  method: string;
  headers: Header[];
  /** null when the captured body is binary: sent as captured, not editable. */
  body: string | null;
}

export function draftOf(e: TrafficEntry): Draft {
  return {
    method: e.method,
    headers: e.req_headers.map(([k, v]) => [k, v] as Header),
    body: e.req_body_encoding === "utf8" ? (e.req_body ?? "") : null,
  };
}

const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** Why the draft can't be sent, or null. Messages are safe to show. */
export function validateDraft(d: Draft): string | null {
  if (!TOKEN.test(d.method) || d.method.length > 32) return "method must be an HTTP token (letters, digits, -)";
  for (const [name, value] of d.headers) {
    if (!TOKEN.test(name) || name.length > 256) return `invalid header name ${JSON.stringify(name)}`;
    if (/[\r\n\0]/.test(value)) return `header ${JSON.stringify(name)}: values can't contain CR or LF (or NUL)`;
  }
  return null;
}

const utf8Length = (s: string) => new TextEncoder().encode(s).length;

/**
 * taphttp sends the header list as given, so a captured Content-Length
 * must follow an edited body. No Content-Length captured: none added.
 */
export function syncContentLength(headers: Header[], body: string): { headers: Header[]; updated: boolean } {
  const len = String(utf8Length(body));
  let updated = false;
  const out = headers.map(([k, v]): Header => {
    if (k.toLowerCase() !== "content-length" || v === len) return [k, v];
    updated = true;
    return [k, len];
  });
  return { headers: out, updated };
}

const sameHeaders = (a: Header[], b: Header[]) => a.length === b.length && a.every(([k, v], i) => k === b[i]![0] && v === b[i]![1]);

export type Built =
  | { ok: true; body: ReplayBody; /** set when Content-Length was rewritten */ contentLength: number | null }
  | { ok: false; error: string };

export function replayBodyFor(original: TrafficEntry, draft: Draft): Built {
  const invalid = validateDraft(draft);
  if (invalid) return { ok: false, error: invalid };
  const body: ReplayBody = { id: original.id };
  if (draft.method !== original.method) body.method = draft.method;

  let headers = draft.headers;
  let contentLength: number | null = null;
  const bodyChanged = draft.body !== null && original.req_body_encoding === "utf8" && draft.body !== (original.req_body ?? "");
  if (bodyChanged) {
    body.body = draft.body!;
    body.body_encoding = "utf8";
    const synced = syncContentLength(headers, draft.body!);
    if (synced.updated) contentLength = utf8Length(draft.body!);
    headers = synced.headers;
  }
  if (!sameHeaders(headers, original.req_headers)) body.req_headers = headers;
  return { ok: true, body, contentLength };
}

export interface Changes {
  method: boolean;
  body: boolean;
  /** Per draft row: not an exact [name, value] pair of the original. */
  headers: boolean[];
  /** Captured headers no longer in the draft (a changed value isn't a removal). */
  removed: number;
  count: number;
}

export function changes(original: TrafficEntry, draft: Draft): Changes {
  const key = ([k, v]: Header) => `${k.toLowerCase()}\n${v}`;
  const pool = new Map<string, number>();
  for (const h of original.req_headers) pool.set(key(h), (pool.get(key(h)) ?? 0) + 1);
  const headers = draft.headers.map((h) => {
    const n = pool.get(key(h)) ?? 0;
    if (n === 0) return true;
    pool.set(key(h), n - 1);
    return false;
  });
  // A changed row whose name matches a captured header that's gone is a
  // modified value, not an addition plus a removal.
  const goneByName = new Map<string, number>();
  for (const [k, n] of pool) {
    const name = k.split("\n", 1)[0]!;
    goneByName.set(name, (goneByName.get(name) ?? 0) + n);
  }
  draft.headers.forEach(([k], i) => {
    const name = k.toLowerCase();
    const n = goneByName.get(name) ?? 0;
    if (headers[i] && n > 0) goneByName.set(name, n - 1);
  });
  const removed = [...goneByName.values()].reduce((a, b) => a + b, 0);
  const method = draft.method !== original.method;
  const body = draft.body !== null && draft.body !== (original.req_body ?? "");
  return { method, body, headers, removed, count: Number(method) + Number(body) + headers.filter(Boolean).length + removed };
}

/** The Host header's value when it names a different host than the URL. */
export function hostMismatch(url: string, headers: Header[]): string | null {
  const host = headers.find(([k]) => k.toLowerCase() === "host")?.[1];
  if (host === undefined) return null;
  let target: URL;
  try {
    target = new URL(url);
  } catch {
    return host;
  }
  const defaultPort = target.protocol === "https:" ? "443" : "80";
  const norm = (h: string) => {
    const t = h.trim().toLowerCase().replace(/\.$/, "");
    return t.endsWith(`:${defaultPort}`) ? t.slice(0, -defaultPort.length - 1) : t;
  };
  return norm(host) === norm(target.host) ? null : host;
}

/** A name repeated with another header in between: the client sends them grouped. */
export function scatteredRepeats(headers: Header[]): boolean {
  const seen = new Set<string>();
  let last = "";
  for (const [k] of headers) {
    const name = k.toLowerCase();
    if (name !== last && seen.has(name)) return true;
    seen.add(name);
    last = name;
  }
  return false;
}

/**
 * The console's `--header 'Name: value'` (replaces every value for the
 * name, at its first position, or appends) and `--drop-header Name`.
 */
export function applyHeaderEdits(headers: Header[], sets: Header[], drops: string[]): Header[] {
  const dropped = new Set(drops.map((d) => d.toLowerCase()));
  let out: Header[] = headers.filter(([k]) => !dropped.has(k.toLowerCase())).map(([k, v]) => [k, v] as Header);
  for (const [name, value] of sets) {
    const lower = name.toLowerCase();
    const first = out.findIndex(([k]) => k.toLowerCase() === lower);
    if (first < 0) {
      out.push([name, value]);
      continue;
    }
    out = out.filter(([k], i) => i === first || k.toLowerCase() !== lower);
    out[first] = [name, value];
  }
  return out;
}
