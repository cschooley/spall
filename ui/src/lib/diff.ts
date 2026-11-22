// Compare two exchanges (#20): a replay with its original now, a finding's
// evidence (M4) and a retest with the original (M8) later. Pure: returns
// data, never markup; the view renders every piece as text.

import { bodyBytes, headerValue } from "./format.ts";
import type { BodyEncoding, Header, TrafficEntry } from "../types.ts";

export type Line = { kind: "same" | "add" | "del"; text: string };

/** Lines per side above which bodies aren't diffed line by line. */
export const MAX_LINES = 5000;

/** Longest-common-subsequence line diff. a is the original, b the new. */
export function lineDiff(a: string[], b: string[]): Line[] {
  // Trim the common prefix and suffix first: replays mostly differ a little.
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const n = midA.length;
  const m = midB.length;
  // lcs[i][j]: LCS length of midA[i..] and midB[j..], in one flat array.
  const lcs = new Uint32Array((n + 1) * (m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * (m + 1) + j] = midA[i] === midB[j] ? lcs[(i + 1) * (m + 1) + j + 1]! + 1 : Math.max(lcs[(i + 1) * (m + 1) + j]!, lcs[i * (m + 1) + j + 1]!);
    }
  }
  const out: Line[] = a.slice(0, start).map((text) => ({ kind: "same", text }));
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (midA[i] === midB[j]) {
      out.push({ kind: "same", text: midA[i]! });
      i++;
      j++;
    } else if (lcs[(i + 1) * (m + 1) + j]! >= lcs[i * (m + 1) + j + 1]!) out.push({ kind: "del", text: midA[i++]! });
    else out.push({ kind: "add", text: midB[j++]! });
  }
  while (i < n) out.push({ kind: "del", text: midA[i++]! });
  while (j < m) out.push({ kind: "add", text: midB[j++]! });
  for (const text of a.slice(endA)) out.push({ kind: "same", text });
  return out;
}

const sortKeys = (v: unknown): unknown =>
  Array.isArray(v)
    ? v.map(sortKeys)
    : v && typeof v === "object"
      ? Object.fromEntries(Object.keys(v as object).sort().map((k) => [k, sortKeys((v as Record<string, unknown>)[k])]))
      : v;

/** Text to diff: JSON pretty-printed with sorted keys, anything else as is. */
export function normalizedText(body: string | null, encoding: BodyEncoding, contentType: string | null): string {
  if (!body || encoding !== "utf8") return body ?? "";
  if (contentType && /[/+]json\b/i.test(contentType)) {
    try {
      return JSON.stringify(sortKeys(JSON.parse(body)), null, 2);
    } catch {
      return body;
    }
  }
  return body;
}

export type BodyDiff =
  | { kind: "text"; lines: Line[]; changed: number }
  | { kind: "binary"; same: boolean; sizeA: number; sizeB: number }
  | { kind: "too-large"; same: boolean; sizeA: number; sizeB: number };

const sameBytes = (x: Uint8Array | null, y: Uint8Array | null) =>
  (x?.length ?? 0) === (y?.length ?? 0) && (x ?? []).every((b, i) => b === y![i]);

export function bodyDiff(a: string | null, encA: BodyEncoding, b: string | null, encB: BodyEncoding, contentType: string | null): BodyDiff {
  const bytesA = bodyBytes(a, encA);
  const bytesB = bodyBytes(b, encB);
  const sizes = { sizeA: bytesA?.length ?? 0, sizeB: bytesB?.length ?? 0 };
  if (encA !== "utf8" || encB !== "utf8") return { kind: "binary", same: sameBytes(bytesA, bytesB), ...sizes };
  const la = normalizedText(a, encA, contentType).split("\n");
  const lb = normalizedText(b, encB, contentType).split("\n");
  if (la.length > MAX_LINES || lb.length > MAX_LINES) return { kind: "too-large", same: sameBytes(bytesA, bytesB), ...sizes };
  const lines = !a && !b ? [] : lineDiff(la, lb);
  return { kind: "text", lines, changed: lines.filter((l) => l.kind !== "same").length };
}

