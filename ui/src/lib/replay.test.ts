import { test } from "node:test";
import assert from "node:assert/strict";
import { ARM_MS, armLabel, armedFor, postReplay, readReplayResponse, type Armed } from "./replay.ts";
import type { TrafficEntry } from "../types.ts";

const entry: TrafficEntry = {
  schema_version: 5, id: "r1", ts: "2026-12-25T10:00:00Z", host: "h", method: "GET",
  url: "https://h.example/a", req_headers: [], req_body: null, req_body_encoding: "utf8",
  status: 200, res_headers: [], res_body: null, res_body_encoding: "utf8", duration_ms: 1, replay_of: "o1",
};

test("each /api/replay answer becomes one kind of result", () => {
  assert.deepEqual(readReplayResponse(200, entry), { kind: "sent", entry });
  const failed = { ...entry, status: null, error: "connect error" };
  assert.deepEqual(readReplayResponse(502, failed), { kind: "failed", entry: failed });
  assert.deepEqual(
    readReplayResponse(428, { decision: "needs_confirmation", error: "DELETE can change state", confirm: "abc123", expires_in_secs: 120 }),
    { kind: "confirm", token: "abc123", reason: "DELETE can change state" },
  );
  assert.deepEqual(readReplayResponse(403, { decision: "deny", error: '"x.test" is not in scope' }), {
    kind: "denied",
    reason: '"x.test" is not in scope',
  });
  assert.deepEqual(readReplayResponse(429, { decision: "rate_limited", error: "the rules of engagement allow 1 sends per second; retry in 0.4s" }), {
    kind: "denied",
    reason: "the rules of engagement allow 1 sends per second; retry in 0.4s",
  });
  assert.deepEqual(readReplayResponse(409, { error: "confirmation expired" }), { kind: "refused", reason: "confirmation expired" });
  assert.deepEqual(readReplayResponse(404, { error: "no such exchange" }), { kind: "error", reason: "no such exchange" });
  assert.deepEqual(readReplayResponse(503, { error: "replay needs taphttp's lever API" }), {
    kind: "error",
    reason: "replay needs taphttp's lever API",
  });
});

test("malformed answers are errors, never a send", () => {
  assert.equal(readReplayResponse(200, { nope: 1 }).kind, "error", "200 without an entry");
  assert.equal(readReplayResponse(502, { error: "taphttp API unavailable" }).kind, "error", "502 without an entry");
  assert.equal(readReplayResponse(428, { error: "x" }).kind, "error", "428 without a token");
  assert.equal(readReplayResponse(428, { confirm: 5 }).kind, "error");
  assert.deepEqual(readReplayResponse(500, "oops"), { kind: "error", reason: "replay failed (HTTP 500)" });
  assert.deepEqual(readReplayResponse(403, null), { kind: "denied", reason: "replay failed (HTTP 403)" });
});

test("postReplay sends same-origin JSON and survives a network failure", async () => {
  const calls: { url: string; init: RequestInit }[] = [];
  const ok = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(JSON.stringify(entry), { status: 200 });
  }) as typeof fetch;
  assert.deepEqual(await postReplay({ id: "o1", confirm: "t" }, ok), { kind: "sent", entry });
  assert.equal(calls[0]!.url, "/api/replay");
  assert.equal(calls[0]!.init.method, "POST");
  assert.equal(new Headers(calls[0]!.init.headers).get("content-type"), "application/json");
  assert.deepEqual(JSON.parse(String(calls[0]!.init.body)), { id: "o1", confirm: "t" });

  const down = (async () => {
    throw new TypeError("Failed to fetch");
  }) as typeof fetch;
  assert.deepEqual(await postReplay({ id: "o1" }, down), { kind: "error", reason: "spall unreachable" });
  const notJson = (async () => new Response("<html>", { status: 502 })) as typeof fetch;
  assert.equal((await postReplay({ id: "o1" }, notJson)).kind, "error");
});

test("arming is for one exchange and a few seconds", () => {
  const armed: Armed = { id: "o1", token: "t", label: "x", until: 1000 + ARM_MS };
  assert.equal(armedFor(armed, "o1", 1000), armed);
  assert.equal(armedFor(armed, "o1", 1000 + ARM_MS - 1), armed);
  assert.equal(armedFor(armed, "o1", 1000 + ARM_MS), null, "expired");
  assert.equal(armedFor(armed, "o2", 1000), null, "another exchange");
  assert.equal(armedFor(null, "o1", 1000), null);
  assert.ok(ARM_MS >= 3000 && ARM_MS <= 10_000);
});

test("the armed label names the method and host", () => {
  assert.equal(armLabel("DELETE", "shop.example.test"), "press again to send DELETE to shop.example.test");
  assert.equal(armLabel("POST", null), "press again to send POST");
});
