import { test } from "node:test";
import assert from "node:assert/strict";
import { asEngagement, asSummaries } from "./engagement.ts";

test("engagements from the API are shape-checked; junk is dropped, not trusted", () => {
  assert.deepEqual(
    asEngagement({ id: "a-1", name: "A", client: 7, starts: "2026-11-23", ends: "", scope: ["a.test", 3, "!b.test"], extra: "<b>" }),
    { id: "a-1", name: "A", client: "", starts: "2026-11-23", ends: null, expires: null, scope: ["a.test", "!b.test"], notes: "" },
  );
  for (const bad of [null, "x", [], { id: 1, name: "x" }, { id: "x" }]) assert.equal(asEngagement(bad), null, JSON.stringify(bad));
  assert.deepEqual(asSummaries([{ id: "a", name: "A", active: true }, { nope: 1 }, { id: "b", name: "B", client: "C" }]), [
    { id: "a", name: "A", client: "", active: true },
    { id: "b", name: "B", client: "C", active: false },
  ]);
  assert.equal(asSummaries({}), null);
});
