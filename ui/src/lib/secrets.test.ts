import { test } from "node:test";
import assert from "node:assert/strict";
import { isSecretHeader } from "./secrets.ts";

test("the flux redaction header list, case-insensitively", () => {
  for (const h of ["Authorization", "cookie", "SET-COOKIE", "proxy-authorization"]) assert.ok(isSecretHeader(h), h);
  for (const h of ["content-type", "x-authorization-hint", "cookies"]) assert.ok(!isSecretHeader(h), h);
});

test("the list is replaced by flux's from /api/redaction, and junk is ignored", async () => {
  const { setSecretHeaders } = await import("./secrets.ts");
  assert.ok(isSecretHeader("x-api-key"), "the default matches flux's");
  assert.equal(setSecretHeaders({ headers: [] }), false);
  assert.equal(setSecretHeaders({ headers: [1] }), false);
  assert.equal(setSecretHeaders(null), false);
  assert.ok(isSecretHeader("cookie"), "unchanged after junk");
  assert.equal(setSecretHeaders({ headers: ["Cookie", "x-new-secret"] }), true);
  assert.ok(isSecretHeader("X-New-Secret"));
  assert.ok(!isSecretHeader("authorization"));
  setSecretHeaders({ headers: ["authorization", "proxy-authorization", "cookie", "set-cookie", "x-api-key", "api-key", "x-auth-token", "x-access-token"] });
});