/** Headers that change on every response; marked in a diff, not hidden. */
export const VOLATILE = new Set([
  "date", "etag", "age", "expires", "last-modified", "x-request-id", "request-id", "x-correlation-id", "x-amzn-requestid",
  "x-amz-request-id", "x-amz-id-2", "cf-ray", "x-runtime", "server-timing", "traceparent", "tracestate", "x-trace-id", "x-b3-traceid",
]);

export interface HeaderRow {
  name: string;
  kind: "same" | "added" | "removed" | "changed" | "moved";
  /** Values in wire order on each side. */
  a: string[];
  b: string[];
  volatile: boolean;
}

/** Per header name, in wire order of first appearance (original first). */
export function headerDiff(a: Header[], b: Header[]): HeaderRow[] {
  const names: string[] = [];
  const values = (list: Header[]) => {
    const m = new Map<string, string[]>();
    for (const [k, v] of list) {
      const name = k.toLowerCase();
      if (!names.includes(name)) names.push(name);
      m.set(name, [...(m.get(name) ?? []), v]);
    }
    return m;
  };
  const va = values(a);
  const vb = values(b);
  return names.map((name) => {
    const x = va.get(name) ?? [];
    const y = vb.get(name) ?? [];
    const sorted = (s: string[]) => [...s].sort().join("\n");
    const kind: HeaderRow["kind"] = !y.length
      ? "removed"
      : !x.length
        ? "added"
        : x.join("\n") === y.join("\n")
          ? "same"
          : sorted(x) === sorted(y)
            ? "moved"
            : "changed";
    return { name, kind, a: x, b: y, volatile: VOLATILE.has(name) };
  });
}

export interface Comparison {
  verdict: string;
  status: { a: number | null; b: number | null };
  request: { headers: HeaderRow[]; body: BodyDiff };
  response: { headers: HeaderRow[]; body: BodyDiff };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Original `a`, new `b` (a replay, or later a retest). */
export function compareExchanges(a: TrafficEntry, b: TrafficEntry): Comparison {
  const reqType = headerValue(b.req_headers, "content-type") ?? headerValue(a.req_headers, "content-type");
  const resType = headerValue(b.res_headers, "content-type") ?? headerValue(a.res_headers, "content-type");
  const request = { headers: headerDiff(a.req_headers, b.req_headers), body: bodyDiff(a.req_body, a.req_body_encoding, b.req_body, b.req_body_encoding, reqType) };
  const response = { headers: headerDiff(a.res_headers, b.res_headers), body: bodyDiff(a.res_body, a.res_body_encoding, b.res_body, b.res_body_encoding, resType) };

  const parts: string[] = [];
  if (b.status === null) parts.push(`no response${b.error ? ` (${b.error})` : ""}`);
  else if (a.status === b.status) parts.push(`same status (${b.status})`);
  else parts.push(`status ${a.status ?? "none"} → ${b.status}`);
  const body = response.body;
  if (b.status !== null) {
    if (body.kind === "text") parts.push(body.changed ? `body differs (${plural(body.changed, "line")})` : "same body");
    else parts.push(body.same ? "same body" : `body differs (${body.sizeA} → ${body.sizeB} bytes)`);
    const changed = response.headers.filter((h) => h.kind !== "same");
    const volatile = changed.filter((h) => h.volatile).length;
    const real = changed.length - volatile;
    if (real) parts.push(`${plural(real, "header")} changed${volatile ? ` (${volatile} volatile)` : ""}`);
    else parts.push(`same headers${volatile ? ` (${volatile} volatile changed)` : ""}`);
  }
  return { verdict: parts.join(" · "), status: { a: a.status, b: b.status }, request, response };
}

export type View = Line | { kind: "skip"; count: number };

/** Collapse unchanged runs, keeping `context` lines around each change. */
export function collapse(lines: Line[], context = 3): View[] {
  const keep = lines.map((l) => l.kind !== "same");
  const near = lines.map((_, i) => {
    for (let d = -context; d <= context; d++) if (keep[i + d]) return true;
    return false;
  });
  const out: View[] = [];
  let skipped = 0;
  lines.forEach((l, i) => {
    if (near[i] || lines.length <= 2 * context + 1) {
      if (skipped) out.push({ kind: "skip", count: skipped });
      skipped = 0;
      out.push(l);
    } else skipped++;
  });
  if (skipped) out.push({ kind: "skip", count: skipped });
  return out;
}
