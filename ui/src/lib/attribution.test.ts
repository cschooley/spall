import { test } from "node:test";
import assert from "node:assert/strict";
import { ofCandidate, ofExchange, taskHref } from "./attribution.ts";

test("exchanges: agent under a task, or you", () => {
  assert.deepEqual(ofExchange({ actor: "agent", task: "T-3" }), { agent: true, task: "T-3", label: "agent · T-3" });
  assert.deepEqual(ofExchange({ actor: "agent", task: "<img src=x>" }), { agent: true, task: null, label: "agent" });
  assert.deepEqual(ofExchange({ actor: "human" }), { agent: false, task: null, label: "you" });
  assert.deepEqual(ofExchange(undefined), { agent: false, task: null, label: "you" });
  assert.deepEqual(ofExchange({}), { agent: false, task: null, label: "you" });
});

test("candidates: agent:T-n proposed, human flagged", () => {
  assert.deepEqual(ofCandidate("agent:T-12"), { agent: true, task: "T-12", label: "agent · T-12" });
  assert.equal(ofCandidate("agent:bogus").label, "agent");
  assert.equal(ofCandidate("human").label, "you");
  assert.equal(taskHref("T-1"), "#tasks/T-1");
});

test("audit records as activity lines, yours and the agent's", async () => {
  const { activityOf } = await import("./attribution.ts");
  assert.deepEqual(activityOf({ ts: "2026-11-23T20:01:02Z", actor: "agent", task: "T-2", action: "replay", outcome: "paused", method: "POST", target: "https://a.test/x", reason: "needs approval" }), {
    time: "20:01:02",
    who: { agent: true, task: "T-2", label: "agent · T-2" },
    what: "replay paused · POST https://a.test/x · needs approval",
  });
  assert.deepEqual(activityOf({ ts: "x", actor: "human", action: "estop", outcome: "e_stop", reason: "engaged: odd" }), {
    time: "--:--:--",
    who: { agent: false, task: null, label: "you" },
    what: "estop e stop · engaged: odd",
  });
});
