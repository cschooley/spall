import { test } from "node:test";
import assert from "node:assert/strict";
import { asEngagement, asSummaries, testingOf } from "./engagement.ts";
import { DEFAULT_RULES } from "./roe.ts";

test("engagements from the API are shape-checked; junk is dropped, not trusted", () => {
  assert.deepEqual(
    asEngagement({ id: "a-1", name: "A", client: 7, starts: "2026-11-23", ends: "", scope: ["a.test", 3, "!b.test"], extra: "<b>" }),
    { id: "a-1", name: "A", client: "", starts: "2026-11-23", ends: null, expires: null, scope: ["a.test", "!b.test"], notes: "", targetNotes: {}, apiSpecs: [], rules: DEFAULT_RULES, testingNow: null },
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
  assert.deepEqual(asEngagement({ id: "a", name: "A", target_notes: { "a.test": "staging", bad: 3 } })?.targetNotes, { "a.test": "staging" });
  const e = asEngagement({ id: "a", name: "A", rules: { rate_limit: null, windows: [{ days: ["mon", "funday"], start: "09:00", end: "17:00" }, { nope: 1 }] } });
  assert.equal(e?.rules.rate_limit, null);
  assert.deepEqual(e?.rules.windows, [{ days: ["mon"], start: "09:00", end: "17:00" }]);
  assert.deepEqual(e?.rules.safe_methods, DEFAULT_RULES.safe_methods, "missing fields take the defaults");
});

test("the authorization countdown", async () => {
  const { expiryText, todayIn, viewFromHash } = await import("./engagement.ts");
  assert.deepEqual(expiryText("2026-11-30", "2026-11-22"), { text: "authorization expires in 8 days", tone: "ok" });
  assert.deepEqual(expiryText("2026-11-24", "2026-11-22"), { text: "authorization expires in 2 days", tone: "soon" });
  assert.deepEqual(expiryText("2026-11-23", "2026-11-22"), { text: "authorization expires in 1 day", tone: "soon" });
  assert.equal(expiryText("2026-11-22", "2026-11-22")?.tone, "soon");
  assert.deepEqual(expiryText("2026-11-20", "2026-11-22"), { text: "authorization expired 2 days ago", tone: "past" });
  assert.deepEqual(expiryText("2027-03-01", "2026-02-28"), { text: "authorization expires in 366 days", tone: "ok" }, "across a leap day");
  assert.equal(expiryText(null, "2026-11-22"), null);
  assert.equal(expiryText("soon", "2026-11-22"), null);
  // 2026-11-22T12:00Z is already the 23rd in Auckland.
  const now = new Date("2026-11-22T12:00:00Z");
  assert.equal(todayIn("Pacific/Auckland", now), "2026-11-23");
  assert.equal(todayIn("America/Chicago", now), "2026-11-22");
  assert.equal(todayIn("Mars/Olympus", now), "2026-11-22", "unknown zones fall back to UTC");
  assert.equal(viewFromHash("#engagement"), "engagement");
  assert.equal(viewFromHash(""), "engagement", "Engagement is home (#71)");
  assert.equal(viewFromHash("#"), "engagement");
  assert.equal(viewFromHash("#traffic"), "traffic");
  assert.equal(viewFromHash("#findings/c-1"), "findings");
  assert.equal(viewFromHash("#surface"), "engagement", "unbuilt views fall back to home");
  assert.equal(viewFromHash("#nonsense"), "engagement");
  assert.equal(viewFromHash("#engagement/paste"), "engagement");
});

test("scope-list previews are shape-checked", async () => {
  const { asPreview } = await import("./engagement.ts");
  assert.deepEqual(
    asPreview({
      items: [
        { line: 2, source: "https://a.test/x", entry: "a.test", exclude: false, error: null, notes: ["path dropped"], present: false },
        { line: 3, source: "nope!", entry: null, exclude: false, error: "not a hostname", notes: [], present: false },
        { junk: true },
      ],
      add: ["a.test"],
      exclude: [],
    }),
    {
      items: [
        { line: 2, source: "https://a.test/x", entry: "a.test", exclude: false, error: null, notes: ["path dropped"], present: false },
        { line: 3, source: "nope!", entry: null, exclude: false, error: "not a hostname", notes: [], present: false },
      ],
      add: ["a.test"],
      exclude: [],
    },
  );
  assert.equal(asPreview({ nope: 1 }), null);
});

test("OpenAPI imports are shape-checked", async () => {
  const { asImport, asApiSpecs } = await import("./engagement.ts");
  const spec = { title: "ACME", version: "2", format: "openapi 3.0.3", servers: ["https://api.acme.test"], paths: ["/users", 5], imported: "t" };
  const i = asImport({ spec, notes: ["relative server"], items: [], add: ["api.acme.test"], exclude: [] });
  assert.deepEqual(i?.spec.paths, ["/users"]);
  assert.deepEqual(i?.preview.add, ["api.acme.test"]);
  assert.equal(asImport({ items: [] }), null, "no spec");
  assert.deepEqual(asApiSpecs([{ nope: 1 }, spec]).length, 1);
});
