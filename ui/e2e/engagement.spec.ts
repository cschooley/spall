// The Engagement view (#37), against an in-memory engagement API: the
// browser test runs spall in log mode, so /api/engagement* is mocked with
// state that survives a reload.
import { expect, test } from "./fixtures.ts";
import type { Page } from "@playwright/test";

type Json = Record<string, unknown>;

const days = (n: number) => new Date(Date.now() + n * 86_400_000).toISOString().slice(0, 10);

function engagementApi(page: Page, over: Json = {}) {
  const state = {
    e: {
      id: "acme-1a2b",
      name: "ACME Q4",
      client: "ACME",
      starts: days(-3),
      ends: days(30),
      expires: days(2),
      contacts: [],
      notes: "",
      scope: ["*.acme.test", "!pay.acme.test"],
      target_notes: {} as Record<string, string>,
      rules: { methods: [], safe_methods: ["GET", "HEAD", "OPTIONS"], rate_limit: 10, time_zone: "UTC", windows: [] as Json[], blackouts: [] as Json[], agents_allowed: false, destructive_allowed: false },
      ...over,
    } as Json & { scope: string[]; rules: Json; target_notes: Record<string, string> },
    others: [{ id: "beta-9f00", name: "Beta", client: "", active: false }],
    testing: { allowed: true, reason: null as string | null },
    calls: [] as { method: string; path: string; body: unknown }[],
  };
  const answer = () => ({ ...state.e, testing_now: state.testing, scope_all: false });
  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: state.e.scope, scope_all: false, engagement: { id: state.e.id, name: state.e.name }, testing: state.testing },
    }),
  );
  void page.route("**/api/engagement", async (r) => {
    if (r.request().method() === "PATCH") {
      const body = r.request().postDataJSON() as Json;
      expect(r.request().headers()["content-type"]).toBe("application/json");
      state.calls.push({ method: "PATCH", path: "/api/engagement", body });
      if (body.ends === "1999-01-01") return r.fulfill({ status: 400, json: { error: "ends: before starts" } });
      const { rules, ...rest } = body;
      Object.assign(state.e, rest);
      if (rules) Object.assign(state.e.rules, rules as Json);
      return r.fulfill({ json: { engagement: answer(), changed: true } });
    }
    return r.fulfill({ json: answer() });
  });
  void page.route("**/api/engagements", (r) => r.fulfill({ json: [{ id: state.e.id, name: state.e.name, client: state.e.client, active: true }, ...state.others] }));
  void page.route("**/api/scope", async (r) => {
    const c = r.request().postDataJSON() as { op: string; entry: string; add?: string[]; exclude?: string[] };
    state.calls.push({ method: "POST", path: "/api/scope", body: c });
    if (c.op === "add") state.e.scope = [c.entry, ...state.e.scope];
    if (c.op === "exclude") state.e.scope = [...state.e.scope, `!${c.entry}`];
    if (c.op === "remove") state.e.scope = state.e.scope.filter((x) => x !== c.entry);
    if (c.op === "apply") state.e.scope = [...state.e.scope, ...(c.add ?? []), ...(c.exclude ?? []).map((x) => `!${x}`)];
    await r.fulfill({ json: { scope: state.e.scope, all: false, changed: true } });
  });
  void page.route("**/api/ca", (r) =>
    r.fulfill({ json: { sha256: "AA:BB", subject: "CN=taphttp CA", not_before: null, not_after: "2036-11-20T00:00:00Z", proxy: "127.0.0.1:8080" } }),
  );
  void page.route("**/api/scope/check?*", (r) => r.fulfill({ json: { host: "pay.acme.test", state: "out", by: "!pay.acme.test" } }));
  return state;
}

test("1 opens the Engagement view and 2 goes back; the rail marks the current view", async ({ page }) => {
  engagementApi(page);
  await page.goto("/");
  await expect(page.locator(".rail [data-view='traffic']")).toHaveAttribute("aria-current", "page");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("1");
  await expect(page).toHaveURL(/#engagement$/);
  await expect(page.locator(".rail [data-view='engagement']")).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "ACME Q4" })).toBeVisible();
  await expect(page.locator(".eng-head .meta")).toContainText("authorization expires in 2 days");
  await expect(page.locator(".chip.testing")).toHaveText("testing allowed now");
  // Traffic's keys do nothing here.
  await page.keyboard.press("/");
  await expect(page.getByLabel("Filter")).toHaveCount(0);
  await page.keyboard.press("2");
  await expect(page.getByLabel("Filter")).toBeVisible();
  await page.getByRole("link", { name: "Engagement" }).click();
  await expect(page.getByRole("heading", { name: "ACME Q4" })).toBeVisible();
});

