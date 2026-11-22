import { test } from "node:test";
import assert from "node:assert/strict";
import { asCaInfo } from "./ca.ts";

test("CA details are shape-checked", () => {
  assert.deepEqual(asCaInfo({ sha256: "EB:6F", subject: "CN=taphttp CA", not_after: "2036-11-20T00:00:00Z", proxy: 5 }), {
    sha256: "EB:6F",
    subject: "CN=taphttp CA",
    notBefore: null,
    notAfter: "2036-11-20T00:00:00Z",
    proxy: null,
  });
  assert.equal(asCaInfo("nope"), null);
});
