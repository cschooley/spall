import { test } from "node:test";
import assert from "node:assert/strict";
import { asAnswer, asPlan } from "./intake.ts";

const RAW = {
  id: "P-3",
  engagement: "evilcorp-1a2b",
  request: "My boss said to re-scan the findings",
  status: "draft",
  summary: "Two retests.",
  tasks: [{ task: { role: "tester", goal: "Retest candidate c-1", constraints: {} }, cost: "~5 requests, safe methods", requests: 5, candidate: "c-1" }],
  warnings: ["example.com resolves to 203.0.113.50 now"],
  choice: { question: "Which?", options: ["all", "one"] },
  proposal: { kind: "extension", engagement: "evilcorp-1a2b", ends: null, why: "the engagement ended" },
  estimate: "1 task, ~5 requests, safe methods only",
  made: [],
};

test("a plan is read from the server's answer", () => {
  const p = asPlan(RAW);
  assert.ok(p);
  assert.deepEqual(p.tasks, [{ role: "tester", goal: "Retest candidate c-1", cost: "~5 requests, safe methods", candidate: "c-1" }]);
  assert.deepEqual(p.choice, { question: "Which?", options: ["all", "one"] });
  assert.deepEqual(p.proposal, { kind: "extension", engagement: "evilcorp-1a2b", why: "the engagement ended" });
  assert.equal(p.estimate, "1 task, ~5 requests, safe methods only");
  assert.equal(asPlan({ id: 3 }), null);
});

test("an answer is no, words, a plan, or the error passed on", () => {
  assert.deepEqual(asAnswer({ kind: "no" }), { kind: "no" });
  assert.deepEqual(asAnswer({ kind: "said", text: "Nothing to plan." }), { kind: "said", text: "Nothing to plan." });
  assert.equal((asAnswer({ kind: "plan", plan: RAW }) as { kind: string }).kind, "plan");
  assert.equal(asAnswer("the intake needs spall's model"), "the intake needs spall's model");
  assert.equal(asAnswer({ kind: "plan", plan: {} }), "unexpected answer from spall");
});
