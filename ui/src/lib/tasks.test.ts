import { test } from "node:test";
import assert from "node:assert/strict";
import { asTask, asTasks, asWaiting, chipOf, costText, createTask, decide, forTaskText, messages, requestRows, taskFromHash, taskLines, taskListLines, usageText } from "./tasks.ts";

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

const FULL = {
  id: "T-3", goal: P, role: "tester", status: "running", reason: null, spent: 4, asked_by: "human", note: "", created: "2026-11-23T20:00:00Z", started: "2026-11-23T20:00:05Z", ended: null,
  constraints: { budget: 30, time_limit_minutes: 60, hosts: ["a.test"], methods: ["GET", "POST"], safe_methods: ["GET"], rate_limit: 2, stop: { server_errors: 3 } },
  plan: [{ text: "baseline", done: true }, { text: "fixation", done: false }],
  log: [
    { ts: "2026-11-23T20:00:05Z", kind: "status", from: "approved", to: "running" },
    { ts: "2026-11-23T20:00:06Z", kind: "read", exchange: "e1" },
    { ts: "2026-11-23T20:00:07Z", kind: "request", exchange: "r-e1", method: "GET", target: `https://a.test/${P}`, outcome: "sent", status: 200 },
    { ts: "2026-11-23T20:00:08Z", kind: "request", exchange: null, method: "POST", target: "https://a.test/login", outcome: "denied" },
    { ts: "2026-11-23T20:00:09Z", kind: "note", by: "human", text: "skip reset" },
    { ts: "2026-11-23T20:00:10Z", kind: "turn", text: P },
    { ts: "2026-11-23T20:00:11Z", kind: "approval", what: "POST https://a.test/login", outcome: "asked" },
    { ts: "2026-11-23T20:00:12Z", kind: "sandbox", argv: ["jq", ".alg", P], exit: 0, ms: 12, outcome: "exit 0" },
  ],
};

test("a full task: constraints, plan, request rows and messages", () => {
  const t = asTask(FULL)!;
  assert.deepEqual([t.budget, t.timeLimit, t.rate, t.stop.serverErrors, t.hosts, t.safeMethods], [30, 60, 2, 3, ["a.test"], ["GET"]]);
  assert.deepEqual(t.plan, [{ text: "baseline", done: true }, { text: "fixation", done: false }]);
  assert.deepEqual(requestRows(t), [
    { time: "20:00:07", method: "GET", target: `https://a.test/${P}`, status: 200, outcome: "sent", exchange: "r-e1", check: null },
    { time: "20:00:08", method: "POST", target: "https://a.test/login", status: null, outcome: "denied", exchange: null, check: null },
  ]);
  assert.deepEqual(messages(t), [
    { time: "20:00:05", who: "sys", text: "approved → running" },
    { time: "20:00:09", who: "you", text: "skip reset" },
    { time: "20:00:10", who: "agent", text: P },
    { time: "20:00:11", who: "sys", text: "asked: POST https://a.test/login" },
    { time: "20:00:12", who: "sys", text: `sandbox $ jq .alg ${P} → exit 0 (12 ms)` },
  ]);
  assert.equal(asTask({ goal: "no id" }), null);
  assert.deepEqual(asTasks({ tasks: [FULL, null, { id: 5 }] }).map((x) => x.id), ["T-3"]);
  assert.equal(asTasks({ tasks: [{ ...FULL, log: undefined, log_len: 9 }] })[0]!.logLen, 9);
});

test("chips and hashes", () => {
  assert.deepEqual(chipOf("waiting_on_approval"), { label: "Needs approval", cls: "waiting" });
  assert.deepEqual(chipOf("failed"), { label: "Failed", cls: "halted" });
  assert.equal(taskFromHash("#tasks/T-12"), "T-12");
  assert.equal(taskFromHash("#tasks/../x"), null);
  assert.equal(taskFromHash("#tasks"), null);
});

test("who runs a task, and what its model has used (#63)", async () => {
  const t = asTask({ ...FULL, runner: "spall", constraints: { ...FULL.constraints, tokens: 2000000 }, usage: { calls: 12, input_tokens: 182340, output_tokens: 9120, model_ms: 95400 } })!;
  assert.equal(t.runner, "spall");
  assert.deepEqual(t.usage, { calls: 12, inputTokens: 182340, outputTokens: 9120, modelMs: 95400, cap: 2000000 });
  assert.equal(usageText(t.usage), "12 model calls · 191k of 2.0M tokens (182k in, 9.1k out) · 1m 35s");
  const old = asTask(FULL)!;
  assert.equal(old.runner, "external", "older tasks were all run by an outside client");
  assert.equal(usageText(old.usage), "no model calls yet");
  assert.equal(asTask({ ...FULL, runner: "<b>x</b>" })!.runner, "external");

  const sent: string[] = [];
  globalThis.fetch = (async (_url: string, init?: RequestInit) => {
    sent.push(init?.body as string);
    return new Response(JSON.stringify(FULL), { status: 201 });
  }) as typeof fetch;
  await createTask({ goal: "g", role: "tester", runner: "spall", tokens: 50000 });
  await createTask({ goal: "g", role: "tester" });
  assert.deepEqual(JSON.parse(sent[0]!), { goal: "g", role: "tester", runner: "spall", constraints: { tokens: 50000 }, note: "" });
  assert.deepEqual(JSON.parse(sent[1]!), { goal: "g", role: "tester", constraints: {}, note: "" });
  assert.ok(taskLines({ ...FULL, runner: "spall", usage: { calls: 1, input_tokens: 10, output_tokens: 2, model_ms: 900 } }).some((l) => l === "run by spall · 1 model call · 12 of 2.0M tokens (10 in, 2 out) · 0s"));
});

test("an agent's report, if it filed one (#81)", () => {
  const t = asTask({ ...FULL, report: { text: "Stack: Apache\nNext: /cgi-bin/", at: "2026-11-24T18:10:00Z" } })!;
  assert.deepEqual(t.report, { text: "Stack: Apache\nNext: /cgi-bin/", at: "2026-11-24T18:10:00Z" });
  assert.equal(asTask(FULL)!.report, null);
  assert.equal(asTask({ ...FULL, report: { text: 5, at: "x" } })!.report, null);
  assert.equal(asTask({ ...FULL, report: "text" })!.report, null);
});
