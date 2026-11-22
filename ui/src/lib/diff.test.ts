import { test } from "node:test";
import assert from "node:assert/strict";
import { bodyDiff, collapse, compareExchanges, headerDiff, lineDiff, normalizedText, VOLATILE } from "./diff.ts";
import type { TrafficEntry } from "../types.ts";

const base: TrafficEntry = {
  schema_version: 5, id: "o", ts: "2026-11-21T10:00:00Z", host: "a.test", method: "POST", url: "https://a.test/login",
  req_headers: [["host", "a.test"], ["content-type", "application/json"]], req_body: '{"user":"admin"}', req_body_encoding: "utf8",
  status: 401, res_headers: [["content-type", "application/json"], ["date", "Sat, 21 Nov 2026 10:00:00 GMT"], ["set-cookie", "a=1"], ["set-cookie", "b=2"]],
  res_body: '{"error":"bad password","code":17}', res_body_encoding: "utf8", duration_ms: 184, replay_of: null,
};
const e = (over: Partial<TrafficEntry>): TrafficEntry => ({ ...base, id: "r", replay_of: "o", ...over });

test("a line diff keeps order and marks adds and deletes", () => {
  assert.deepEqual(lineDiff(["a", "b", "c"], ["a", "x", "c"]), [
    { kind: "same", text: "a" },
    { kind: "del", text: "b" },
    { kind: "add", text: "x" },
    { kind: "same", text: "c" },
  ]);
  assert.deepEqual(lineDiff([], ["a"]), [{ kind: "add", text: "a" }]);
  assert.deepEqual(lineDiff(["a"], []), [{ kind: "del", text: "a" }]);
  assert.deepEqual(lineDiff([], []), []);
});

test("JSON is compared pretty-printed with sorted keys, so order and spacing aren't differences", () => {
  assert.equal(normalizedText('{"b":1,"a":{"d":2,"c":3}}', "utf8", "application/json"), normalizedText('{ "a": {"c":3, "d":2}, "b":1 }', "utf8", "application/json"));
  assert.equal(normalizedText('{"a":1}', "utf8", "application/json"), '{\n  "a": 1\n}');
  assert.equal(normalizedText("not json {", "utf8", "application/json"), "not json {", "invalid JSON compared as text");
  assert.equal(normalizedText('{"a":1}', "utf8", "text/plain"), '{"a":1}', "only JSON content types are reformatted");
  assert.equal(normalizedText(null, "utf8", null), "");
});

test("body diff: text by lines, binary by bytes and size, big bodies capped", () => {
  const d = bodyDiff('{"error":"bad password","code":17}', "utf8", '{"code":17,"error":"ok"}', "utf8", "application/json");
  assert.equal(d.kind, "text");
  assert.ok(d.kind === "text" && d.changed === 2, JSON.stringify(d)); // one line removed, one added
  assert.deepEqual(bodyDiff("aGk=", "base64", "aGk=", "base64", null), { kind: "binary", same: true, sizeA: 2, sizeB: 2 });
  assert.deepEqual(bodyDiff("aGk=", "base64", "aGkh", "base64", null), { kind: "binary", same: false, sizeA: 2, sizeB: 3 });
  assert.deepEqual(bodyDiff("hi", "utf8", "aGk=", "base64", null).kind, "binary", "text vs binary compares bytes");
  const big = Array.from({ length: 6000 }, (_, i) => `line ${i}`).join("\n");
  const capped = bodyDiff(big, "utf8", `${big}\nmore`, "utf8", "text/plain");
  assert.equal(capped.kind, "too-large");
  const same = bodyDiff("same", "utf8", "same", "utf8", null);
  assert.ok(same.kind === "text" && same.changed === 0);
  assert.deepEqual(bodyDiff(null, "utf8", null, "utf8", null), { kind: "text", lines: [], changed: 0 });
});

test("header diff in wire order: added, removed, changed, moved; volatile ones marked", () => {
  const rows = headerDiff(
    [["content-type", "application/json"], ["date", "d1"], ["set-cookie", "a=1"], ["set-cookie", "b=2"], ["x-old", "1"]],
    [["content-type", "text/html"], ["date", "d2"], ["set-cookie", "b=2"], ["set-cookie", "a=1"], ["x-new", "1"]],
  );
  assert.deepEqual(
    rows.map((r) => [r.name, r.kind, r.volatile]),
    [
      ["content-type", "changed", false],
      ["date", "changed", true],
      ["set-cookie", "moved", false],
      ["x-old", "removed", false],
      ["x-new", "added", false],
    ],
  );
  assert.deepEqual(rows[0]!.a, ["application/json"]);
  assert.deepEqual(rows[0]!.b, ["text/html"]);
  assert.deepEqual(headerDiff([["A", "1"]], [["a", "1"]]).map((r) => r.kind), ["same"], "names are case-insensitive");
  for (const v of ["date", "etag", "x-request-id", "age"]) assert.ok(VOLATILE.has(v), v);
  assert.ok(!VOLATILE.has("set-cookie"), "session cookies are a real signal");
});

test("the verdict reads like a sentence and ignores volatile headers", () => {
  const same = compareExchanges(base, e({ res_headers: base.res_headers.map(([k, v]) => [k, k === "date" ? "later" : v] as [string, string]) }));
  assert.equal(same.verdict, "same status (401) · same body · same headers (1 volatile changed)");
  const diff = compareExchanges(
    base,
    e({ status: 200, res_body: '{"code":17,"error":"ok"}', res_headers: [["content-type", "application/json"], ["date", "x"], ["set-cookie", "a=1"]] }),
  );
  assert.equal(diff.verdict, "status 401 → 200 · body differs (2 lines) · 1 header changed (1 volatile)");
  const failed = compareExchanges(base, e({ status: null, error: "connect error", res_body: null, res_headers: [] }));
  assert.match(failed.verdict, /^no response \(connect error\)/);
});

test("long unchanged runs collapse, keeping context around changes", () => {
  const lines = lineDiff(
    Array.from({ length: 20 }, (_, i) => `l${i}`),
    Array.from({ length: 20 }, (_, i) => (i === 10 ? "changed" : `l${i}`)),
  );
  const view = collapse(lines, 2);
  assert.deepEqual(
    view.map((v) => (v.kind === "skip" ? `skip ${v.count}` : `${v.kind} ${v.text}`)),
    ["skip 8", "same l8", "same l9", "del l10", "add changed", "same l11", "same l12", "skip 7"],
  );
  assert.deepEqual(collapse(lineDiff(["a"], ["a"]), 2), [{ kind: "same", text: "a" }], "short runs stay");
});
