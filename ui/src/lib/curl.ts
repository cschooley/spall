// "Copy as curl". The result gets pasted into a shell, and every part of it
// is attacker-controlled, so quoting must be exact: POSIX single quotes,
// with ' written as '\''. Tested against a real shell.

import type { TrafficEntry } from "../types.ts";

export function shellQuote(s: string): string {
  return `'${s.replaceAll("'", `'\\''`)}'`;
}

/** Headers curl manages itself or that only made sense hop-by-hop. */
const SKIP = new Set(["content-length", "proxy-connection", "connection", "transfer-encoding", "keep-alive"]);

export function toCurl(e: TrafficEntry): string {
  const parts = ["curl"];
  if (e.method.toUpperCase() !== "GET") parts.push("-X", shellQuote(e.method));
  parts.push(shellQuote(e.url));
  for (const [name, value] of e.req_headers) {
    if (SKIP.has(name.toLowerCase())) continue;
    parts.push("-H", shellQuote(`${name}: ${value}`));
  }
  let prefix = "";
  if (e.req_body) {
    if (e.req_body_encoding === "base64") {
      prefix = `printf '%s' ${shellQuote(e.req_body)} | base64 -d | `;
      parts.push("--data-binary", "@-");
    } else {
      parts.push("--data-binary", shellQuote(e.req_body));
    }
  }
  return prefix + parts.join(" ");
}
