import { test } from "node:test";
import assert from "node:assert/strict";
import {
  base64Bytes,
  durationText,
  headerRows,
  headerValue,
  hexRows,
  HEX_BYTES_PER_ROW,
  pathOf,
  statusClass,
  statusText,
  timeOf,
} from "./format.ts";

test("pathOf keeps path and query, tolerates relative and junk URLs", () => {
  assert.equal(pathOf("https://example.com:443/a/b?x=1#frag"), "/a/b?x=1");
  assert.equal(pathOf("http://example.com"), "/");
  assert.equal(pathOf("/already/relative?q"), "/already/relative?q");
  assert.equal(pathOf("not a url"), "not a url");
});

test("timeOf shows local HH:MM:SS.mmm and survives bad input", () => {
  const ts = "2026-12-25T09:40:03.123Z";
  const d = new Date(ts);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  assert.equal(
    timeOf(ts),
    `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.123`,
  );
  assert.equal(timeOf("yesterday"), "yesterday");
});

test("status helpers", () => {
  assert.equal(statusClass(204), "st-2");
  assert.equal(statusClass(302), "st-3");
  assert.equal(statusClass(404), "st-4");
  assert.equal(statusClass(503), "st-5");
  assert.equal(statusClass(null), "st-err");
  assert.equal(statusClass(101), "");
  assert.equal(statusText(null), "ERR");
  assert.equal(statusText(200), "200");
  assert.equal(durationText(null), "");
  assert.equal(durationText(42), "42 ms");
});

test("headerRows keeps wire order and numbers repeats n/N, case-insensitively", () => {
  const rows = headerRows([
    ["Set-Cookie", "a=1"],
    ["content-type", "image/png"],
    ["set-cookie", "b=2"],
    ["set-cookie", "c=3"],
  ]);
  assert.deepEqual(
    rows.map((r) => [r.name, r.value, r.count]),
    [
      ["Set-Cookie", "a=1", "1/3"],
      ["content-type", "image/png", null],
      ["set-cookie", "b=2", "2/3"],
      ["set-cookie", "c=3", "3/3"],
    ],
  );
});

test("headerRows tolerates a v2 map and malformed input", () => {
  assert.deepEqual(
    headerRows({ host: "h" }).map((r) => [r.name, r.value]),
    [["host", "h"]],
  );
  assert.deepEqual(headerRows([["ok", "1"], ["bad"], "nope", null] as never), [
    { name: "ok", value: "1", count: null },
  ]);
  assert.deepEqual(headerRows(undefined), []);
});

test("headerValue finds the first value, case-insensitively", () => {
  assert.equal(headerValue([["Content-Type", "text/html"]], "content-type"), "text/html");
  assert.equal(headerValue([], "content-type"), null);
});

// Same reference body as the M0 hex view check: 0..255 twice, an HTML-looking
// string, 63 zero bytes. 600 bytes.
function sample(): Uint8Array {
  const parts = [
    ...Array.from({ length: 256 }, (_, i) => i),
    ...Array.from({ length: 256 }, (_, i) => i),
    ...Array.from(new TextEncoder().encode("<script>alert(1)</script>")),
    ...new Array(63).fill(0),
  ];
  return Uint8Array.from(parts);
}

/** Independent reference implementation (Python `ref` ported literally). */
function refHex(b: Uint8Array, start: number, count: number): string {
  const out: string[] = [];
  for (let r = start; r < start + count; r++) {
    const off = r * 8;
    if (off >= b.length) break;
    const c = Array.from(b.slice(off, off + 8));
    const hx = c.map((x) => x.toString(16).padStart(2, "0")).join(" ").padEnd(23, " ");
    const asc = c.map((x) => (x >= 0x20 && x < 0x7f ? String.fromCharCode(x) : ".")).join("");
    out.push(`${off.toString(16).padStart(8, "0")}  ${hx}  ${asc}`);
  }
  return out.join("\n");
}

test("hexRows matches the reference dump at edge sizes and pages", () => {
  const data = sample();
  assert.equal(data.length, 600);
  assert.equal(HEX_BYTES_PER_ROW, 8);
  for (const n of [0, 1, 7, 8, 9, 600]) {
    assert.equal(hexRows(data.slice(0, n), 0, 1000), refHex(data.slice(0, n), 0, 1000), `${n} bytes`);
  }
  assert.equal(hexRows(data, 64, 64), refHex(data, 64, 64), "page 2");
  assert.equal(hexRows(data, 74, 10).split("\n").length, 1, "last row only");
});

