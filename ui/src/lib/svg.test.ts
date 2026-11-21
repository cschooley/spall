import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { parseIcon } from "./svg.ts";

const vendor = new URL("../vendor/verdigris/", import.meta.url);

test("every vendored icon parses into shape elements", () => {
  for (const dir of ["icons", "icons/views"]) {
    for (const f of readdirSync(new URL(dir, vendor)).filter((n) => n.endsWith(".svg"))) {
      const icon = parseIcon(readFileSync(new URL(`${dir}/${f}`, vendor), "utf8"));
      assert.equal(icon.attrs.viewBox, "0 0 24 24", f);
      assert.ok(icon.children.length > 0, f);
    }
  }
});

test("spall's mark: attributes camel-cased for React, accent stroke kept, title dropped", () => {
  const icon = parseIcon(readFileSync(new URL("icons/spall.svg", vendor), "utf8"));
  assert.deepEqual(icon.attrs, {
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: "1.75",
    strokeLinecap: "round",
    strokeLinejoin: "round",
  });
  assert.deepEqual(icon.children, [
    { tag: "path", attrs: { d: "M4 4h10l-2 4 4 1-1 3h5v8H4z" } },
    { tag: "path", attrs: { d: "M17 4.5l2.5-1 1 2.5-2.5 1z", stroke: "var(--vg-icon-accent, currentColor)" } },
  ]);
});

test("anything beyond plain shapes is rejected, not passed through", () => {
  const wrap = (inner: string) => `<svg viewBox="0 0 24 24">${inner}</svg>`;
  for (const bad of [
    wrap("<script>alert(1)</script>"),
    wrap('<path d="M0 0" onload="alert(1)"/>'),
    wrap('<image href="https://x.example/a.png"/>'),
    wrap('<use href="#a"/>'),
    wrap('<path d="M0 0" style="fill:red"/>'),
    wrap('<foreignObject><div/></foreignObject>'),
    '<div viewBox="0 0 24 24"></div>',
  ]) {
    assert.throws(() => parseIcon(bad), /^Error: icon:/, bad);
  }
});
