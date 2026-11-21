import { test } from "node:test";
import assert from "node:assert/strict";
import { isSecretHeader } from "./secrets.ts";

test("the flux redaction header list, case-insensitively", () => {
  for (const h of ["Authorization", "cookie", "SET-COOKIE", "proxy-authorization"]) assert.ok(isSecretHeader(h), h);
  for (const h of ["content-type", "x-authorization-hint", "cookies"]) assert.ok(!isSecretHeader(h), h);
});
