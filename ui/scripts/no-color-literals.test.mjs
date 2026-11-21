// Colors come from verdigris tokens (--vg-*) only (CLAUDE.md). Fail on any
// color literal in spall's own UI source; the vendored tokens are exempt.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const root = new URL("../src", import.meta.url).pathname;
const LITERAL = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(/gi;

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return name === "vendor" ? [] : files(p);
    return /\.(css|tsx?)$/.test(name) && !/\.test\.ts$/.test(name) ? [p] : [];
  });
}

test("no color literals outside the vendored verdigris tokens", () => {
  const hits = [];
  for (const f of files(root)) {
    readFileSync(f, "utf8")
      .split("\n")
      .forEach((line, i) => {
        for (const m of line.matchAll(LITERAL)) hits.push(`${relative(root, f)}:${i + 1}: ${m[0]}`);
      });
  }
  assert.deepEqual(hits, []);
});