test("details, scope and rules edit through the API and persist across a reload", async ({ page }) => {
  const api = engagementApi(page);
  await page.goto("/#engagement");
  const details = page.getByRole("region", { name: "Engagement details" });
  await details.getByRole("button", { name: "edit" }).click();
  await details.getByLabel("Name").fill("ACME Q4 retest");
  await details.getByLabel("Notes").fill("VPN: use the jump host\nNo DoS");
  await details.getByRole("button", { name: "save" }).click();
  await expect(page.getByRole("heading", { name: "ACME Q4 retest" })).toBeVisible();
  await expect(page.locator(".statusbar .engagement-name")).toHaveText("ACME Q4 retest");

  // A refused change shows why and keeps the form open.
  await details.getByRole("button", { name: "edit" }).click();
  await details.getByLabel("Ends").fill("1999-01-01");
  await details.getByRole("button", { name: "save" }).click();
  await expect(details.getByRole("alert")).toHaveText("ends: before starts");
  await details.getByRole("button", { name: "cancel" }).click();

  const scope = page.getByRole("region", { name: "Scope" });
  await scope.getByLabel("Scope entry").fill("api.acme.test");
  await scope.getByRole("button", { name: "add", exact: true }).click();
  await expect(scope.getByRole("list", { name: "In scope" })).toContainText("api.acme.test");
  await scope.getByLabel("Scope entry").fill("sso.acme.test");
  await scope.getByRole("button", { name: "exclude" }).click();
  await expect(scope.getByRole("list", { name: "Excluded" })).toContainText("sso.acme.test");
  await scope.getByRole("button", { name: "Remove pay.acme.test" }).click();
  await expect(scope.getByRole("list", { name: "Excluded" })).not.toContainText("pay.acme.test");
  await scope.getByRole("button", { name: "Note on api.acme.test" }).click();
  await scope.getByLabel("Note for api.acme.test").fill("staging only");
  await scope.getByRole("button", { name: "save" }).click();
  await expect(scope.getByRole("list", { name: "In scope" })).toContainText("staging only");
  await scope.getByLabel("Check a URL or host").fill("https://pay.acme.test/");
  await scope.getByRole("button", { name: "check" }).click();
  await expect(scope.getByRole("status")).toHaveText("pay.acme.test: out of scope (excluded by !pay.acme.test)");

  const rules = page.getByRole("region", { name: "Rules of engagement" });
  await rules.getByRole("button", { name: "edit" }).click();
  await rules.getByLabel("Rate ceiling (per second)").fill("2");
  await rules.getByLabel("Time zone").fill("America/Chicago");
  await rules.getByLabel("Agents may act").check();
  await rules.getByRole("button", { name: "save" }).click();
  await expect(rules).toContainText("2 per second");
  await rules.getByLabel("Window days").fill("sat,sun");
  await rules.getByLabel("Window times").fill("10:00-14:00");
  await rules.getByRole("button", { name: "add window" }).click();
  await expect(rules.getByRole("list", { name: "Testing windows" })).toContainText("sat,sun 10:00-14:00");
  await rules.getByLabel("No-test from").fill("2026-12-24T00:00");
  await rules.getByLabel("No-test to").fill("2026-12-27T00:00");
  await rules.getByLabel("No-test note").fill("holiday freeze");
  await rules.getByRole("button", { name: "add no-test window" }).click();
  await expect(rules.getByRole("list", { name: "No-test windows" })).toContainText("2026-12-24 00:00 → 2026-12-27 00:00holiday freeze");

  expect(api.calls.map((c) => [c.path, c.body])).toEqual([
    ["/api/engagement", { name: "ACME Q4 retest", notes: "VPN: use the jump host\nNo DoS" }],
    ["/api/engagement", { ends: "1999-01-01" }],
    ["/api/scope", { op: "add", entry: "api.acme.test" }],
    ["/api/scope", { op: "exclude", entry: "sso.acme.test" }],
    ["/api/scope", { op: "remove", entry: "!pay.acme.test" }],
    ["/api/engagement", { target_notes: { "api.acme.test": "staging only" } }],
    ["/api/engagement", { rules: { rate_limit: 2, time_zone: "America/Chicago", agents_allowed: true } }],
    ["/api/engagement", { rules: { windows: [{ days: ["sat", "sun"], start: "10:00", end: "14:00" }] } }],
    ["/api/engagement", { rules: { blackouts: [{ from: "2026-12-24T00:00", to: "2026-12-27T00:00", note: "holiday freeze" }] } }],
  ]);

  await page.reload();
  await expect(page.getByRole("heading", { name: "ACME Q4 retest" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Engagement details" })).toContainText("No DoS");
  await expect(page.getByRole("list", { name: "In scope" })).toContainText("staging only");
  await expect(page.getByRole("list", { name: "Testing windows" })).toContainText("sat,sun 10:00-14:00");
});

test("switching and creating engagements; not testing now says why", async ({ page }) => {
  const api = engagementApi(page);
  api.testing = { allowed: false, reason: "outside the testing windows (mon-fri 09:00-17:00 UTC; it is sun 10:00 there)" };
  let switched: unknown = null;
  let created: unknown = null;
  await page.route("**/api/engagements/active", async (r) => {
    switched = r.request().postDataJSON();
    await r.fulfill({ json: { id: "beta-9f00", name: "Beta" } });
  });
  await page.route("**/api/engagements", async (r) => {
    if (r.request().method() !== "POST") return r.fallback();
    created = r.request().postDataJSON();
    await r.fulfill({ status: 201, json: { id: "gamma-0001", name: "Gamma" } });
  });
  await page.goto("/#engagement");
  await expect(page.locator(".chip.testing")).toHaveText(/^not testing now: outside the testing windows/);
  await expect(page.locator(".statusbar .testing-closed")).toBeVisible();
  await page.getByLabel("Active engagement").selectOption("beta-9f00");
  await expect.poll(() => switched).toEqual({ id: "beta-9f00" });
  await page.getByRole("button", { name: "new engagement" }).click();
  await page.getByLabel("New engagement name").fill("Gamma");
  await page.getByRole("button", { name: "create" }).click();
  await expect.poll(() => created).toEqual({ name: "Gamma" });
  // Unbuilt panels are dimmed and inert.
  const todo = page.locator(".todo-panels li");
  await expect(todo).toHaveCount(3);
  await expect(todo.first()).toHaveAttribute("aria-disabled", "true");
  expect(await todo.first().evaluate((e) => Number(getComputedStyle(e).opacity))).toBeLessThan(0.5);
});

test("hostile engagement, scope and rule strings render as text", async ({ page }) => {
  const p = `<img src=x onerror="window.__pwned=1">`;
  engagementApi(page, {
    name: `ACME ${p}`,
    client: p,
    notes: `${p}\n<script>window.__pwned=2</script>`,
    scope: [`${p}.test`, `!${p}`],
    target_notes: { [`${p}.test`]: p },
    rules: {
      methods: [p],
      safe_methods: [],
      rate_limit: null,
      time_zone: p,
      windows: [],
      blackouts: [{ from: "2026-12-24T00:00", to: "2026-12-27T00:00", note: p }],
      agents_allowed: false,
      destructive_allowed: false,
    },
  });
  await page.goto("/#engagement");
  await expect(page.getByRole("heading", { name: `ACME ${p}` })).toBeVisible();
  await expect(page.locator(".eng-head .meta")).toContainText(p);
  await expect(page.getByRole("list", { name: "In scope" })).toContainText(`${p}.test${p}`);
  await expect(page.getByRole("list", { name: "Excluded" })).toContainText(p);
  await expect(page.getByRole("region", { name: "Rules of engagement" })).toContainText(p);
  await expect(page.getByRole("region", { name: "Engagement details" })).toContainText("<script>");
  expect(await page.locator(".eng img, .eng script").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("paste a scope list: the server previews every line, one click applies (#38)", async ({ page }) => {
  const api = engagementApi(page);
  const p = `<img src=x onerror="window.__pwned=1">`;
  const texts: string[] = [];
  await page.route("**/api/scope/parse", async (r) => {
    const { text } = r.request().postDataJSON() as { text: string };
    texts.push(text);
    await r.fulfill({
      json: {
        items: [
          { line: 2, source: "https://www.acme.test/login", entry: "www.acme.test", exclude: false, error: null, notes: ['path "/login" dropped: scope is by host'], present: false },
          { line: 3, source: "*.acme.test", entry: "*.acme.test", exclude: false, error: null, notes: ["already in the scope"], present: true },
          { line: 5, source: "pay.acme.test", entry: "pay.acme.test", exclude: true, error: null, notes: [], present: false },
          { line: 6, source: p, entry: null, exclude: false, error: "not a hostname, *.wildcard, IP or CIDR", notes: [], present: false },
        ],
        add: ["www.acme.test"],
        exclude: ["pay.acme.test"],
      },
    });
  });
  await page.goto("/");
  // `scope paste` in the console opens the panel in the Engagement view.
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await page.getByLabel("Console command").fill("scope paste");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#engagement\/paste$/);
  const panel = page.getByRole("group", { name: "Paste a scope list" });
  await panel.getByLabel("Scope list").fill(`In scope:\n- https://www.acme.test/login\n*.acme.test\nOut of scope:\npay.acme.test\n${p}`);
  await expect(panel.getByRole("status")).toHaveText("1 to add · 1 to exclude · 1 already there · 1 not understood (skipped)");
  const rows = panel.getByRole("table", { name: "Preview" }).locator("tbody tr");
  await expect(rows).toHaveCount(4);
  await expect(rows.nth(0)).toContainText('add www.acme.testpath "/login" dropped');
  await expect(rows.nth(2)).toContainText("exclude pay.acme.test");
  await expect(rows.nth(3)).toContainText(p);
  expect(await page.locator(".eng img").count()).toBe(0);
  await panel.getByRole("button", { name: "apply 2" }).click();
  await expect(panel).toHaveCount(0);
  await expect(page).toHaveURL(/#engagement$/);
  await expect(page.getByRole("list", { name: "In scope" })).toContainText("www.acme.test");
  await expect(page.getByRole("list", { name: "Excluded" })).toContainText("pay.acme.test");
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/scope", body: { op: "apply", add: ["www.acme.test"], exclude: ["pay.acme.test"] } });
  expect(texts.at(-1)).toContain("Out of scope:");
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("hosts from traffic: rows show their scope state, scope: filters, the Engagement view decides (#39)", async ({ page }) => {
  const api = engagementApi(page);
  const t = `h${Date.now().toString(36)}`;
  const p = `<img src=x onerror="window.__pwned=1">`;
  const hosts = [
    { host: `app.${t}.test`, count: 5, first_seen: "2026-11-22T10:00:00Z", last_seen: "2026-11-22T10:05:00Z", state: "in", by: `*.${t}.test` },
    { host: `www.tracker-${t}.com`, count: 3, first_seen: "2026-11-22T10:00:00Z", last_seen: "2026-11-22T10:04:00Z", state: "unknown", by: null },
    { host: `pay.${t}.test`, count: 1, first_seen: "2026-11-22T10:00:00Z", last_seen: "2026-11-22T10:01:00Z", state: "out", by: `!pay.${t}.test` },
    { host: p, count: 1, first_seen: "2026-11-22T10:00:00Z", last_seen: "2026-11-22T10:01:00Z", state: "unknown", by: null },
  ];
  await page.route("**/api/hosts", (r) =>
    r.fulfill({ json: { hosts, counts: { in: 1, out: 1, unknown: hosts.filter((h) => h.state === "unknown").length }, dropped: 0 } }),
  );
  const { append, entry } = await import("./fixtures.ts");
  append(
    entry({ host: `app.${t}.test`, url: `http://app.${t}.test/a` }),
    entry({ host: `www.tracker-${t}.com`, url: `http://www.tracker-${t}.com/p.gif` }),
    entry({ host: `pay.${t}.test`, url: `http://pay.${t}.test/x` }),
  );
  await page.goto("/");
  const { filterTo } = await import("./fixtures.ts");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(3);
  const marker = (h: string) => page.locator("tr.row", { hasText: h }).locator(".sc");
  await expect(marker(`app.${t}.test`)).toHaveAttribute("data-scope", "in");
  await expect(marker(`www.tracker-${t}.com`)).toHaveAttribute("data-scope", "unknown");
  await expect(marker(`pay.${t}.test`)).toHaveAttribute("data-scope", "out");
  await filterTo(page, `${t} scope:unknown`);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await expect(page.locator("tr.row")).toContainText(`www.tracker-${t}.com`);

  await page.goto("/#engagement");
  const panel = page.getByRole("region", { name: "Hosts from traffic" });
  const undecided = panel.getByRole("list", { name: "Undecided hosts" });
  await expect(undecided.locator("li")).toHaveCount(2);
  await expect(undecided).toContainText(p);
  await panel.getByRole("button", { name: `Exclude *.tracker-${t}.com` }).click();
  await expect.poll(() => api.calls.at(-1)?.body).toEqual({ op: "exclude", entry: `*.tracker-${t}.com` });
  await panel.getByRole("checkbox").check();
  await expect(panel.getByRole("list", { name: "Hosts" }).locator("li")).toHaveCount(4);
  expect(await page.locator(".eng img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("import an OpenAPI file: servers previewed, hosts applied, the spec kept (#40)", async ({ page }) => {
  const api = engagementApi(page);
  const p = `<img src=x onerror="window.__pwned=1">`;
  let sent: { text: string } | null = null;
  await page.route("**/api/openapi/parse", async (r) => {
    sent = r.request().postDataJSON() as { text: string };
    await r.fulfill({
      json: {
        spec: { title: `ACME ${p}`, version: "2", format: "openapi 3.0.3", servers: ["https://api.acme.test/v2", "/rel"], paths: ["/orders/{id}", "/users"], imported: "2026-11-22T00:00:00Z" },
        notes: ['server "/rel" is relative: its host is wherever the spec is served from'],
        items: [{ line: 1, source: "https://api.acme.test/v2", entry: "api.acme.test", exclude: false, error: null, notes: ['path "/v2" dropped: scope is by host'], present: false }],
        add: ["api.acme.test"],
        exclude: [],
      },
    });
  });
  await page.goto("/#engagement/import");
  const panel = page.getByRole("group", { name: "Import an OpenAPI file" });
  await panel.getByLabel("OpenAPI file").setInputFiles({ name: "acme.yaml", mimeType: "text/yaml", buffer: Buffer.from("openapi: 3.0.3\n") });
  await expect(panel.getByRole("status")).toHaveText(`ACME ${p} 2 (openapi 3.0.3) · 2 paths · 1 to add · 0 to exclude`);
  await expect(panel.getByRole("list", { name: "Import notes" })).toContainText("is relative");
  expect(sent).toEqual({ text: "openapi: 3.0.3\n" });
  await panel.getByRole("button", { name: "apply 1 and keep the spec" }).click();
  await expect(panel).toHaveCount(0);
  expect(api.calls.slice(-2).map((c) => c.body)).toEqual([
    { op: "apply", add: ["api.acme.test"], exclude: [] },
    { api_specs: [{ title: `ACME ${p}`, version: "2", format: "openapi 3.0.3", servers: ["https://api.acme.test/v2", "/rel"], paths: ["/orders/{id}", "/users"], imported: "2026-11-22T00:00:00Z" }] },
  ]);
  const kept = page.getByRole("list", { name: "API descriptions" });
  await expect(kept).toContainText(`ACME ${p} 2`);
  await expect(kept).toContainText("openapi 3.0.3 · 2 paths · 2 servers");
  await expect(page.getByRole("list", { name: "In scope" })).toContainText("api.acme.test");
  await kept.getByRole("button", { name: `Forget ACME ${p}` }).click();
  await expect(page.getByRole("list", { name: "API descriptions" })).toHaveCount(0);
  expect(await page.locator(".eng img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("browser setup: the CA's fingerprint and downloads, steps with the proxy, and a live check (#32)", async ({ page }) => {
  engagementApi(page);
  const p = `<img src=x onerror="window.__pwned=1">`;
  await page.route("**/api/ca", (r) =>
    r.fulfill({ json: { sha256: "EB:6F:71:3A", subject: `CN=taphttp CA ${p}`, not_before: "2026-11-20T00:00:00Z", not_after: "2036-11-20T00:00:00Z", proxy: "127.0.0.1:8080" } }),
  );
  await page.goto("/");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await page.getByLabel("Console command").fill("ca");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#engagement\/ca$/);
  await expect(page.locator(".con-log")).toContainText("sha256: EB:6F:71:3A");
  await page.keyboard.press("Escape");
  const setup = page.getByRole("region", { name: "Set up a browser" });
  await expect(setup).toContainText("EB:6F:71:3A");
  await expect(setup).toContainText(`CN=taphttp CA ${p}`);
  await expect(setup.getByRole("link", { name: "download taphttp-ca.pem" })).toHaveAttribute("href", "/api/ca.pem");
  await expect(setup.getByRole("link", { name: ".der" })).toHaveAttribute("download", "taphttp-ca.der");
  await expect(setup).toContainText("HTTP proxy 127.0.0.1, port 8080");
  await expect(setup).toContainText("http://taphttp/");
  expect(await page.locator(".eng img").count()).toBe(0);

  await setup.getByRole("button", { name: "start the check" }).click();
  await expect(setup.getByRole("status")).toContainText("Waiting for an https request");
  const { append, entry, token } = await import("./fixtures.ts");
  const t = token();
  append(entry({ host: `${t}.example`, url: `https://${t}.example/`, ts: new Date(Date.now() + 1000).toISOString() }));
  await expect(setup.getByRole("status")).toHaveText(`✓ spall saw GET https://${t}.example/ (200). The browser is set up.`);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
