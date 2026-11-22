import { test } from "node:test";
import assert from "node:assert/strict";
import { asEngagement, asSummaries, testingOf } from "./engagement.ts";
import { DEFAULT_RULES } from "./roe.ts";

test("engagements from the API are shape-checked; junk is dropped, not trusted", () => {
  assert.deepEqual(
    asEngagement({ id: "a-1", name: "A", client: 7, starts: "2026-11-23", ends: "", scope: ["a.test", 3, "!b.test"], extra: "<b>" }),
    { id: "a-1", name: "A", client: "", starts: "2026-11-23", ends: null, expires: null, scope: ["a.test", "!b.test"], notes: "", rules: DEFAULT_RULES, testingNow: null },
  );
  for (const bad of [null, "x", [], { id: 1, name: "x" }, { id: "x" }]) assert.equal(asEngagement(bad), null, JSON.stringify(bad));
  assert.deepEqual(asSummaries([{ id: "a", name: "A", active: true }, { nope: 1 }, { id: "b", name: "B", client: "C" }]), [
    { id: "a", name: "A", client: "", active: true },
    { id: "b", name: "B", client: "C", active: false },
  ]);
  assert.equal(asSummaries({}), null);
});

test("testing-now and rules from the API", () => {
  assert.deepEqual(testingOf({ allowed: false, reason: "in a no-test window" }), { allowed: false, reason: "in a no-test window" });
  assert.equal(testingOf({ allowed: "yes" }), null);
  const e = asEngagement({ id: "a", name: "A", rules: { rate_limit: null, windows: [{ days: ["mon", "funday"], start: "09:00", end: "17:00" }, { nope: 1 }] } });
  assert.equal(e?.rules.rate_limit, null);
  assert.deepEqual(e?.rules.windows, [{ days: ["mon"], start: "09:00", end: "17:00" }]);
  assert.deepEqual(e?.rules.safe_methods, DEFAULT_RULES.safe_methods, "missing fields take the defaults");
});
