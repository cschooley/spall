import { test } from "node:test";
import assert from "node:assert/strict";
import { asWaiting, costText, decide, forTaskText, taskLines, taskListLines } from "./tasks.ts";

const P = `<img src=x onerror="window.__pwned=1">`;

function task(id: string, ask: unknown, over: Record<string, unknown> = {}) {
  return {
    id,
    goal: P,
    role: "mapper",
    status: "waiting_on_approval",
    spent: 3,
    constraints: { budget: 50, time_limit_minutes: 60 },
    pending: { method: "POST", target: `https://a.test/${P}`, host_header: null, reason: "r", asked: `2026-11-23T20:0${id.slice(-1)}:00Z`, ask },
    ...over,
  };
}

test("only waiting tasks with a readable ask, oldest ask first; text kept as text", () => {
  const list = asWaiting({
    tasks: [task("T-3", { kind: "budget" }), task("T-1", { kind: "confirm" }), task("T-2", { kind: "warp" }), task("T-4", { kind: "time" }, { status: "running" })],
  });
  assert.deepEqual(list.map((w) => w.id), ["T-1", "T-3"]);
  assert.equal(list[0]!.goal, P);
  assert.equal(list[0]!.pending.target, `https://a.test/${P}`);
  assert.deepEqual(asWaiting(null), []);
  assert.deepEqual(asWaiting({ tasks: "no" }), []);
});

test("what approving for the task would change; an RoE confirmation is once only", () => {
  const w = (ask: unknown) => asWaiting({ tasks: [task("T-1", ask)] })[0]!;
  assert.equal(forTaskText(w({ kind: "host", host: "v2.a.test" })), "add v2.a.test to T-1's hosts");
  assert.equal(forTaskText(w({ kind: "method", method: "PUT" })), "let T-1 send PUT");
  assert.equal(forTaskText(w({ kind: "unsafe", method: "POST" })), "let T-1 send POST without asking");
  assert.equal(forTaskText(w({ kind: "time" })), "time limit 60 → 120 min");
  assert.equal(forTaskText(w({ kind: "budget" })), "budget 50 → 100 requests");
  assert.equal(forTaskText(w({ kind: "confirm" })), null);
  assert.equal(costText(w({ kind: "confirm" })), "1 request · 3 of 50 spent");
});

test("decide sends the op; the server's refusal comes back as text", async () => {
  const sent: [string, string | undefined][] = [];
  const answers: [number, unknown][] = [
    [200, { id: "T-1" }],
    [400, { error: "deny: say why" }],
  ];
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    sent.push([url, init?.body as string | undefined]);
    const [status, body] = answers.shift()!;
    return new Response(JSON.stringify(body), { status });
  }) as typeof fetch;
  assert.equal(await decide("T-1", { op: "approve_once" }), null);
  assert.equal(await decide("T/1", { op: "deny", reason: "" }), "deny: say why");
  assert.deepEqual(sent, [
    ["/api/tasks/T-1", '{"op":"approve_once"}'],
    ["/api/tasks/T%2F1", '{"op":"deny","reason":""}'],
  ]);
});

test("console lines for tasks", () => {
  assert.deepEqual(taskListLines({ tasks: [] }), ["no tasks in this engagement"]);
  assert.deepEqual(taskListLines({ tasks: [{ id: "T-1", status: "running", role: "mapper", spent: 2, constraints: { budget: 10 }, goal: P }] }), [
    `T-1    running             mapper   2/10  ${P}`,
  ]);
  const lines = taskLines({
    id: "T-2", status: "waiting_on_approval", role: "tester", asked_by: "human", goal: "g", spent: 1, note: "my boss said",
    constraints: { budget: 5, time_limit_minutes: 60, hosts: [], methods: ["GET"], safe_methods: [] },
    pending: { method: "POST", target: "https://a.test/", reason: "asks" }, log: [{}, {}],
  });
  assert.deepEqual(lines, [
    "T-2 · waiting_on_approval · tester · asked by human",
    "goal: g",
    "spent 1 of 5 requests · time limit 60 min",
    "hosts: the engagement's scope · methods: GET · without asking: none",
    "note: my boss said",
    "waiting on approval: POST https://a.test/ (asks)",
    "log: 2 entries",
  ]);
});
