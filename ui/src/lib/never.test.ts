import { test } from "node:test";
import assert from "node:assert/strict";
import { asNever, SUGGESTED } from "./never.ts";

test("the never-test list is read from the server's answer", () => {
  assert.deepEqual(asNever({ entries: ["*.gov", 3], exceptions: ["*.gov"] }), { entries: ["*.gov"], exceptions: ["*.gov"] });
  assert.deepEqual(asNever(null), { entries: [], exceptions: [] });
  assert.equal(asNever("spall unreachable"), "spall unreachable");
  assert.deepEqual(SUGGESTED, ["*.gov", "*.mil"], "offered, not on by default");
});
