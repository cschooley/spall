// The untrusted-content bans must actually fire. A config refactor that
// silently drops them would otherwise go unnoticed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ESLint } from "eslint";

const eslint = new ESLint({ cwd: new URL("..", import.meta.url).pathname });

async function ruleIds(code, filePath = "src/probe.tsx") {
  const [result] = await eslint.lintText(code, { filePath });
  return result.messages.map((m) => m.ruleId);
}

test("dangerouslySetInnerHTML is an error", async () => {
  const ids = await ruleIds(
    "export const X = (p: { h: string }) => <div dangerouslySetInnerHTML={{ __html: p.h }} />;\n",
  );
  assert.ok(ids.includes("no-restricted-syntax"), String(ids));
});

test("innerHTML, outerHTML and insertAdjacentHTML are errors", async () => {
  for (const code of [
    "export function f(e: HTMLElement, s: string) { e.innerHTML = s; }\n",
    "export function f(e: HTMLElement, s: string) { e.outerHTML = s; }\n",
    "export function f(e: HTMLElement, s: string) { e.insertAdjacentHTML('beforeend', s); }\n",
  ]) {
    const ids = await ruleIds(code, "src/probe.ts");
    assert.ok(ids.includes("no-restricted-properties"), `${code} -> ${ids}`);
  }
});

test("textContent is fine", async () => {
  const ids = await ruleIds("export function f(e: HTMLElement, s: string) { e.textContent = s; }\n", "src/probe.ts");
  assert.deepEqual(ids, []);
});
