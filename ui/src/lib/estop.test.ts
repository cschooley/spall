import { test } from "node:test";
import assert from "node:assert/strict";
import { asEStop, clearEStop, engageEStop, sinceText } from "./estop.ts";

test("status: null when clear, fields as text when engaged", () => {
  assert.equal(asEStop(null), null);
  assert.equal(asEStop(undefined), null);
  assert.deepEqual(asEStop({ since: "2026-11-23T20:00:00Z", by: "human", reason: "<b>x</b>", extra: 1 }), {
    since: "2026-11-23T20:00:00Z",
    by: "human",
    reason: "<b>x</b>",
  });
  assert.deepEqual(asEStop({ reason: 5 }), { since: "", by: "", reason: "" });
  assert.equal(sinceText("unknown"), "unknown");
});

test("engage and clear post JSON and say what happened", async () => {
  const calls: { url: string; body: string | undefined; type: string | null }[] = [];
  const answers: [number, unknown][] = [
    [200, { estop: { since: "s", by: "human", reason: "r" }, halted: ["e/T-1", "e/T-2"], errors: [] }],
    [200, { estop: null, was: null }],
    [500, { error: "agents are stopped, but not everything was saved" }],
  ];
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    calls.push({ url, body: init?.body as string | undefined, type: new Headers(init?.headers).get("content-type") });
    const [status, body] = answers.shift()!;
    return new Response(JSON.stringify(body), { status });
  }) as typeof fetch;
  assert.deepEqual(await engageEStop("odd"), { ok: true, text: "e-stop engaged: 2 agent tasks halted for good. Human traffic is unaffected." });
  assert.equal((await clearEStop()).ok, true);
  assert.deepEqual(await engageEStop(), { ok: false, text: "e-stop: agents are stopped, but not everything was saved" });
  assert.deepEqual(calls.map((c) => [c.url, c.body, c.type]), [
    ["/api/estop", '{"reason":"odd"}', "application/json"],
    ["/api/estop/clear", "{}", "application/json"],
    ["/api/estop", "{}", "application/json"],
  ]);
});
