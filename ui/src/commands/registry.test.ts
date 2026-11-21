import { test } from "node:test";
import assert from "node:assert/strict";
import { COMMANDS, complete, execute, paletteItems, tokenize, type Ctx, type OutKind } from "./registry.ts";

function fakeCtx(over: Partial<Ctx> = {}) {
  const lines: [string, OutKind][] = [];
  const calls: string[] = [];
  const ctx: Ctx = {
    out: (text, kind = "out") => void lines.push([text, kind]),
    setFilter: (q) => void calls.push(`filter:${q}`),
    focusFilter: () => void calls.push("focus"),
    openById: async (id) => (calls.push(`open:${id}`), id === "known"),
    selected: () => null,
    copyCurl: async () => (calls.push("curl"), true),
    curlText: () => null,
    jumpToNewest: () => void calls.push("newest"),
    clearTable: () => void calls.push("clear-table"),
    setTheme: (t) => void calls.push(`theme:${t}`),
    clearConsole: () => void calls.push("clear-console"),
    openKeys: () => void calls.push("keys"),
    setConsoleKey: (k) => (calls.push(`key:${k}`), k.length === 1),
    ...over,
  };
  return { ctx, lines, calls };
}

test("tokenize splits on whitespace and honors quotes", () => {
  assert.deepEqual(tokenize("open abc"), ["open", "abc"]);
  assert.deepEqual(tokenize(`  theme   blued-light `), ["theme", "blued-light"]);
  assert.deepEqual(tokenize(`filter "a b" 'c d' e`), ["filter", "a b", "c d", "e"]);
  assert.deepEqual(tokenize(""), []);
});

test("unknown commands warn and point at help", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("frobnicate now", ctx);
  assert.deepEqual(lines, [
    ["frobnicate now", "cmd"],
    ["unknown command: frobnicate. try help", "warn"],
  ]);
});

test("help lists every documented command, not god", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("help", ctx);
  const text = lines.map(([t]) => t).join("\n");
  for (const c of COMMANDS.filter((c) => !c.hidden)) assert.ok(text.includes(c.usage), c.name);
  assert.ok(!text.includes("god"));
});

test("filter takes the raw rest of the line (quotes and spacing kept); no argument clears", async () => {
  const { ctx, calls } = fakeCtx();
  await execute(`filter host:a.example  "login form"`, ctx);
  await execute("filter", ctx);
  assert.deepEqual(calls, [`filter:host:a.example  "login form"`, "filter:"]);
});

test("open validates the id before asking", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("open", ctx);
  await execute("open ../etc", ctx);
  await execute("open missing-1", ctx);
  await execute("open known", ctx);
  assert.deepEqual(calls, ["open:missing-1", "open:known"]);
  assert.deepEqual(lines.filter(([, k]) => k === "warn").map(([t]) => t), [
    "usage: open <id>",
    "open: not an exchange id: ../etc",
    "open: no exchange missing-1",
  ]);
});

test("theme accepts the four verdigris themes only", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("theme blued-light", ctx);
  await execute("theme neon", ctx);
  assert.deepEqual(calls, ["theme:blued-light"]);
  assert.match(lines.at(-1)![0], /^theme: rust \| rust-light \| blued \| blued-light$/);
});

test("goto: traffic is here; unbuilt views say so", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("goto traffic", ctx);
  await execute("goto findings", ctx);
  assert.equal(lines[1]![0], "already on traffic");
  assert.deepEqual(lines[3], ["goto: findings is not built yet", "warn"]);
});

test("curl needs a selected exchange", async () => {
  const none = fakeCtx();
  await execute("curl", none.ctx);
  assert.deepEqual(none.lines.at(-1), ["curl: select an exchange first", "warn"]);
  const some = fakeCtx({ curlText: () => "curl 'https://x.example/'" });
  await execute("curl", some.ctx);
  assert.deepEqual(some.lines.slice(1), [
    ["curl 'https://x.example/'", "out"],
    ["copied to the clipboard", "sys"],
  ]);
});

test("god answers no", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("god", ctx);
  assert.deepEqual(lines.at(-1), ["no.", "out"]);
});

test("tab completion: command names, then arguments", () => {
  assert.deepEqual(complete("th"), { line: "theme ", options: ["theme"] });
  assert.deepEqual(complete("theme blu"), { line: null, options: ["blued", "blued-light"] });
  assert.deepEqual(complete("theme blued-l"), { line: "theme blued-light ", options: ["blued-light"] });
  assert.deepEqual(complete("goto f"), { line: "goto findings ", options: ["findings"] });
  assert.ok(!complete("").options.includes("god"));
  assert.deepEqual(complete("zz").options, []);
});

test("the agent may never be offered export or promotion (MCP, M5)", () => {
  const exposed = COMMANDS.filter((c) => c.mcp).map((c) => c.name);
  for (const name of exposed) assert.ok(!/export|promote|confirm|reject/.test(name), name);
  assert.ok(exposed.includes("filter") && exposed.includes("open"));
});

test("palette items come from the registry; ones needing arguments go to the console", () => {
  const items = paletteItems();
  for (const it of items) assert.ok(COMMANDS.some((c) => c.name === it.command), it.label);
  const open = items.find((i) => i.command === "open")!;
  assert.equal(open.prefill, "open ");
});
