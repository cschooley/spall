import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_RULES } from "../lib/roe.ts";
import { asCandidate } from "../lib/candidates.ts";
import { RAW } from "../lib/candidates.fixture.ts";
import { COMMANDS, UNREGISTERED, VIEWS, complete, execute, paletteItems, tokenize, type Ctx, type OutKind } from "./registry.ts";
import { RISKS, SPECS } from "./specs.gen.ts";
import { THEMES } from "../lib/theme.ts";

const ENG = {
  id: "acme-q4-1a2b",
  name: "ACME Q4",
  client: "ACME",
  starts: "2026-11-23",
  ends: "2026-12-04",
  expires: null,
  scope: ["*.acme.test", "acme.test", "!pay.acme.test"],
  notes: "",
  targetNotes: {},
  apiSpecs: [],
  rules: DEFAULT_RULES,
  testingNow: { allowed: true, reason: null },
};

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
    replay: async (id, opts) =>
      void calls.push(`replay:${id}${opts.yes ? ":yes" : ""}${opts.edit ? `:${JSON.stringify(opts.edit)}` : ""}`),
    scope: () => ({ entries: ["app.example.test", "*.api.example.test"], all: false }),
    changeScope: async (c) => (calls.push(`scope:${JSON.stringify(c)}`), { scope: ["x.test"], all: c.op === "all" && c.on, changed: true }),
    audit: async (last) => (calls.push(`audit:${last}`), []),
    engagement: async () => ({ ...ENG }),
    listEngagements: async () => [
      { id: "acme-q4-1a2b", name: "ACME Q4", client: "ACME", active: true },
      { id: "beta-9f00", name: "Beta", client: "", active: false },
    ],
    newEngagement: async (name) => (calls.push(`new:${name}`), { ...ENG, id: "n-1", name, scope: [] }),
    useEngagement: async (w) => (calls.push(`use:${w}`), { ...ENG, id: "beta-9f00", name: "Beta" }),
    setEngagement: async (f, v) => (calls.push(`set:${f}=${JSON.stringify(v)}`), { engagement: { ...ENG, [f]: v || null }, changed: true }),
    setRules: async (r) => (calls.push(`rules:${JSON.stringify(r)}`), { engagement: { ...ENG, rules: { ...DEFAULT_RULES, ...r } as typeof DEFAULT_RULES }, changed: true }),
    openScopePaste: (what = "paste") => void calls.push(what),
    ca: async () => ({ sha256: "EB:6F:71", subject: "CN=taphttp CA", notBefore: null, notAfter: "2036-11-20T00:00:00Z", proxy: "127.0.0.1:8080", taphttpVersion: "0.6.1" }),
    openSetup: () => void calls.push("setup"),
    promote: async (id, undo) => (calls.push(`promote:${id}${undo ? ":undo" : ""}`), id === "c-1" ? { ...asCandidate(RAW)!, id, promoted: undo ? null : "now" } : `no candidate ${id}`),
    estop: async (a, reason) => (calls.push(`estop:${a}:${reason ?? ""}`), { ok: true, text: a === "clear" ? "cleared" : "engaged" }),
    exportFindings: async (u) => (calls.push(`export:${u}`), u ? { ok: false, text: "an unredacted export needs spall started with --allow-unredacted-export" } : { ok: true, text: "exported spall-x.sarif (redacted)" }),
    goto: (v) => (calls.push(`goto:${v}`), ["engagement", "traffic", "findings"].includes(v)),
    flag: async (i) => {
      calls.push(`flag:${JSON.stringify(i)}`);
      return { kind: "created", candidate: asCandidate(RAW)! };
    },
    rules: async () => ({
      wstg: "4.2",
      attribution: "OWASP WSTG v4.2 (CC BY-SA 4.0)",
      rules: [
        { id: "WSTG-INPV-05", title: "Testing for SQL Injection", cwe: 89, help: null },
        { id: "WSTG-INPV-01", title: "Testing for Reflected Cross Site Scripting", cwe: 79, help: null },
        { id: "spall/other", title: "Other (not in the catalog)", cwe: null, help: null },
      ],
    }),
    hosts: async () => ({
      hosts: [
        { host: "app.acme.test", count: 12, firstSeen: "", lastSeen: "", state: "in" as const, by: "*.acme.test" },
        { host: "www.google-analytics.com", count: 4, firstSeen: "", lastSeen: "", state: "unknown" as const, by: null },
        { host: "pay.acme.test", count: 1, firstSeen: "", lastSeen: "", state: "out" as const, by: "!pay.acme.test" },
      ],
      counts: { in: 1, out: 1, unknown: 1 },
      dropped: 0,
    }),
    checkScope: async (t) => (calls.push(`check:${t}`), { host: t, state: "unknown", by: null }),
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

