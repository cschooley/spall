import { test } from "node:test";
import assert from "node:assert/strict";
import { mergeExchanges, tsKey } from "./merge.ts";

test("tsKey orders RFC 3339 times with any fraction length", () => {
  const ts = ["2026-12-25T10:00:05.123456789Z", "2026-12-25T10:00:05.1Z", "2026-12-25T10:00:05Z", "2026-12-25T10:00:04.999Z"];
  assert.deepEqual([...ts].sort((a, b) => (tsKey(a) < tsKey(b) ? -1 : 1)), [
    "2026-12-25T10:00:04.999Z",
    "2026-12-25T10:00:05Z",
    "2026-12-25T10:00:05.1Z",
    "2026-12-25T10:00:05.123456789Z",
  ]);
});

const x = (id: string, ts: string, host = "h") => ({ id, ts, host });

test("history and live merge oldest-first, deduped, filtered", () => {
  const history = [x("c", "2026-12-25T10:00:03Z"), x("b", "2026-12-25T10:00:02Z"), x("a", "2026-12-25T10:00:01Z")];
  const live = [x("c", "2026-12-25T10:00:03Z"), x("d", "2026-12-25T10:00:04Z"), x("z", "2026-12-25T10:00:05Z", "skip")];
  const out = mergeExchanges(history, live, (e) => e.host !== "skip");
  assert.deepEqual(out.map((e) => e.id), ["a", "b", "c", "d"]);
});

test("equal timestamps keep a stable order (history before live, then arrival)", () => {
  const t = "2026-12-25T10:00:00Z";
  const out = mergeExchanges([x("h2", t), x("h1", t)], [x("l1", t), x("l2", t)], () => true);
  assert.deepEqual(out.map((e) => e.id), ["h1", "h2", "l1", "l2"]);
});
