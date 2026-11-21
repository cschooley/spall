// Pure formatting helpers. No DOM access: everything here returns plain
// strings that components render as text.

import type { Header } from "../types.ts";

export function pathOf(url: string): string {
  if (url.startsWith("/")) return url;
  try {
    const u = new URL(url);
    return u.pathname + u.search;
  } catch {
    return url;
  }
}

/** Local HH:MM:SS.mmm; the raw value if it is not a date. */
export function timeOf(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

export function statusClass(status: number | null): string {
  if (status === null) return "st-err";
  const k = Math.floor(status / 100);
  return k >= 2 && k <= 5 ? `st-${k}` : "";
}

export function statusText(status: number | null): string {
  return status === null ? "ERR" : String(status);
}

export function durationText(ms: number | null): string {
  return ms === null ? "" : `${ms} ms`;
}

/** Headers as [name, value] pairs, tolerating a v2 map or malformed input. */
function pairs(h: unknown): Header[] {
  if (Array.isArray(h)) {
    return h.filter(
      (p): p is Header =>
        Array.isArray(p) && p.length === 2 && typeof p[0] === "string" && typeof p[1] === "string",
    );
  }
  if (h && typeof h === "object") {
    return Object.entries(h as Record<string, unknown>).map(([k, v]) => [k, String(v)]);
  }
  return [];
}

export interface HeaderRow {
  name: string;
  value: string;
  /** "n/N" when the name repeats, else null. */
  count: string | null;
}

/** Wire order, repeats kept and never merged; repeated names get n/N. */
export function headerRows(h: unknown): HeaderRow[] {
  const list = pairs(h);
  const totals = new Map<string, number>();
  for (const [k] of list) totals.set(k.toLowerCase(), (totals.get(k.toLowerCase()) ?? 0) + 1);
  const seen = new Map<string, number>();
  return list.map(([name, value]) => {
    const key = name.toLowerCase();
    const total = totals.get(key) ?? 1;
    if (total < 2) return { name, value, count: null };
    const n = (seen.get(key) ?? 0) + 1;
    seen.set(key, n);
    return { name, value, count: `${n}/${total}` };
  });
}

export function headerValue(h: unknown, name: string): string | null {
  const found = pairs(h).find(([k]) => k.toLowerCase() === name);
  return found ? found[1] : null;
}

/** Decode standard base64; throws on invalid input. */
export function base64Bytes(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export const HEX_BYTES_PER_ROW = 8;
export const HEX_ROWS_PER_PAGE = 64;

/** One row per 8 bytes: offset, hex pairs, printable ASCII ('.' otherwise). */
export function hexRows(bytes: Uint8Array, startRow: number, rowCount: number): string {
  const lines: string[] = [];
  for (let r = startRow; r < startRow + rowCount; r++) {
    const off = r * HEX_BYTES_PER_ROW;
    if (off >= bytes.length) break;
    const chunk = Array.from(bytes.subarray(off, off + HEX_BYTES_PER_ROW));
    const hex = chunk.map((b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = chunk.map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${off.toString(16).padStart(8, "0")}  ${hex.padEnd(HEX_BYTES_PER_ROW * 3 - 1)}  ${ascii}`);
  }
  return lines.join("\n");
}

export const MAX_BODY_CHARS = 256 * 1024;

/** Text body for display: JSON pretty-printed when it parses, then truncated. */
export function prettyBody(
  body: string,
  contentType: string | null,
  maxChars = MAX_BODY_CHARS,
): { text: string; truncated: boolean } {
  let text = body;
  if ((contentType && /json/i.test(contentType)) || /^\s*[[{]/.test(body)) {
    try {
      text = JSON.stringify(JSON.parse(body), null, 2);
    } catch {
      // not JSON after all; show as-is
    }
  }
  return text.length > maxChars
    ? { text: text.slice(0, maxChars), truncated: true }
    : { text, truncated: false };
}

export async function sha256Hex(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** The console filter: substring match over method, host, path and status. */
export function matchesFilter(
  e: { method: string; host: string; url: string; status: number | null },
  q: string,
): boolean {
  if (q === "") return true;
  return `${e.method} ${e.host} ${pathOf(e.url)} ${statusText(e.status)}`.toLowerCase().includes(q);
}

const KNOWN_METHODS = new Set(["GET", "POST", "PUT", "PATCH", "DELETE"]);

/** CSS class for a method's color. A fixed set: the method string comes
 * from traffic and must never become a class name. */
export function methodClass(method: string): string {
  const m = method.toUpperCase();
  return KNOWN_METHODS.has(m) ? `m-${m}` : "m-OTHER";
}

const REASONS: Record<number, string> = {
  200: "OK", 201: "Created", 202: "Accepted", 204: "No Content", 206: "Partial Content",
  301: "Moved Permanently", 302: "Found", 303: "See Other", 304: "Not Modified", 307: "Temporary Redirect", 308: "Permanent Redirect",
  400: "Bad Request", 401: "Unauthorized", 403: "Forbidden", 404: "Not Found", 405: "Method Not Allowed",
  409: "Conflict", 410: "Gone", 413: "Content Too Large", 415: "Unsupported Media Type", 422: "Unprocessable Content", 429: "Too Many Requests",
  500: "Internal Server Error", 501: "Not Implemented", 502: "Bad Gateway", 503: "Service Unavailable", 504: "Gateway Timeout",
};

export function statusLine(status: number | null): string {
  if (status === null) return "ERR (no response)";
  const reason = REASONS[status];
  return reason ? `${status} ${reason}` : String(status);
}

/** The body as captured bytes, for saving; null if empty or undecodable. */
export function bodyBytes(body: string | null, encoding: "utf8" | "base64"): Uint8Array<ArrayBuffer> | null {
  if (!body) return null;
  if (encoding === "utf8") return new TextEncoder().encode(body);
  try {
    return base64Bytes(body);
  } catch {
    return null;
  }
}

/** Long ids shortened for display (full id stays in a tooltip and copy). */
export function shortId(id: string): string {
  return id.length > 14 ? `${id.slice(0, 8)}…${id.slice(-3)}` : id;
}

/** The URL for display, without the scheme's default port (taphttp records
 * `https://host:443/…`). The raw URL stays in tooltips and copy as curl. */
export function displayUrl(url: string): string {
  const m = /^(https?):\/\/([^/?#]+):(\d+)(.*)$/i.exec(url);
  if (!m) return url;
  const [, scheme, host, port, rest] = m as unknown as [string, string, string, string, string];
  const isDefault = (scheme.toLowerCase() === "https" && port === "443") || (scheme.toLowerCase() === "http" && port === "80");
  return isDefault ? `${scheme}://${host}${rest}` : url;
}

export function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