test("goto shows a built view; unbuilt ones say so", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("goto findings", ctx);
  await execute("goto tasks", ctx);
  await execute("goto nowhere", ctx);
  assert.deepEqual(calls, ["goto:findings", "goto:tasks"]);
  assert.equal(lines.length, 5, "a built view needs no reply");
  assert.deepEqual(lines[2], ["goto: tasks is not built yet", "warn"]);
  assert.equal(lines[4]![1], "warn");
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

test("replay takes an id or the selected exchange, and --yes", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("replay abc-1", ctx);
  await execute("replay --yes abc-1", ctx);
  await execute("replay abc-1 --yes", ctx);
  assert.deepEqual(calls, ["replay:abc-1", "replay:abc-1:yes", "replay:abc-1:yes"]);

  const none = fakeCtx();
  await execute("replay", none.ctx);
  assert.deepEqual(none.lines.at(-1), ["usage: replay <id> [--yes] (or select an exchange)", "warn"]);
  const sel = fakeCtx({ selected: () => ({ id: "sel-1" }) as never });
  await execute("replay", sel.ctx);
  assert.deepEqual(sel.calls, ["replay:sel-1"]);

  for (const bad of ["replay ../x", "replay a b", "replay a --force"]) {
    const t = fakeCtx();
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
  assert.equal(lines.length, 3, "replay's own output comes from ctx.replay");
});

test("scope lists the engagement scope, or says replay is unavailable", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("scope", ctx);
  assert.deepEqual(lines.slice(1), [["scope: app.example.test, *.api.example.test", "out"]]);
  const empty = fakeCtx({ scope: () => ({ entries: [], all: false }) });
  await execute("scope", empty.ctx);
  assert.equal(empty.lines.at(-1)![1], "warn");
  assert.match(empty.lines.at(-1)![0], /no scope.*scope add.*SPALL_SCOPE/);
  const off = fakeCtx({ scope: () => null });
  await execute("scope", off.ctx);
  assert.match(off.lines.at(-1)![0], /lever API/);
});

test("audit prints the newest attempts, one line each", async () => {
  const records = [
    { ts: "2026-11-21T20:00:01.5Z", outcome: "sent", method: "GET", target: "http://a.test/x", exchange_id: "e1", result_id: "r1" },
    { ts: "2026-11-21T20:00:02Z", outcome: "denied", method: "GET", target: "http://b.test/", exchange_id: "e2", reason: '"b.test" is not in scope' },
    { ts: "2026-11-21T20:00:03Z", outcome: "sent", method: "DELETE", target: "http://a.test/", exchange_id: "e3", confirmed: true },
  ];
  const { ctx, lines, calls } = fakeCtx({ audit: async (n) => (calls.push(`audit:${n}`), records) });
  await execute("audit --last 3", ctx);
  assert.deepEqual(calls, ["audit:3"]);
  assert.equal(lines.length, 4);
  assert.match(lines[1]![0], /20:00:01 +sent +GET +http:\/\/a\.test\/x/);
  assert.deepEqual(lines[2]![1], "warn", "denials stand out");
  assert.match(lines[2]![0], /not in scope/);
  assert.match(lines[3]![0], /confirmed/);

  const t = fakeCtx();
  await execute("audit", t.ctx);
  assert.deepEqual(t.calls, ["audit:20"]);
  assert.match(t.lines.at(-1)![0], /no replay attempts yet/);
  const bad = fakeCtx();
  await execute("audit --last x", bad.ctx);
  assert.equal(bad.calls.length, 0);
  const err = fakeCtx({ audit: async () => "the audit log needs taphttp's lever API" });
  await execute("audit", err.ctx);
  assert.deepEqual(err.lines.at(-1), ["audit: the audit log needs taphttp's lever API", "warn"]);
});

