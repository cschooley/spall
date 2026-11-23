import { test } from "node:test";
import assert from "node:assert/strict";
import { asCandidate, asCandidates, flagBody, paramNames, parseFlagArgs, stateOf, where } from "./candidates.ts";
import { G, RAW } from "./candidates.fixture.ts";

test("asCandidate reads what the UI shows from a flux finding", () => {
  const c = asCandidate(RAW)!;
  assert.equal(c.id, "c-0a1b2c3d");
  assert.equal(c.ruleId, "WSTG-INPV-05");
  assert.equal(c.level, "error");
  assert.equal(c.rank, 60);
  assert.equal(c.status, "candidate");
  assert.equal(c.method, "GET");
  assert.equal(c.url, "https://shop.test/api/p/17");
  assert.equal(c.parameter, "sort");
  assert.deepEqual(c.cwes, [89]);
  assert.deepEqual(c.exchanges, ["ex-1", "ex-2"]);
  assert.deepEqual(c.evidence, { method: "GET", target: "https://shop.test/api/p/17?sort=x'", status: 500, noResponse: false });
  assert.equal(c.suppressed, null);
  assert.equal(stateOf(c), "to triage");
  assert.equal(where(c), "GET https://shop.test/api/p/17 · parameter sort");
});

test("decisions in effect, and junk", () => {
  const with_ = (props: object, extra: object = {}) => asCandidate({ ...RAW, finding: { ...RAW.finding, ...extra, properties: { ...RAW.finding.properties, ...props } } })!;
  const dup = with_(
    {},
    {
      suppressions: [
        { kind: "external", status: "rejected", justification: "old" },
        { kind: "external", status: "accepted", justification: `duplicate of ${G}: same`, properties: { "flux/disposition": "duplicate", "flux/duplicateOf": G } },
      ],
    },
  );
  assert.deepEqual(dup.suppressed, { disposition: "duplicate", justification: `duplicate of ${G}: same`, duplicateOf: G });
  assert.equal(stateOf(dup), "duplicate");
  const rej = with_({}, { suppressions: [{ kind: "external", justification: "encoded", properties: { "flux/disposition": "falsePositive" } }] });
  assert.equal(stateOf(rej), "rejected", "no status means accepted (in effect)");
  const review = with_({}, { suppressions: [{ kind: "external", status: "underReview", justification: "?" }] });
  assert.equal(stateOf(review), "to triage");
  assert.equal(stateOf(with_({ "flux/status": "confirmed" })), "confirmed");
  assert.equal(stateOf(with_({}, { suppressions: [{ kind: "external", status: "accepted", justification: "ok", properties: { "flux/disposition": "acceptedRisk" } }] })), "accepted risk");
  const odd = with_({ "flux/status": "pwned" }, { level: "critical", rank: "high" });
  assert.equal(odd.status, null);
  assert.equal(odd.level, null);
  assert.equal(odd.rank, null);

  assert.equal(asCandidate({ id: "c-1" }), null);
  assert.equal(asCandidate(null), null);
  assert.deepEqual(asCandidates({ candidates: [RAW, null, 5] }).map((c) => c.id), ["c-0a1b2c3d"]);
  assert.deepEqual(asCandidates({}), []);
});

test("flagBody leaves out empty fields", () => {
  assert.deepEqual(flagBody({ exchange: "e1", rule: "R", parameter: " ", message: "", replays: [] }), { exchange: "e1", rule: "R" });
  assert.deepEqual(flagBody({ exchange: "e1", rule: "R", parameter: " id ", level: "error", rank: 0, message: " m ", replays: ["e2"], cwe: 693 }), {
    exchange: "e1",
    rule: "R",
    parameter: "id",
    level: "error",
    rank: 0,
    message: "m",
    replays: ["e2"],
    cwe: 693,
  });
});

test("parseFlagArgs", () => {
  assert.deepEqual(parseFlagArgs(["e1", "--rule", "WSTG-INPV-05", "--param", "sort", "--level", "error", "--rank", "70", "--replay", "e2", "--replay", "e3", "--message", "SQLi"]), {
    exchange: "e1",
    rule: "WSTG-INPV-05",
    parameter: "sort",
    level: "error",
    rank: 70,
    replays: ["e2", "e3"],
    message: "SQLi",
  });
  assert.deepEqual(parseFlagArgs(["e1", "--rule", "spall/other", "--cwe", "CWE-693"]), { exchange: "e1", rule: "spall/other", cwe: 693 });
  for (const bad of [[], ["--rule", "x"], ["e1"], ["e1", "--rule"], ["e1", "--rule", "R", "--level", "high"], ["e1", "--rule", "R", "--rank", "101"], ["e1", "--rule", "R", "--cwe", "x"], ["e1", "--rule", "R", "--nope", "1"]]) {
    assert.equal(typeof parseFlagArgs(bad), "string", bad.join(" "));
  }
});

test("paramNames from a URL's query", () => {
  assert.deepEqual(paramNames("https://a.test/x?sort=1&page=2&sort=3#frag"), ["sort", "page"]);
  assert.deepEqual(paramNames("https://a.test/x?q%20x=1&a+b=2"), ["q x", "a b"]);
  assert.deepEqual(paramNames("https://a.test/x"), []);
  assert.deepEqual(paramNames("https://a.test/x?%E0%A4%A=1"), ["%E0%A4%A"]);
});

test("promotion, export names", async () => {
  const { exportName, promotable } = await import("./candidates.ts");
  const c = asCandidate({ ...RAW, promoted: "2026-11-23T16:00:00Z", finding: { ...RAW.finding, properties: { ...RAW.finding.properties, "flux/status": "confirmed" } } })!;
  assert.equal(c.promoted, "2026-11-23T16:00:00Z");
  assert.ok(promotable(c));
  assert.ok(!promotable(asCandidate(RAW)!), "still a candidate");
  assert.equal(exportName('attachment; filename="spall-acme-1a2b-20261123.sarif"'), "spall-acme-1a2b-20261123.sarif");
  assert.equal(exportName('attachment; filename="../../etc/passwd"'), "spall-export.sarif");
  assert.equal(exportName(null), "spall-export.sarif");
});
