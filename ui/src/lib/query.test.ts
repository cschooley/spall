import { test } from "node:test";
import assert from "node:assert/strict";
import { apiParams, matchesQuery, parseFilter } from "./query.ts";

test("parses tokens and free text", () => {
  assert.deepEqual(parseFilter("host:shop.example status:4xx method:post login form"), {
    host: "shop.example",
    statusClass: 4,
    method: "POST",
    text: "login form",
    ignored: [],
  });
  assert.deepEqual(parseFilter("status:404"), { status: 404, ignored: [] });
  assert.deepEqual(parseFilter("  "), { ignored: [] });
});

test("unsupported or malformed tokens are reported, not guessed", () => {
  assert.deepEqual(parseFilter("by:agent status:abc status:7xx status:99999 host:").ignored, [
    "by:agent",
    "status:abc",
    "status:7xx",
    "status:99999",
    "host:",
  ]);
});

test("API params: exact filters, status classes (taphttp 0.4.0+) and q; replay: stays client-side", () => {
  assert.deepEqual(apiParams(parseFilter("host:a method:get status:503 x y")), [
    ["host", "a"],
    ["method", "GET"],
    ["status", "503"],
    ["q", "x y"],
  ]);
  assert.deepEqual(apiParams(parseFilter("status:5xx")), [["status", "5xx"]]);
  assert.deepEqual(apiParams(parseFilter("replay:yes")), [], "taphttp can't filter on having replay_of");
});

test("replay:yes / replay:no select replays or originals", () => {
  assert.equal(parseFilter("replay:yes").replay, true);
  assert.equal(parseFilter("replay:no").replay, false);
  assert.deepEqual(parseFilter("replay:maybe").ignored, ["replay:maybe"]);
  const orig = { ...e, replay_of: null };
  const rep = { ...e, replay_of: "o1" };
  assert.ok(matchesQuery(rep, parseFilter("replay:yes")) && !matchesQuery(orig, parseFilter("replay:yes")));
  assert.ok(matchesQuery(orig, parseFilter("replay:no")) && !matchesQuery(rep, parseFilter("replay:no")));
  assert.ok(matchesQuery({ host: "h", method: "GET", url: "/", status: 1 }, parseFilter("replay:no")), "no replay_of field: original");
});

const e = { host: "Shop.Example.test", method: "POST", url: "https://shop.example.test/api/Login?x=1", status: 401 };

test("client predicate mirrors the API: host contains, method equals, q in URL or host", () => {
  for (const f of ["", "host:shop", "host:EXAMPLE", "method:post", "status:401", "status:4xx", "login", "x=1", "shop.example.test"]) {
    assert.ok(matchesQuery(e, parseFilter(f)), f);
  }
  for (const f of ["host:other", "method:get", "status:400", "status:5xx", "logout", "post"]) {
    assert.ok(!matchesQuery(e, parseFilter(f)), f);
  }
  assert.ok(!matchesQuery({ ...e, status: null }, parseFilter("status:4xx")));
});