test("every command has a risk class; replay sends traffic and scope changes policy", () => {
  for (const c of COMMANDS) assert.ok((RISKS as readonly string[]).includes(c.risk), c.name);
  assert.deepEqual(
    COMMANDS.filter((c) => c.risk !== "read").map((c) => [c.name, c.risk]),
    [
      ["replay", "send"],
      ["scope", "policy"],
      ["engagement", "policy"],
      ["roe", "policy"],
      ["estop", "policy"],
      ["flag", "record"],
      ["promote", "export"],
      ["export", "export"],
    ],
  );
  assert.ok(COMMANDS.filter((c) => c.risk === "policy").every((c) => !c.mcp), "agents never change the rules");
  const item = paletteItems().find((i) => i.command === "replay")!;
  assert.equal(item.label, "Replay exchange…");
  assert.equal(item.prefill, "replay ");
});

test("replay edits from the console: --method, --header, --drop-header, --body", async () => {
  const { ctx, calls } = fakeCtx();
  await execute(`replay a1 --method PUT --header 'X-Role: admin' --header "Cookie:sid=2" --drop-header referer --body '{"qty": 2}' --yes`, ctx);
  assert.equal(calls.length, 1);
  const [, id, yes, edit] = calls[0]!.match(/^replay:([^:]+):(yes):(.*)$/)!;
  assert.equal(id, "a1");
  assert.equal(yes, "yes");
  assert.deepEqual(JSON.parse(edit!), {
    method: "PUT",
    sets: [["X-Role", "admin"], ["Cookie", "sid=2"]],
    drops: ["referer"],
    body: '{"qty": 2}',
  });

  for (const bad of [
    "replay a1 --method",
    "replay a1 --header novalue",
    "replay a1 --header 'bad name: v'",
    "replay a1 --method 'G ET'",
    "replay a1 --drop-header",
    "replay a1 --body",
  ]) {
    const t = fakeCtx();
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
});

test("scope add, remove and all on|off change the scope; bad forms warn", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("scope add *.whatever.com", ctx);
  await execute("scope remove app.example.test", ctx);
  await execute("scope all on", ctx);
  await execute("scope all off", ctx);
  assert.deepEqual(calls, [
    'scope:{"op":"add","entry":"*.whatever.com"}',
    'scope:{"op":"remove","entry":"app.example.test"}',
    'scope:{"op":"all","on":true}',
    'scope:{"op":"all","on":false}',
  ]);
  assert.ok(lines.some(([t, k]) => /every host/.test(t) && k === "warn"), "all on warns about what it means");
  assert.ok(lines.some(([t]) => /scope: x\.test/.test(t)), "prints the scope after a change");

  for (const bad of ["scope add", "scope all", "scope all maybe", "scope frob x", "scope add a b"]) {
    const t = fakeCtx();
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
  const refused = fakeCtx({ changeScope: async () => 'scope entry "http://x": not a hostname' });
  await execute("scope add http://x", refused.ctx);
  assert.deepEqual(refused.lines.at(-1), ['scope: scope entry "http://x": not a hostname', "warn"]);
  const all = fakeCtx({ scope: () => ({ entries: ["a.test", "*"], all: true }) });
  await execute("scope", all.ctx);
  assert.ok(all.lines.some(([t, k]) => /every host/.test(t) && k === "warn"));
});

test("scope exclude and scope check; exclusions are listed apart", async () => {
  const { ctx, lines, calls } = fakeCtx({
    changeScope: async (c) => (calls.push(`scope:${JSON.stringify(c)}`), { scope: ["*.a.test", "!sso.a.test"], all: false, changed: true }),
    engagement: async () => ({ ...ENG }),
    listEngagements: async () => [
      { id: "acme-q4-1a2b", name: "ACME Q4", client: "ACME", active: true },
      { id: "beta-9f00", name: "Beta", client: "", active: false },
    ],
    newEngagement: async (name) => (calls.push(`new:${name}`), { ...ENG, id: "n-1", name, scope: [] }),
    useEngagement: async (w) => (calls.push(`use:${w}`), { ...ENG, id: "beta-9f00", name: "Beta" }),
    setEngagement: async (f, v) => (calls.push(`set:${f}=${JSON.stringify(v)}`), { engagement: { ...ENG, [f]: v || null }, changed: true }),
    setRules: async (r) => (calls.push(`rules:${JSON.stringify(r)}`), { engagement: { ...ENG, rules: { ...DEFAULT_RULES, ...r } as typeof DEFAULT_RULES }, changed: true }),
    openScopePaste: (what = "paste") => void calls.push(what),
    ca: async () => ({ sha256: "EB:6F:71", subject: "CN=taphttp CA", notBefore: null, notAfter: "2036-11-20T00:00:00Z", proxy: "127.0.0.1:8080", taphttpVersion: "0.6.1" }),
    openSetup: () => void calls.push("setup"),
    hosts: async () => ({
      hosts: [
        { host: "app.acme.test", count: 12, firstSeen: "", lastSeen: "", state: "in" as const, by: "*.acme.test" },
        { host: "www.google-analytics.com", count: 4, firstSeen: "", lastSeen: "", state: "unknown" as const, by: null },
        { host: "pay.acme.test", count: 1, firstSeen: "", lastSeen: "", state: "out" as const, by: "!pay.acme.test" },
      ],
      counts: { in: 1, out: 1, unknown: 1 },
      dropped: 0,
    }),
    checkScope: async (t) => (calls.push(`check:${t}`), { host: "sso.a.test", state: "out", by: "!sso.a.test" }),
  });
  await execute("scope exclude sso.a.test", ctx);
  assert.deepEqual(calls, ['scope:{"op":"exclude","entry":"sso.a.test"}']);
  assert.deepEqual(lines.slice(-2), [
    ["scope: *.a.test", "out"],
    ["excluded: sso.a.test (wins over the scope)", "out"],
  ]);
  await execute("scope remove !sso.a.test", ctx);
  assert.equal(calls.at(-1), 'scope:{"op":"remove","entry":"!sso.a.test"}');

  await execute("scope check https://SSO.a.test/login", ctx);
  assert.equal(calls.at(-1), "check:https://SSO.a.test/login");
  assert.deepEqual(lines.at(-1), ["sso.a.test: out of scope (excluded by !sso.a.test)", "warn"]);
  const inn = fakeCtx({ checkScope: async () => ({ host: "x.a.test", state: "in", by: "*.a.test" }) });
  await execute("scope check x.a.test", inn.ctx);
  assert.deepEqual(inn.lines.at(-1), ["x.a.test: in scope (by *.a.test)", "out"]);
  const unk = fakeCtx();
  await execute("scope check c.test", unk.ctx);
  assert.deepEqual(unk.lines.at(-1), ["c.test: not in scope (no entry matches; scope add c.test, or scope exclude c.test)", "warn"]);
  const err = fakeCtx({ checkScope: async () => "that URL has no host" });
  await execute("scope check http://", err.ctx);
  assert.deepEqual(err.lines.at(-1), ["scope check: that URL has no host", "warn"]);
  for (const bad of ["scope check", "scope check a b", "scope exclude"]) {
    const t = fakeCtx();
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
  assert.deepEqual(complete("scope ").options, ["add", "exclude", "remove", "check", "paste", "import", "all"]);
  const paste = fakeCtx();
  await execute("scope paste", paste.ctx);
  await execute("scope import", paste.ctx);
  assert.deepEqual(paste.calls, ["paste", "import"]);
});

test("hostile hosts from scope check are printed as given, as text", async () => {
  const p = `<img src=x onerror=alert(1)>`;
  const { ctx, lines } = fakeCtx({ checkScope: async () => ({ host: p, state: "unknown", by: null }) });
  await execute("scope check x", ctx);
  assert.ok(lines.at(-1)![0].startsWith(`${p}: not in scope`));
});

test("engagement shows the active engagement; list, new, use and set change it", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("engagement", ctx);
  assert.deepEqual(lines.slice(1), [
    ["engagement: ACME Q4 (acme-q4-1a2b) · client ACME", "out"],
    ["dates: 2026-11-23 → 2026-12-04 · authorization expiry not set", "out"],
    ["scope: 2 entries, 1 excluded (scope lists them)", "out"],
  ]);
  lines.length = 0;
  await execute("engagement list", ctx);
  assert.deepEqual(lines.slice(1), [
    ["* ACME Q4 (acme-q4-1a2b) · ACME", "out"],
    ["  Beta (beta-9f00)", "out"],
  ]);
  await execute(`engagement new "Gamma web"`, ctx);
  assert.match(lines.at(-1)![0], /created Gamma web \(n-1\), now active.*scope add/);
  await execute("engagement use Beta", ctx);
  assert.equal(lines.at(-1)![0], "engagement: now Beta (beta-9f00)");
  await execute("engagement set client ACME Corp", ctx);
  await execute(`engagement set expires ""`, ctx);
  assert.deepEqual(calls, ["new:Gamma web", "use:Beta", 'set:client="ACME Corp"', 'set:expires=""']);
  assert.equal(lines.at(-1)![0], "engagement: expires cleared");

  for (const bad of ["engagement set", "engagement set colour red", "engagement new", "engagement use", "engagement frob"]) {
    const t = fakeCtx();
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
  const err = fakeCtx({ setEngagement: async () => "ends: before starts" });
  await execute("engagement set ends 2026-01-01", err.ctx);
  assert.deepEqual(err.lines.at(-1), ["engagement: ends: before starts", "warn"]);
  const off = fakeCtx({ engagement: async () => "engagements need taphttp's lever API" });
  await execute("engagement", off.ctx);
  assert.equal(off.lines.at(-1)![1], "warn");
  const eng = COMMANDS.find((c) => c.name === "engagement")!;
  assert.equal(eng.risk, "policy");
  assert.equal(eng.mcp, false, "agents may read the engagement later, never change it");
  assert.deepEqual(complete("engagement set ").options, ["name", "client", "starts", "ends", "expires", "notes"]);
});

test("roe shows the rules and whether testing is allowed now", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("roe", ctx);
  assert.deepEqual(lines.slice(1).map(([t]) => t), [
    "methods: any · confirm all but GET, HEAD, OPTIONS · rate 10/s · time zone UTC",
    "testing windows: any time",
    "no-test windows: none",
    "agents: not allowed · destructive actions: not allowed",
    "testing now: allowed",
  ]);
  const closed = fakeCtx({ engagement: async () => ({ ...ENG, testingNow: { allowed: false, reason: "outside the testing windows (...)" } }) });
  await execute("roe", closed.ctx);
  assert.deepEqual(closed.lines.at(-1), ["testing now: not allowed: outside the testing windows (...)", "warn"]);
});

test("roe set, window and blackout build rule changes", async () => {
  const withWindows = {
    ...ENG,
    rules: { ...DEFAULT_RULES, windows: [{ days: ["sat" as const], start: "10:00", end: "12:00" }], blackouts: [{ from: "2026-11-26T18:00", to: "2026-11-27T08:00", note: "" }] },
  };
  const { ctx, calls, lines } = fakeCtx({ engagement: async () => withWindows });
  for (const cmd of [
    "roe set methods get,post",
    "roe set methods any",
    "roe set safe GET",
    "roe set rate 2.5",
    "roe set rate off",
    "roe set tz America/Chicago",
    "roe set agents yes",
    "roe set destructive no",
    "roe window add mon-fri 09:00-17:00",
    "roe window remove 1",
    "roe window clear",
    "roe blackout add 2026-12-24T00:00 2026-12-27T00:00 holiday freeze",
    "roe blackout remove 1",
  ]) await execute(cmd, ctx);
  assert.deepEqual(calls, [
    'rules:{"methods":["GET","POST"]}',
    'rules:{"methods":[]}',
    'rules:{"safe_methods":["GET"]}',
    'rules:{"rate_limit":2.5}',
    'rules:{"rate_limit":0}',
    'rules:{"time_zone":"America/Chicago"}',
    'rules:{"agents_allowed":true}',
    'rules:{"destructive_allowed":false}',
    'rules:{"windows":[{"days":["sat"],"start":"10:00","end":"12:00"},{"days":["mon","tue","wed","thu","fri"],"start":"09:00","end":"17:00"}]}',
    'rules:{"windows":[]}',
    'rules:{"windows":[]}',
    'rules:{"blackouts":[{"from":"2026-11-26T18:00","to":"2026-11-27T08:00","note":""},{"from":"2026-12-24T00:00","to":"2026-12-27T00:00","note":"holiday freeze"}]}',
    'rules:{"blackouts":[]}',
  ]);
  assert.ok(lines.some(([t]) => t.startsWith("testing windows:")), "prints the rules after a change");

  for (const bad of [
    "roe set rate fast",
    "roe set rate -1",
    "roe set agents maybe",
    "roe set colour red",
    "roe set methods G@T",
    "roe window add funday 09:00-17:00",
    "roe window add mon 9-5",
    "roe window remove 7",
    "roe blackout add 2026-12-24 2026-12-27",
    "roe frob",
  ]) {
    const t = fakeCtx({ engagement: async () => withWindows });
    await execute(bad, t.ctx);
    assert.equal(t.calls.length, 0, bad);
    assert.equal(t.lines.at(-1)![1], "warn", bad);
  }
  const refused = fakeCtx({ setRules: async () => 'rules of engagement: time zone "Mars/Olympus": not an IANA zone' });
  await execute("roe set tz Mars/Olympus", refused.ctx);
  assert.equal(refused.lines.at(-1)![1], "warn");
  assert.deepEqual(complete("roe ").options, ["set", "window", "blackout"]);
  assert.deepEqual(complete("roe set ").options, ["methods", "safe", "rate", "tz", "agents", "destructive"]);
});

test("hosts lists what traffic has touched, with each host's scope state", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("hosts", ctx);
  assert.deepEqual(lines.slice(1), [
    ["hosts: 1 in scope · 1 excluded · 1 undecided", "out"],
    ["  in        12  app.acme.test  (*.acme.test)", "out"],
    ["  ?          4  www.google-analytics.com", "warn"],
    ["  out        1  pay.acme.test  (!pay.acme.test)", "out"],
  ]);
  lines.length = 0;
  await execute("hosts --unknown", ctx);
  assert.deepEqual(lines.slice(1).map(([t]) => t), ["hosts: 1 in scope · 1 excluded · 1 undecided", "  ?          4  www.google-analytics.com", "decide them in the Engagement view, or: scope add|exclude <host>"]);
  const t = fakeCtx();
  await execute("hosts --nope", t.ctx);
  assert.equal(t.lines.at(-1)![1], "warn");
  const h = COMMANDS.find((c) => c.name === "hosts")!;
  assert.equal(h.risk, "read");
});

