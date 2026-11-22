import { test } from "node:test";
import assert from "node:assert/strict";
import { chainOf, editSummary, replaysOf } from "./replays.ts";
import type { TrafficEntry } from "../types.ts";

const base: TrafficEntry = {
  schema_version: 5, id: "o", ts: "2026-11-21T10:00:00Z", host: "a.test", method: "POST", url: "https://a.test/x",
  req_headers: [["host", "a.test"], ["content-type", "application/json"]], req_body: "{}", req_body_encoding: "utf8",
  status: 200, res_headers: [], res_body: null, res_body_encoding: "utf8", duration_ms: 1, replay_of: null,
};
const e = (id: string, over: Partial<TrafficEntry> = {}): TrafficEntry => ({ ...base, id, ...over });

test("replays of an exchange: from every source, once each, newest first", () => {
  const r1 = e("r1", { replay_of: "o", ts: "2026-11-21T10:01:00Z" });
  const r2 = e("r2", { replay_of: "o", ts: "2026-11-21T10:02:00.5Z" });
  const other = e("x", { replay_of: "zzz" });
  const got = replaysOf("o", [[r1, other, e("o")], [r2, r1]]);
  assert.deepEqual(got.map((r) => r.id), ["r2", "r1"]);
  assert.deepEqual(replaysOf("nope", [[r1]]), []);
});

test("a chain walks back to the first original, and stops at gaps and loops", () => {
  const all = new Map([e("o"), e("r1", { replay_of: "o" }), e("r2", { replay_of: "r1" })].map((x) => [x.id, x]));
  const look = (id: string) => all.get(id);
  assert.deepEqual(chainOf(all.get("r2")!, look), { chain: ["o", "r1", "r2"], missing: null });
  assert.deepEqual(chainOf(all.get("o")!, look), { chain: ["o"], missing: null });
  assert.deepEqual(chainOf(e("r9", { replay_of: "gone" }), look), { chain: ["r9"], missing: "gone" });
  const loop = new Map([e("a", { replay_of: "b" }), e("b", { replay_of: "a" })].map((x) => [x.id, x]));
  assert.deepEqual(chainOf(loop.get("a")!, (id) => loop.get(id)).chain, ["b", "a"], "a loop stops, no hang");
});

test("what a replay changed, ignoring headers the client adds on send", () => {
  const o = e("o");
  assert.equal(editSummary(o, e("r", { replay_of: "o" })), "as captured");
  // hyper adds Content-Length (and Host when absent); not an edit.
  assert.equal(editSummary(o, e("r", { req_headers: [...o.req_headers, ["content-length", "2"]] })), "as captured");
  assert.equal(editSummary(o, e("r", { method: "PUT" })), "method PUT");
  assert.equal(
    editSummary(o, e("r", { req_headers: [["host", "a.test"], ["content-type", "text/plain"], ["x-a", "1"]], req_body: "x" })),
    "2 headers · body",
  );
  assert.equal(editSummary(o, e("r", { req_headers: [["host", "a.test"]] })), "1 header");
  assert.equal(editSummary(null, e("r")), "", "original unknown: say nothing");
});
