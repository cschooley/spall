import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_RULES, describeRules, parseDays, parseSpan, parseMethods, type Rules } from "./roe.ts";

test("days: ranges, lists and names", () => {
  assert.deepEqual(parseDays("mon-fri"), ["mon", "tue", "wed", "thu", "fri"]);
  assert.deepEqual(parseDays("Sat,Sun"), ["sat", "sun"]);
  assert.deepEqual(parseDays("weekdays"), ["mon", "tue", "wed", "thu", "fri"]);
  assert.deepEqual(parseDays("weekends"), ["sat", "sun"]);
  assert.equal(parseDays("daily")?.length, 7);
  assert.deepEqual(parseDays("fri-mon"), ["fri", "sat", "sun", "mon"], "ranges wrap past sunday");
  assert.deepEqual(parseDays("mon,wed-thu,mon"), ["mon", "wed", "thu"], "no repeats");
  for (const bad of ["", "funday", "mon-", "mon--fri", "1-5"]) assert.equal(parseDays(bad), null, bad);
});

test("spans and methods", () => {
  assert.deepEqual(parseSpan("09:00-17:00"), { start: "09:00", end: "17:00" });
  assert.deepEqual(parseSpan("22:00-06:00"), { start: "22:00", end: "06:00" });
  assert.deepEqual(parseSpan("00:00-24:00"), { start: "00:00", end: "24:00" });
  for (const bad of ["9-17", "09:00", "25:00-26:00", "09:00-09:00", "24:00-01:00"]) assert.equal(parseSpan(bad), null, bad);
  assert.deepEqual(parseMethods("get, Post"), ["GET", "POST"]);
  assert.deepEqual(parseMethods("any"), []);
  assert.equal(parseMethods("G ET"), null);
});

test("rules read as a few lines", () => {
  assert.deepEqual(describeRules(DEFAULT_RULES), [
    "methods: any · confirm all but GET, HEAD, OPTIONS · rate 10/s · time zone UTC",
    "testing windows: any time",
    "no-test windows: none",
    "agents: not allowed · destructive actions: not allowed · requests that look destructive: asked for · sweeps: up to 200 requests",
  ]);
  const r: Rules = {
    ...DEFAULT_RULES,
    methods: ["GET", "POST"],
    safe_methods: [],
    rate_limit: null,
    time_zone: "America/Chicago",
    windows: [{ days: ["mon", "tue", "wed", "thu", "fri"], start: "09:00", end: "17:00" }, { days: ["sat"], start: "10:00", end: "12:00" }],
    blackouts: [{ from: "2026-11-26T18:00", to: "2026-11-27T08:00", note: "release freeze" }],
    agents_allowed: true,
  };
  assert.deepEqual(describeRules(r), [
    "methods: GET, POST · confirm every method · no rate limit · time zone America/Chicago",
    "testing windows: 1) mon-fri 09:00-17:00  2) sat 10:00-12:00",
    "no-test windows: 1) 2026-11-26 18:00 → 2026-11-27 08:00 (release freeze)",
    "agents: allowed · destructive actions: not allowed · requests that look destructive: asked for · sweeps: up to 200 requests",
  ]);
});

test("the rules form sends only what changed", async () => {
  const { rulesFormOf, rulesPatchOf } = await import("./roe.ts");
  const f = rulesFormOf(DEFAULT_RULES);
  assert.deepEqual(f, { methods: "any", safe: "GET, HEAD, OPTIONS", rate: "10", tz: "UTC", agents: false, destructive: false, refuseLooking: false, sweepMax: "200" });
  assert.deepEqual(rulesPatchOf(f, DEFAULT_RULES), {});
  assert.deepEqual(rulesPatchOf({ ...f, methods: "get,post", rate: "off", tz: " Europe/Berlin ", agents: true }, DEFAULT_RULES), {
    methods: ["GET", "POST"],
    rate_limit: 0,
    time_zone: "Europe/Berlin",
    agents_allowed: true,
  });
  assert.deepEqual(rulesPatchOf({ ...f, safe: "" }, DEFAULT_RULES), { safe_methods: [] });
  assert.deepEqual(rulesPatchOf({ ...f, rate: "2.5" }, DEFAULT_RULES), { rate_limit: 2.5 });
  for (const bad of [{ ...f, rate: "fast" }, { ...f, rate: "-1" }, { ...f, methods: "G@T" }, { ...f, safe: "any" }]) {
    assert.equal(typeof rulesPatchOf(bad, DEFAULT_RULES), "string", JSON.stringify(bad));
  }
});