test("ca shows the CA's fingerprint and where to get it, and opens browser setup", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("ca", ctx);
  assert.deepEqual(lines.slice(1).map(([t]) => t), [
    "ca: CN=taphttp CA, valid until 2036-11-20T00:00:00Z",
    "sha256: EB:6F:71",
    "download: /api/ca.pem (or /api/ca.der); devices on the proxy: http://taphttp/",
    "browser setup steps are in the Engagement view",
  ]);
  assert.deepEqual(calls, ["setup"]);
  const off = fakeCtx({ ca: async () => "the CA certificate comes from taphttp's lever API" });
  await execute("ca", off.ctx);
  assert.equal(off.lines.at(-1)![1], "warn");
  assert.equal(COMMANDS.find((c) => c.name === "ca")!.risk, "read");
});

test("rules lists the catalog, or what matches", async () => {
  const { ctx, lines } = fakeCtx();
  await execute("rules sql", ctx);
  assert.deepEqual(lines.slice(1), [
    ["  WSTG-INPV-05 Testing for SQL Injection (CWE-89)", "out"],
    ["1 of 3 rules · OWASP WSTG v4.2 (CC BY-SA 4.0)", "sys"],
  ]);
  const all = fakeCtx();
  await execute("rules", all.ctx);
  assert.equal(all.lines.length, 5);
  const none = fakeCtx();
  await execute("rules nope", none.ctx);
  assert.deepEqual(none.lines.slice(1), [['rules: nothing matches "nope"', "warn"]]);
  const r = COMMANDS.find((c) => c.name === "rules")!;
  assert.equal(r.mcp, true);
  assert.equal(r.risk, "read");
});

