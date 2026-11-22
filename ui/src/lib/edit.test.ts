import { test } from "node:test";
import assert from "node:assert/strict";
import {
  applyHeaderEdits,
  changes,
  draftOf,
  hostMismatch,
  replayBodyFor,
  scatteredRepeats,
  syncContentLength,
  validateDraft,
} from "./edit.ts";
import type { TrafficEntry } from "../types.ts";

const orig: TrafficEntry = {
  schema_version: 5, id: "o1", ts: "2026-12-25T10:00:00Z", host: "shop.example.test", method: "POST",
  url: "https://shop.example.test/api/cart",
  req_headers: [["host", "shop.example.test"], ["content-type", "application/json"], ["content-length", "9"], ["cookie", "a=1"]],
  req_body: '{"qty":1}', req_body_encoding: "utf8",
  status: 200, res_headers: [], res_body: null, res_body_encoding: "utf8", duration_ms: 1, replay_of: null,
};

test("a draft starts as the captured request; binary bodies aren't editable", () => {
  assert.deepEqual(draftOf(orig), { method: "POST", headers: orig.req_headers, body: '{"qty":1}' });
  assert.equal(draftOf({ ...orig, req_body: null }).body, "");
  assert.equal(draftOf({ ...orig, req_body: "iVBO", req_body_encoding: "base64" }).body, null);
  assert.notEqual(draftOf(orig).headers, orig.req_headers, "a copy, not the original's array");
});

test("an unchanged draft replays as captured: only the id", () => {
  assert.deepEqual(replayBodyFor(orig, draftOf(orig)), { ok: true, body: { id: "o1" }, contentLength: null });
});

test("only changed fields are sent; the full ordered header list when headers change", () => {
  const d = draftOf(orig);
  d.method = "PUT";
  assert.deepEqual(replayBodyFor(orig, d), { ok: true, body: { id: "o1", method: "PUT" }, contentLength: null });

  const h = draftOf(orig);
  h.headers = [["cookie", "a=1"], ["cookie", "b=2"], ["host", "shop.example.test"]];
  assert.deepEqual(replayBodyFor(orig, h), {
    ok: true,
    body: { id: "o1", req_headers: [["cookie", "a=1"], ["cookie", "b=2"], ["host", "shop.example.test"]] },
    contentLength: null,
  });
});

test("an edited body is sent as utf8 and Content-Length follows it", () => {
  const d = draftOf(orig);
  d.body = '{"qty":1000}';
  const r = replayBodyFor(orig, d);
  assert.ok(r.ok);
  assert.equal(r.body.body, '{"qty":1000}');
  assert.equal(r.body.body_encoding, "utf8");
  assert.deepEqual(r.body.req_headers?.find(([k]) => k === "content-length"), ["content-length", "12"]);
  assert.equal(r.contentLength, 12);
  // Bytes, not characters.
  assert.deepEqual(syncContentLength([["Content-Length", "1"]], "é"), { headers: [["Content-Length", "2"]], updated: true });
  // No Content-Length captured: none added (hyper frames the body).
  assert.deepEqual(syncContentLength([["host", "h"]], "abc"), { headers: [["host", "h"]], updated: false });
  assert.deepEqual(syncContentLength([["content-length", "3"]], "abc"), { headers: [["content-length", "3"]], updated: false });
});

test("a binary body is never sent back as text", () => {
  const bin = { ...orig, req_body: "iVBO", req_body_encoding: "base64" as const };
  const d = draftOf(bin);
  d.method = "PATCH";
  const r = replayBodyFor(bin, d);
  assert.ok(r.ok);
  assert.equal(r.body.body, undefined);
  assert.equal(r.body.body_encoding, undefined);
});

test("drafts are validated: token methods and names, no CR, LF or NUL in values", () => {
  const bad = (f: (d: ReturnType<typeof draftOf>) => void) => {
    const d = draftOf(orig);
    f(d);
    return validateDraft(d);
  };
  assert.equal(bad(() => {}), null);
  assert.match(bad((d) => (d.method = "GET /x"))!, /method/);
  assert.match(bad((d) => (d.method = ""))!, /method/);
  assert.match(bad((d) => d.headers.push(["bad name", "v"]))!, /header name/);
  assert.match(bad((d) => d.headers.push(["", "v"]))!, /header name/);
  assert.match(bad((d) => d.headers.push(["x", "a\r\nInjected: 1"]))!, /x/);
  assert.match(bad((d) => d.headers.push(["x", "a\nb"]))!, /CR or LF/);
  assert.match(bad((d) => d.headers.push(["x", "a\0"]))!, /x/);
  assert.equal(bad((d) => d.headers.push(["X-Custom_1", "any text: ok; = ,"])), null);
  const r = replayBodyFor(orig, { ...draftOf(orig), method: "G ET" });
  assert.equal(r.ok, false);
});

test("changes mark what differs from the original", () => {
  assert.deepEqual(changes(orig, draftOf(orig)), { method: false, body: false, headers: [false, false, false, false], removed: 0, count: 0 });
  const d = draftOf(orig);
  d.method = "PUT";
  d.headers = [["host", "shop.example.test"], ["cookie", "a=2"], ["x-new", "1"]];
  d.body = "{}";
  const c = changes(orig, d);
  assert.deepEqual(c.headers, [false, true, true]);
  assert.equal(c.removed, 2, "content-type and content-length are gone; cookie was modified, not removed");
  assert.equal(c.method && c.body, true);
  assert.equal(c.count, 1 + 1 + 2 + 2);
});

test("a Host header that differs from the URL's host is called out", () => {
  assert.equal(hostMismatch(orig.url, orig.req_headers), null);
  assert.equal(hostMismatch(orig.url, [["Host", "SHOP.example.test"]]), null);
  assert.equal(hostMismatch("https://shop.example.test:443/x", [["host", "shop.example.test"]]), null);
  assert.equal(hostMismatch("http://a.test:8080/", [["host", "a.test:8080"]]), null);
  assert.equal(hostMismatch(orig.url, [["host", "admin.internal"]]), "admin.internal");
  assert.equal(hostMismatch(orig.url, []), null, "no Host: derived from the URL");
});

test("repeats split by another header are noted (sent grouped)", () => {
  assert.equal(scatteredRepeats([["a", "1"], ["a", "2"], ["b", "3"]]), false);
  assert.equal(scatteredRepeats([["a", "1"], ["b", "3"], ["A", "2"]]), true);
  assert.equal(scatteredRepeats([]), false);
});

test("console header edits: set replaces every value in place, drop removes, new ones append", () => {
  const h: [string, string][] = [["host", "h"], ["cookie", "a=1"], ["x", "1"], ["Cookie", "b=2"]];
  assert.deepEqual(applyHeaderEdits(h, [["Cookie", "c=3"]], []), [["host", "h"], ["Cookie", "c=3"], ["x", "1"]]);
  assert.deepEqual(applyHeaderEdits(h, [], ["COOKIE"]), [["host", "h"], ["x", "1"]]);
  assert.deepEqual(applyHeaderEdits(h, [["x-new", "v"]], ["x"]), [["host", "h"], ["cookie", "a=1"], ["Cookie", "b=2"], ["x-new", "v"]]);
});
