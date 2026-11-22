import { test } from "node:test";
import assert from "node:assert/strict";
import { asListing, hostKey, parentWildcard, stateLookup } from "./hosts.ts";

test("host listings are shape-checked", () => {
  const l = asListing({
    hosts: [{ host: "a.test", count: 3, first_seen: "t1", last_seen: "t2", state: "in", by: "*.test" }, { host: "b.test", state: "weird" }, { nope: 1 }],
    counts: { in: 1, out: 0, unknown: 1 },
    dropped: 0,
  });
  assert.deepEqual(l?.hosts, [
    { host: "a.test", count: 3, firstSeen: "t1", lastSeen: "t2", state: "in", by: "*.test" },
    { host: "b.test", count: 0, firstSeen: "", lastSeen: "", state: "unknown", by: null },
  ]);
  assert.equal(asListing({}), null);
});

test("row hosts are keyed like the server's", () => {
  assert.equal(hostKey("App.Acme.Test:8443"), "app.acme.test");
  assert.equal(hostKey("acme.test."), "acme.test");
  assert.equal(hostKey("[fd00::1]:443"), "fd00::1");
  assert.equal(hostKey("fd00::1"), "fd00::1");
  const look = stateLookup(asListing({ hosts: [{ host: "app.acme.test", state: "out", by: "!app.acme.test" }] }));
  assert.equal(look("APP.acme.test:443")?.state, "out");
  assert.equal(look("other.test"), null);
});

test("the parent wildcard for one-click exclusion", () => {
  assert.equal(parentWildcard("www.google-analytics.com"), "*.google-analytics.com");
  assert.equal(parentWildcard("a.b.cdn.test"), "*.b.cdn.test");
  assert.equal(parentWildcard("acme.test"), null);
  assert.equal(parentWildcard("10.1.2.3"), null);
  assert.equal(parentWildcard("fd00::1"), null);
});
