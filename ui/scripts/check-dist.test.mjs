import { test } from "node:test";
import assert from "node:assert/strict";
import { checkHtml } from "./check-dist.mjs";

const ok = `<!doctype html><html><head>
<script type="module" crossorigin src="/assets/index-abc.js"></script>
<link rel="stylesheet" crossorigin href="/assets/index-def.css">
</head><body><div id="root"></div></body></html>`;

test("a Vite-style page passes", () => {
  assert.deepEqual(checkHtml(ok), []);
});

test("each CSP-breaking construct is reported", () => {
  const cases = {
    "<script>alert(1)</script>": "inline <script>",
    '<script src="/a.js">x()</script>': "inline content",
    "<style>body{}</style>": "inline <style>",
    '<div style="color:red"></div>': "style= attribute",
    '<img src="/x.png" onerror="x()">': "inline event handler",
    '<script src="https://cdn.example/x.js"></script>': "external URL",
    '<link href="//fonts.example/f.css" rel="stylesheet">': "external URL",
  };
  for (const [html, want] of Object.entries(cases)) {
    const problems = checkHtml(ok.replace("</body>", html + "</body>"));
    assert.ok(problems.some((p) => p.includes(want)), `${html} -> ${problems}`);
  }
});

test("inline scripts are caught however the end tag is written", () => {
  for (const html of [
    "<script>alert(1)</script >",
    "<SCRIPT>alert(1)</SCRIPT\n>",
    '<script>alert(1)</script foo="bar">',
    "<script>alert(1)", // no end tag: the browser runs it to end of document
  ]) {
    const problems = checkHtml(ok.replace("</body>", html + "</body>"));
    assert.ok(problems.some((p) => p.includes("inline <script>")), `${JSON.stringify(html)} -> ${problems}`);
  }
});
