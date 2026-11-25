import { test } from "node:test";
import assert from "node:assert/strict";
import { asOob, hitLine } from "./oob.ts";

test("the OOB state is read from the server, hits as text", () => {
  assert.deepEqual(asOob({ configured: false }), { configured: false });
  assert.equal(asOob("spall unreachable"), "spall unreachable");
  const s = asOob({
    configured: true,
    domain: "oast.lab",
    payloads: [{ id: "abc", host: "abc.oast.lab", url: "http://abc.oast.lab/", lab_url: "http://interactsh/abc", by: "human", at: "t" }, { junk: 1 }],
    hits: [{ id: "I-1", payload: "abc", protocol: "dns", remote: "203.0.113.9", at: "t", raw_request: "<b>x</b>", task: "T-1", exchange: "s-4" }],
  });
  assert.ok(typeof s !== "string" && s.configured);
  assert.equal(s.payloads.length, 1);
  assert.equal(s.payloads[0]!.labUrl, "http://interactsh/abc");
  assert.equal(hitLine(s.hits[0]!), "I-1  t  dns   from 203.0.113.9  on abc  via s-4  (T-1)");
});
