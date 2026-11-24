import { test } from "node:test";
import assert from "node:assert/strict";
import { asQuad, asScorecard, mmss, remaining, scoreLines } from "./quad.ts";

test("a burst and its countdown are read from the status", () => {
  const q = asQuad({ until: "2026-11-24T10:02:00Z", seconds: 120, workers: 8, capped: null, started: "x" });
  assert.deepEqual(q, { until: "2026-11-24T10:02:00Z", seconds: 120, workers: 8, capped: null });
  assert.equal(remaining(q!, Date.parse("2026-11-24T10:00:30Z")), 90);
  assert.equal(remaining(q!, Date.parse("2026-11-24T10:05:00Z")), 0);
  assert.equal(asQuad(null), null);
  assert.equal(asQuad({ until: 5 }), null);
  assert.equal(mmss(125), "2:05");
  assert.equal(mmss(-3), "0:00");
});

test("a scorecard is numbers and the server's card", () => {
  const c = asScorecard({ seconds: 45, workers: 8, requests: 137, tokens: 412000, candidates: 3, ended: "estop", card: "QUAD 0:45 · 8× agents" });
  assert.ok(c);
  assert.deepEqual(scoreLines(c), ["quad over (e-stop): 0:45, 8 workers", "  137 requests · 412000 tokens · 3 candidates", "  card: QUAD 0:45 · 8× agents"]);
  assert.equal(asScorecard({ seconds: "45" }), null);
  assert.equal(asScorecard({ seconds: 1, workers: 8, requests: 1, tokens: 1, candidates: 1, ended: "boom", card: "" }), null);
});
