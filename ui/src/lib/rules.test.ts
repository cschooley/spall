import { test } from "node:test";
import assert from "node:assert/strict";
import { asCatalog, OTHER, ruleLine, searchRules } from "./rules.ts";

const raw = {
  wstg: "4.2",
  attribution: "OWASP WSTG v4.2 (CC BY-SA 4.0)",
  rules: [
    { id: "WSTG-INPV-05", title: "Testing for SQL Injection", cwe: 89, help: "https://owasp.org/x" },
    { id: "WSTG-ATHZ-04", title: "Testing for Insecure Direct Object References", cwe: 639, help: "https://owasp.org/y" },
    { id: OTHER, title: "Other (not in the catalog)", cwe: null, help: null },
    { id: 5, title: "bad" },
    null,
  ],
};

test("asCatalog keeps well-formed rules only", () => {
  const c = asCatalog(raw);
  assert.ok(c);
  assert.equal(c.wstg, "4.2");
  assert.deepEqual(
    c.rules.map((r) => [r.id, r.cwe]),
    [["WSTG-INPV-05", 89], ["WSTG-ATHZ-04", 639], [OTHER, null]],
  );
  assert.equal(asCatalog({ rules: "x" }), null);
  assert.equal(asCatalog(null), null);
});

test("searchRules matches every word in id, title or cwe", () => {
  const rules = asCatalog(raw)!.rules;
  const ids = (t: string) => searchRules(rules, t).map((r) => r.id);
  assert.deepEqual(ids("sql injection"), ["WSTG-INPV-05"]);
  assert.deepEqual(ids("CWE-639"), ["WSTG-ATHZ-04"]);
  assert.deepEqual(ids("inpv"), ["WSTG-INPV-05"]);
  assert.equal(ids("").length, 3);
  assert.deepEqual(ids("nothing"), []);
});

test("ruleLine names the CWE when there is one", () => {
  const [sqli, , other] = asCatalog(raw)!.rules;
  assert.equal(ruleLine(sqli!), "WSTG-INPV-05 Testing for SQL Injection (CWE-89)");
  assert.equal(ruleLine(other!), "spall/other Other (not in the catalog)");
});
