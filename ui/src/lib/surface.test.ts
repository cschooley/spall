import { test } from "node:test";
import assert from "node:assert/strict";
import { asSurface, filterRows, mark, rowLine, totals, type SurfaceListing } from "./surface.ts";

const RAW = {
  categories: [
    { code: "ATHZ", name: "Authorization" },
    { code: "INPV", name: "Input validation" },
  ],
  endpoints: [
    {
      key: "app.test|GET|/users/{}",
      host: "app.test",
      method: "GET",
      template: "/users/{id}",
      count: 4,
      statuses: [200, 404],
      params: ["q"],
      first_seen: "t1",
      last_seen: "t2",
      example: "s-9",
      by_you: 3,
      tasks: ["T-2"],
      documented: false,
      scope: "in",
      coverage: { ATHZ: { state: "found", candidates: ["c-1"] }, INPV: { state: "tested", by: ["agent:T-2"], how: ["sweep x"] }, BOGUS: { state: "nope" } },
    },
    { key: "app.test|ANY|/login", host: "app.test", method: "ANY", template: "/login", count: 0, statuses: [], params: [], first_seen: "", last_seen: "", example: null, by_you: 0, tasks: [], documented: true, scope: "weird", coverage: {} },
  ],
  dropped: 0,
};

test("the server's listing is read defensively", () => {
  const s = asSurface(RAW) as SurfaceListing;
  assert.equal(s.rows.length, 2);
  const [a, b] = s.rows;
  assert.equal(a?.byYou, 3);
  assert.deepEqual(Object.keys(a?.coverage ?? {}), ["ATHZ", "INPV"]);
  assert.equal(b?.scope, "unknown");
  assert.equal(asSurface("spall unreachable"), "spall unreachable");
  assert.equal(asSurface({}), "unexpected answer from spall");
});

test("filters: untested, found, host, method, documented, agent, text", () => {
  const { rows } = asSurface(RAW) as SurfaceListing;
  const keys = (q: string) => filterRows(rows, q).map((r) => r.template);
  assert.deepEqual(keys("untested:athz"), ["/login"]);
  assert.deepEqual(keys("untested:INPV"), ["/login"]);
  assert.deepEqual(keys("tested:INPV"), ["/users/{id}"]);
  assert.deepEqual(keys("found"), ["/users/{id}"]);
  assert.deepEqual(keys("documented"), ["/login"]);
  assert.deepEqual(keys("agent"), ["/users/{id}"]);
  assert.deepEqual(keys("method:POST"), ["/login"]);
  assert.deepEqual(keys("host:APP users"), ["/users/{id}"]);
  assert.deepEqual(keys(""), ["/users/{id}", "/login"]);
});

test("totals and marks read without color", () => {
  const { rows, categories } = asSurface(RAW) as SurfaceListing;
  assert.deepEqual(totals(rows, categories), { ATHZ: { found: 1, tested: 0, untested: 1 }, INPV: { found: 0, tested: 1, untested: 1 } });
  const [r] = rows;
  assert.ok(r);
  assert.deepEqual([mark(r.coverage.ATHZ), mark(r.coverage.INPV), mark(undefined)], ["!", "✓", "·"]);
  assert.equal(rowLine(r, categories), "GET    app.test/users/{id}  4× !✓  you,◆T-2");
});
