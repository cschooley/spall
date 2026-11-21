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
  prettyBody,
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

test("prettyBody pretty-prints JSON, leaves text alone, truncates for display", () => {
  assert.deepEqual(prettyBody('{"a":1}', "application/json"), {
    text: '{\n  "a": 1\n}',
    truncated: false,
  });
  assert.equal(prettyBody("[1,2]", null).text, "[\n  1,\n  2\n]", "sniffed JSON");
  assert.equal(prettyBody("{not json", "application/json").text, "{not json");
  assert.equal(prettyBody("<h1>hi</h1>", "text/html").text, "<h1>hi</h1>");
  const big = prettyBody("x".repeat(300_000), "text/plain", 1000);
  assert.equal(big.truncated, true);
  assert.equal(big.text.length, 1000);
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