test("flag files a candidate; a match says where it stands; never MCP", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("flag e1 --rule WSTG-INPV-05 --param sort --level error", ctx);
  assert.deepEqual(calls, ['flag:{"exchange":"e1","rule":"WSTG-INPV-05","parameter":"sort","level":"error"}']);
  assert.deepEqual(lines.slice(1).map(([t]) => t), ["flagged c-0a1b2c3d: WSTG-INPV-05 · GET https://shop.test/api/p/17 · parameter sort", "waiting for triage in Findings (5)"]);
  const rejected = asCandidate({ ...RAW, finding: { ...RAW.finding, suppressions: [{ kind: "external", status: "accepted", justification: "output is encoded", properties: { "flux/disposition": "falsePositive" } }] } })!;
  const dup = fakeCtx({ flag: async () => ({ kind: "exists", candidate: rejected, message: "already" }) });
  await execute("flag e1 --rule WSTG-INPV-05", dup.ctx);
  assert.deepEqual(dup.lines[1], ["flag: already flagged as c-0a1b2c3d (WSTG-INPV-05, rejected): output is encoded", "warn"]);
  const bad = fakeCtx();
  await execute("flag e1", bad.ctx);
  assert.equal(bad.lines[1]![1], "warn");
  assert.deepEqual(bad.calls, []);
  const f = COMMANDS.find((c) => c.name === "flag")!;
  assert.equal(f.mcp, false);
});