test("base64Bytes decodes, and rejects garbage", () => {
  assert.deepEqual(Array.from(base64Bytes("AAEC/w==")), [0, 1, 2, 255]);
  assert.throws(() => base64Bytes("!!!not base64"));
});

test("body views: json when it parses, raw always, binary as lossy text", async () => {
  const { bodyText, jsonView } = await import("./format.ts");
  assert.equal(jsonView('{"a":1,"b":[2]}'), '{\n  "a": 1,\n  "b": [\n    2\n  ]\n}');
  assert.equal(jsonView(" [1] "), "[\n  1\n]");
  assert.equal(jsonView("{not json"), null);
  assert.equal(jsonView("42"), null, "a bare scalar isn't worth a JSON view");
  assert.equal(jsonView("<p>hi</p>"), null);
  // Binary as text: UTF-8, bad bytes as U+FFFD, never thrown.
  assert.equal(bodyText("aGkg4pyTIP8="), "hi ✓ \uFFFD");
  assert.equal(bodyText("!!!not base64"), null);
});

test("matchesFilter checks method, host, path and status, case-insensitively", async () => {
  const { matchesFilter } = await import("./format.ts");
  const entry = { method: "POST", host: "API.example.test", url: "https://api.example.test/v1/login?x=1", status: 401 };
  for (const q of ["", "post", "api.example", "/v1/login", "x=1", "401", "POST api"]) {
    assert.ok(matchesFilter(entry, q.toLowerCase()), q);
  }
  for (const q of ["get", "example.com", "500", "fragment"]) {
    assert.ok(!matchesFilter(entry, q), q);
  }
  assert.ok(matchesFilter({ ...entry, status: null }, "err"), "ERR status is searchable");
});

test("methodClass maps to a fixed set; never echoes the (attacker-controlled) method", async () => {
  const { methodClass } = await import("./format.ts");
  for (const m of ["GET", "POST", "PUT", "PATCH", "DELETE"]) assert.equal(methodClass(m), `m-${m}`);
  assert.equal(methodClass("get"), "m-GET");
  for (const m of ["OPTIONS", "HEAD", "row open", "x\" onclick=\"y", ""]) assert.equal(methodClass(m), "m-OTHER");
});

test("statusLine adds the standard reason phrase when known", async () => {
  const { statusLine } = await import("./format.ts");
  assert.equal(statusLine(401), "401 Unauthorized");
  assert.equal(statusLine(200), "200 OK");
  assert.equal(statusLine(299), "299");
  assert.equal(statusLine(null), "ERR (no response)");
});

test("bodyBytes returns the captured bytes for either encoding", async () => {
  const { bodyBytes } = await import("./format.ts");
  assert.deepEqual(Array.from(bodyBytes("hé", "utf8")!), [0x68, 0xc3, 0xa9]);
  assert.deepEqual(Array.from(bodyBytes("AAEC/w==", "base64")!), [0, 1, 2, 255]);
  assert.equal(bodyBytes(null, "utf8"), null);
  assert.equal(bodyBytes("", "utf8"), null);
  assert.equal(bodyBytes("!!", "base64"), null);
});

test("shortId keeps the start and end of long ids", async () => {
  const { shortId } = await import("./format.ts");
  assert.equal(shortId("0b6c3e0e-5f53-4c39-9b7a-3f0f5c1a2d4e"), "0b6c3e0e…d4e");
  assert.equal(shortId("short-id"), "short-id");
});

test("displayUrl drops only the scheme's default port", async () => {
  const { displayUrl } = await import("./format.ts");
  assert.equal(displayUrl("https://httpbin.org:443/post?x=1"), "https://httpbin.org/post?x=1");
  assert.equal(displayUrl("http://example.com:80/"), "http://example.com/");
  assert.equal(displayUrl("https://example.com:8443/a"), "https://example.com:8443/a");
  assert.equal(displayUrl("http://example.com:443/"), "http://example.com:443/");
  assert.equal(displayUrl("/relative"), "/relative");
  assert.equal(displayUrl("not a url"), "not a url");
});

test("humanSize uses B, KB, MB with one decimal", async () => {
  const { humanSize } = await import("./format.ts");
  assert.equal(humanSize(64), "64 B");
  assert.equal(humanSize(1023), "1023 B");
  assert.equal(humanSize(49412), "48.3 KB");
  assert.equal(humanSize(5 * 1024 * 1024), "5.0 MB");
});
