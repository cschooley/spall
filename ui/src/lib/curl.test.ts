import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { shellQuote, toCurl } from "./curl.ts";
import type { TrafficEntry } from "../types.ts";

const base: TrafficEntry = {
  schema_version: 3, id: "x", ts: "2026-12-25T10:00:00Z", host: "h", method: "GET",
  url: "https://h.example/a?b=1", req_headers: [], req_body: null, req_body_encoding: "utf8",
  status: 200, res_headers: [], res_body: null, res_body_encoding: "utf8", duration_ms: 1, replay_of: null,
};

/** What a POSIX shell actually parses the command into (the real test of quoting). */
function argv(cmd: string): string[] {
  const out = execFileSync("sh", ["-c", `set -- ${cmd.replace(/^(.*\| )?curl /, "")}; for a; do printf '%s\\0' "$a"; done`]);
  return out.toString("utf8").split("\0").slice(0, -1);
}

test("shellQuote survives hostile strings through a real shell", () => {
  for (const s of ["plain", "it's", "a'b'c", "$(touch /tmp/pwn)", "`id`", "a\\nb", "semi;colon", "", "'"]) {
    const out = execFileSync("sh", ["-c", `printf '%s' ${shellQuote(s)}`]).toString("utf8");
    assert.equal(out, s, s);
  }
});

test("GET: url and headers in wire order, repeats kept, hop-by-hop dropped", () => {
  const cmd = toCurl({
    ...base,
    req_headers: [["host", "h.example"], ["cookie", "a=1"], ["cookie", "b=2"], ["proxy-connection", "keep-alive"], ["content-length", "0"]],
  });
  assert.deepEqual(argv(cmd), ["https://h.example/a?b=1", "-H", "host: h.example", "-H", "cookie: a=1", "-H", "cookie: b=2"]);
});

test("POST with a hostile text body", () => {
  const body = `{"q":"'; rm -rf / #"} $(id)`;
  const cmd = toCurl({ ...base, method: "POST", req_body: body, req_headers: [["content-type", "application/json"]] });
  assert.deepEqual(argv(cmd), ["-X", "POST", "https://h.example/a?b=1", "-H", "content-type: application/json", "--data-binary", body]);
});

test("binary bodies go through base64 -d on stdin", () => {
  const cmd = toCurl({ ...base, method: "PUT", req_body: "AAEC/w==", req_body_encoding: "base64" });
  assert.match(cmd, /^printf '%s' 'AAEC\/w==' \| base64 -d \| curl /);
  assert.deepEqual(argv(cmd), ["-X", "PUT", "https://h.example/a?b=1", "--data-binary", "@-"]);
});
