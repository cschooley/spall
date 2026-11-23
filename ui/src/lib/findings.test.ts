import { test } from "node:test";
import assert from "node:assert/strict";
import { asCandidate, type Candidate } from "./candidates.ts";
import { RAW } from "./candidates.fixture.ts";
import { candidateFromHash, duplicateTargets, hostOf, matchesFindingQuery, nextAfter, queue, tabCounts } from "./findings.ts";

function cand(id: string, over: { status?: string; disposition?: string; rule?: string; url?: string; message?: string } = {}): Candidate {
  const f = RAW.finding;
  return asCandidate({
    ...RAW,
    id,
    finding: {
      ...f,
      ruleId: over.rule ?? f.ruleId,
      message: { text: over.message ?? f.message.text },
      locations: over.url ? [{ physicalLocation: { artifactLocation: { uri: over.url } }, logicalLocations: [{ name: "GET", kind: "httpMethod" }] }] : f.locations,
      suppressions: over.disposition ? [{ kind: "external", status: "accepted", justification: "why", properties: { "flux/disposition": over.disposition } }] : [],
      properties: { ...f.properties, "flux/status": over.status ?? "candidate" },
    },
  })!;
}

const LIST = [
  cand("c-1"),
  cand("c-2", { status: "confirmed", rule: "WSTG-ATHZ-04", url: "https://api.other.test:8443/o/1" }),
  cand("c-3", { disposition: "falsePositive" }),
  cand("c-4", { disposition: "duplicate" }),
  cand("c-5", { message: "Reflected in title" }),
];

test("tabs split the queue by where each stands", () => {
  const ids = (tab: Parameters<typeof queue>[1]) => queue(LIST, tab, "").map((c) => c.id);
  assert.deepEqual(ids("to triage"), ["c-1", "c-5"]);
  assert.deepEqual(ids("confirmed"), ["c-2"]);
  assert.deepEqual(ids("decided"), ["c-3", "c-4"]);
  assert.equal(ids("all").length, 5);
  assert.deepEqual(tabCounts(LIST), { "to triage": 2, confirmed: 1, decided: 2, all: 5 });
});

test("the queue's filter terms", () => {
  const ids = (q: string) => queue(LIST, "all", q).map((c) => c.id);
  assert.deepEqual(ids("rule:wstg-athz"), ["c-2"]);
  assert.deepEqual(ids("host:other.test"), ["c-2"]);
  assert.deepEqual(ids("state:rejected"), ["c-3"]);
  assert.deepEqual(ids("state:to-triage title"), ["c-5"]);
  assert.deepEqual(ids("cwe:89 state:dup"), ["c-4"]);
  assert.deepEqual(ids("level:error c-1"), ["c-1"]);
  assert.deepEqual(ids("nothing-here"), []);
  assert.ok(matchesFindingQuery(LIST[0]!, "SORT"), "parameter, any case");
});

test("after a decision the selection moves down, else up", () => {
  const before = queue(LIST, "all", "");
  const without = (id: string) => before.filter((c) => c.id !== id);
  assert.equal(nextAfter(before, without("c-2"), "c-2"), "c-3");
  assert.equal(nextAfter(before, without("c-5"), "c-5"), "c-4");
  assert.equal(nextAfter(before, before, "c-2"), "c-2", "still there: stays");
  assert.equal(nextAfter([LIST[0]!], [], "c-1"), null);
});

test("hash, hosts and duplicate targets", () => {
  assert.equal(candidateFromHash("#findings/c-0a1b"), "c-0a1b");
  assert.equal(candidateFromHash("#findings"), null);
  assert.equal(candidateFromHash("#findings/../x"), null);
  assert.equal(hostOf("https://User@API.Test:443/x"), "api.test");
  assert.equal(hostOf("https://[::1]:8443/x"), "[::1]");
  assert.equal(hostOf(null), "");
  assert.deepEqual(duplicateTargets(LIST, "c-1").map((c) => c.id), ["c-2", "c-5"]);
});
