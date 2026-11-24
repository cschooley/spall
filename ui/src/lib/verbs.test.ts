import { test } from "node:test";
import assert from "node:assert/strict";
import { asDrafted, draftedLine, VERBS, verbsOn } from "./verbs.ts";
import { SPECS } from "../commands/specs.gen.ts";
import { findCommand } from "../commands/registry.ts";

test("every verb is a registry command: palette, console and MCP alike (#67)", () => {
  for (const v of VERBS) {
    const spec = SPECS.find((s) => s.name === v.name);
    assert.ok(spec, `${v.name}: in the server's registry`);
    assert.equal(spec.mcp, true, `${v.name}: an MCP tool`);
    assert.equal(spec.risk, "record", `${v.name}: drafts only`);
    const cmd = findCommand(v.name);
    assert.ok(cmd?.palette, `${v.name}: a palette entry`);
    assert.ok(cmd.run, `${v.name}: a console command`);
    assert.ok(v.hint(2).length > 0, `${v.name}: its cost, first`);
  }
  assert.deepEqual(
    verbsOn("exchange").map((v) => v.name),
    ["why", "map_auth", "test_for"],
  );
  assert.ok(!VERBS.some((v) => v.name === "poc"), "hidden until the sandbox (#13)");
});

test("a draft is read from the answer, or the error passed on", () => {
  const d = asDrafted({ task: { id: "T-7", role: "tester" }, cost: "~5 requests, safe methods" });
  assert.deepEqual(d, { id: "T-7", role: "tester", cost: "~5 requests, safe methods" });
  assert.equal(asDrafted("no candidate c-1"), "no candidate c-1");
  assert.equal(asDrafted({}), "unexpected answer from spall");
  assert.match(draftedLine("retest", d as never), /drafted T-7 \(tester, ~5 requests/);
});
