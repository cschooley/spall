import { test } from "node:test";
import assert from "node:assert/strict";
import { asChecks, asSweep, sweepLines } from "./sweep.ts";

test("an estimate, a summary and the checks are read from the server", () => {
  assert.deepEqual(asSweep({ check: "metafiles", rule: "WSTG-INFO-03", planned: 12, cost: "12 GET requests (one is the soft-404 baseline)" }), {
    kind: "estimate",
    check: "metafiles",
    rule: "WSTG-INFO-03",
    planned: 12,
    cost: "12 GET requests (one is the soft-404 baseline)",
  });
  const done = asSweep({ check: "admin-interfaces", rule: "WSTG-CONF-05", base: "https://a.test", planned: 3, sent: 3, hits: [{ path: "/admin", exchange: "s-2", status: 401, size: 14 }], misses: 1, skipped: [], stopped: null });
  assert.ok(typeof done !== "string" && done.kind === "done");
  assert.deepEqual(sweepLines(done as never), [
    "sweep admin-interfaces on https://a.test: 3 of 3 sent, 1 hit(s), 1 miss(es)",
    "  hit /admin  401  14 B  open s-2 · flag s-2 --rule WSTG-CONF-05",
  ]);
  assert.equal(asSweep("no check"), "no check");
  assert.deepEqual(asChecks({ checks: [{ name: "mine", rule: "WSTG-CONF-04", title: "t", method: "GET", paths: ["/a", "/b"], source: "engagement" }] }), [
    { name: "mine", rule: "WSTG-CONF-04", title: "t", method: "GET", paths: 2, source: "engagement" },
  ]);
});
