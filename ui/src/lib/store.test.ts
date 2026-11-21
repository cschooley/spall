import { test } from "node:test";
import assert from "node:assert/strict";
import { addEntries, emptyStore, clearStore } from "./store.ts";
import type { TrafficEntry } from "../types.ts";

const e = (id: string): TrafficEntry => ({
  schema_version: 3,
  id,
  ts: "2026-12-25T10:00:00Z",
  host: "h",
  method: "GET",
  url: "/",
  req_headers: [],
  req_body: null,
  req_body_encoding: "utf8",
  status: 200,
  res_headers: [],
  res_body: null,
  res_body_encoding: "utf8",
  duration_ms: 1,
  replay_of: null,
});

const ids = (s: ReturnType<typeof emptyStore>) => s.items.map((x) => x.id);

test("adds in order and dedupes by id (SSE reconnects replay history)", () => {
  let s = addEntries(emptyStore(), [e("a"), e("b")], 10);
  s = addEntries(s, [e("b"), e("c"), e("a")], 10);
  assert.deepEqual(ids(s), ["a", "b", "c"]);
});

test("returns the same store when nothing is new, so React can skip a render", () => {
  const s = addEntries(emptyStore(), [e("a")], 10);
  assert.equal(addEntries(s, [e("a")], 10), s);
  assert.equal(addEntries(s, [], 10), s);
});

test("caps at the newest N and forgets evicted ids", () => {
  let s = addEntries(emptyStore(), [e("1"), e("2"), e("3"), e("4")], 3);
  assert.deepEqual(ids(s), ["2", "3", "4"]);
  s = addEntries(s, [e("1")], 3);
  assert.deepEqual(ids(s), ["3", "4", "1"], "evicted id can come back");
});

test("clear hides rows but remembers ids, so a reconnect does not re-add them", () => {
  let s = addEntries(emptyStore(), [e("a"), e("b")], 10);
  s = clearStore(s);
  assert.deepEqual(ids(s), []);
  s = addEntries(s, [e("a"), e("c")], 10);
  assert.deepEqual(ids(s), ["c"]);
});

test("ignores entries without a string id", () => {
  const bad = { ...e("x"), id: 5 } as unknown as TrafficEntry;
  assert.deepEqual(ids(addEntries(emptyStore(), [bad], 10)), []);
});