test("promote and export are a human's: never MCP", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("promote c-1", ctx);
  await execute("promote c-1 --undo", ctx);
  await execute("promote c-9", ctx);
  await execute("promote", ctx);
  await execute("export", ctx);
  await execute("export --unredacted", ctx);
  await execute("export --nope", ctx);
  assert.deepEqual(calls, ["promote:c-1", "promote:c-1:undo", "promote:c-9", "export:false", "export:true"]);
  const out = lines.filter(([, k]) => k !== "cmd");
  assert.deepEqual(out, [
    ["c-1 promoted: in the next export", "out"],
    ["c-1 is no longer promoted", "out"],
    ["promote: no candidate c-9", "warn"],
    ["usage: promote <candidate-id> [--undo]", "warn"],
    ["exported spall-x.sarif (redacted)", "out"],
    ["export: an unredacted export needs spall started with --allow-unredacted-export", "warn"],
    ["usage: export [--unredacted]", "warn"],
  ]);
  for (const n of ["promote", "export", "flag"]) assert.equal(COMMANDS.find((c) => c.name === n)!.mcp, false, n);
});

test("the UI and the server's registry (#53) list the same commands, with the server's risk and mcp", () => {
  assert.deepEqual(UNREGISTERED, [], "implemented here but not in src/commands.rs");
  assert.deepEqual(
    COMMANDS.map((c) => c.name),
    SPECS.map((s) => s.name),
    "in src/commands.rs but not implemented here",
  );
  for (const s of SPECS) {
    const c = COMMANDS.find((c) => c.name === s.name)!;
    assert.deepEqual([c.risk, c.mcp, c.hidden, c.usage, c.desc], [s.risk, s.mcp, s.hidden, s.usage, s.desc], s.name);
  }
  assert.deepEqual([...RISKS], ["read", "record", "send", "unsafe", "destructive", "policy", "export"]);
  for (const c of COMMANDS.filter((c) => c.risk === "policy" || c.risk === "export")) assert.equal(c.mcp, false, c.name);
});

