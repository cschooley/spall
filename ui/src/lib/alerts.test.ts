import { test } from "node:test";
import assert from "node:assert/strict";
import { alertHash, alertText, asAlerts } from "./alerts.ts";

test("alerts are read as kinds and ids, worded without engagement text", () => {
  const r = asAlerts({
    latest: 4,
    alerts: [
      { seq: 1, at: "t", kind: "waiting", task: "T-3" },
      { seq: 2, at: "t", kind: "oob_hit", task: "T-3", item: "I-12" },
      { seq: 3, at: "t", kind: "failed", task: "secret.evilcorp.test" },
      { seq: 4, at: "t", kind: "exfiltrate", task: "T-1" },
    ],
  });
  assert.ok(typeof r !== "string");
  assert.equal(r.latest, 4);
  assert.deepEqual(r.alerts.map(alertText), ["T-3 is waiting on your approval", "Out-of-band callback I-12 (T-3)", "a task failed: open it for the reason"]);
  assert.deepEqual(r.alerts.map(alertHash), ["tasks/T-3", "engagement", "engagement"]);
  assert.equal(asAlerts("spall unreachable"), "spall unreachable");
});
