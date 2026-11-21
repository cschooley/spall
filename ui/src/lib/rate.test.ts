import { test } from "node:test";
import assert from "node:assert/strict";
import { litSegments, requestRate } from "./rate.ts";

test("requests per second over a sliding window", () => {
  const now = 100_000;
  assert.equal(requestRate([], now), 0);
  assert.equal(requestRate([now - 1000, now - 2000, now - 3000, now - 4000], now, 10_000), 0.4);
  assert.equal(requestRate([now - 20_000, now - 500], now, 10_000), 0.1, "old ones ignored");
});

test("meter lights one segment per req/s, up to 10, and at least one when busy", () => {
  assert.equal(litSegments(0), 0);
  assert.equal(litSegments(0.4), 1);
  assert.equal(litSegments(3.2), 4);
  assert.equal(litSegments(50), 10);
});