test("goto and theme take what the UI has (the server's list names them too)", () => {
  const goto = COMMANDS.find((c) => c.name === "goto")!;
  const theme = COMMANDS.find((c) => c.name === "theme")!;
  assert.equal(goto.desc, VIEWS.join(" | "));
  assert.equal(theme.desc, THEMES.join(" | "));
  const enumOf = (c: (typeof COMMANDS)[number], k: string) => (c.args.properties as Record<string, { enum?: readonly string[] }>)[k]?.enum;
  assert.deepEqual(enumOf(goto, "view"), [...VIEWS]);
  assert.deepEqual(enumOf(theme, "name"), [...THEMES]);
});

test("estop engages with a reason, clear clears; an agent is never offered it", async () => {
  const { ctx, lines, calls } = fakeCtx();
  await execute("estop odd 500s", ctx);
  await execute("estop clear", ctx);
  await execute("estop clear now", ctx);
  assert.deepEqual(calls.filter((c) => c.startsWith("estop")), ["estop:engage:odd 500s", "estop:clear:"]);
  const said = lines.filter(([, k]) => k !== "cmd");
  assert.deepEqual(said, [
    ["engaged", "stop"],
    ["cleared", "out"],
    ["usage: estop [reason] | estop clear", "warn"],
  ]);
  const e = COMMANDS.find((c) => c.name === "estop")!;
  assert.equal(e.mcp, false);
  assert.ok(paletteItems().some((i) => i.command === "estop" && !i.prefill), "one press from the palette");
});
