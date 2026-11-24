import { test } from "node:test";
import assert from "node:assert/strict";
import { asCandidate, type Candidate } from "./candidates.ts";
import { RAW } from "./candidates.fixture.ts";
import { candidateFromHash, duplicateTargets, groupDuplicates, hostOf, matchesFindingQuery, nextAfter, originalOf, queue, tabCounts } from "./findings.ts";

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
  assert.deepEqual(tabCounts(LIST), { "to triage": 2, confirmed: 1, promoted: 0, decided: 2, all: 5 });
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

test("duplicates are listed under their original (#78)", () => {
  const orig = { ...cand("c-o", { status: "confirmed" }), guid: "g-1" };
  const decided = { ...cand("c-d"), suppressed: { disposition: "duplicate" as const, justification: "same", duplicateOf: "g-1" } };
  const maybe = { ...cand("c-m"), possibleDuplicateOf: "c-o" };
  const other = cand("c-x");
  const lost = { ...cand("c-l"), possibleDuplicateOf: "c-gone" };
  const all = [maybe, other, orig, decided, lost];
  assert.equal(originalOf(decided, all)?.id, "c-o");
  assert.equal(originalOf(maybe, all)?.id, "c-o");
  assert.equal(originalOf(lost, all), null, "an original that's gone");
  assert.equal(originalOf(other, all), null);
  assert.deepEqual(
    groupDuplicates(all, all).map((g) => [g.c.id, g.original?.id ?? null, g.nested]),
    [
      ["c-x", null, false],
      ["c-o", null, false],
      ["c-m", "c-o", true],
      ["c-d", "c-o", true],
      ["c-l", null, false],
    ],
  );
  // The original in another tab: the duplicate stays in place and names it.
  assert.deepEqual(
    groupDuplicates([maybe, other], all).map((g) => [g.c.id, g.original?.id ?? null, g.nested]),
    [
      ["c-m", "c-o", false],
      ["c-x", null, false],
    ],
  );
});
