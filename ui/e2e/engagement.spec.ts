// The Engagement view (#37), against the in-memory engagement API in
// engagement-api.ts.
import { expect, test } from "./fixtures.ts";
import { engagementApi } from "./engagement-api.ts";

test("a bare URL opens Engagement, the home view; unbuilt views fall back to it (#71)", async ({ page }) => {
  engagementApi(page);
  await page.goto("/");
  await expect(page.locator(".rail [data-view='engagement']")).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "ACME Q4" })).toBeVisible();
  await expect(page.getByLabel("Filter")).toHaveCount(0);
  await page.goto("/#surface");
  await expect(page.locator(".rail [data-view='engagement']")).toHaveAttribute("aria-current", "page");
  await page.goto("/#traffic");
  await expect(page.getByLabel("Filter")).toBeVisible();
});

test("1 opens the Engagement view and 2 goes back; the rail marks the current view", async ({ page }) => {
  engagementApi(page);
  await page.goto("/#traffic");
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
  await expect(todo).toHaveText([/Coverage: endpoints seen vs\. tested/]);
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
      refuse_destructive_looking: false,
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
  await page.goto("/#traffic");
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
  // The preview isn't laid out like the Traffic table: long lines wrap
  // instead of being cut off with an ellipsis.
  const preview = panel.getByRole("table", { name: "Preview" });
  expect(await preview.evaluate((t) => getComputedStyle(t).tableLayout)).toBe("auto");
  expect(await rows.nth(0).locator("td.notes").evaluate((td) => getComputedStyle(td).whiteSpace)).toBe("normal");
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
  await page.goto("/#traffic");
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
  // The state dot doesn't run into the host name.
  expect(await undecided.locator(".sc").first().evaluate((d) => parseFloat(getComputedStyle(d).marginRight))).toBeGreaterThan(0);
  await panel.getByRole("button", { name: `Exclude *.tracker-${t}.com` }).click();
  await expect.poll(() => api.calls.at(-1)?.body).toEqual({ op: "exclude", entry: `*.tracker-${t}.com` });
  await panel.getByRole("checkbox").check();
  await expect(panel.getByRole("list", { name: "Hosts" }).locator("li")).toHaveCount(4);
  expect(await page.locator(".eng img").count()).toBe(0);

  // With scope all on, nothing is undecided, and the panel says why (#72).
  await page.unroute("**/api/hosts");
  const allIn = hosts.map((h) => ({ ...h, state: "in", by: "*" }));
  await page.route("**/api/hosts", (r) => r.fulfill({ json: { hosts: allIn, counts: { in: allIn.length, out: 0, unknown: 0 }, dropped: 0 } }));
  await page.reload();
  await expect(panel.locator(".note.wrap").first()).toContainText("Scope all is on: every host counts as in scope, so none are undecided.");
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
  await page.goto("/#traffic");
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

test("too-wide scope entries are refused until you say so; the override is explicit (#83)", async ({ page }) => {
  const api = engagementApi(page);
  // The server's answer to a too-wide entry without the override.
  await page.route("**/api/scope", async (r) => {
    const c = r.request().postDataJSON() as { op: string; entry?: string; wide?: boolean; add?: string[] };
    if (!c.wide && (c.entry === "10.0.0.0/8" || (c.add ?? []).includes("*.com"))) {
      api.calls.push({ method: "POST", path: "/api/scope", body: c });
      return r.fulfill({ status: 400, json: { error: 'scope entry "10.0.0.0/8" is too wide: 10.0.0.0/8 is wider than /16, the most one scope entry may cover. Name the client\'s hosts or a narrower network, or add it with --wide (audited)' } });
    }
    return r.fallback();
  });
  await page.route("**/api/scope/parse", (r) =>
    r.fulfill({
      json: {
        items: [
          { line: 1, source: "*.com", entry: "*.com", exclude: false, error: null, notes: ["*.com is too wide: com is a public suffix"], present: false, wide: true },
          { line: 2, source: "app.acme.test", entry: "app.acme.test", exclude: false, error: null, notes: [], present: false, wide: false },
        ],
        add: ["*.com", "app.acme.test"],
        exclude: [],
      },
    }),
  );
  await page.goto("/#engagement");
  const input = page.getByLabel("Scope entry");
  await input.fill("10.0.0.0/8");
  await input.press("Enter");
  await expect(page.locator(".eng")).toContainText("is too wide");
  await page.getByRole("button", { name: "add 10.0.0.0/8 anyway" }).click();
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/scope", body: { op: "add", entry: "10.0.0.0/8", wide: true } });

  await page.goto("/#engagement/paste");
  const panel = page.getByRole("group", { name: "Paste a scope list" });
  await panel.getByLabel("Scope list").fill("*.com\napp.acme.test");
  const apply = panel.getByRole("button", { name: "apply 2" });
  await expect(apply).toBeDisabled();
  await panel.getByRole("checkbox", { name: /too-wide entry anyway \(\*\.com\)/ }).check();
  await apply.click();
  await expect(panel).toHaveCount(0);
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/scope", body: { op: "apply", add: ["*.com", "app.acme.test"], exclude: [], wide: true } });
});

test("the never-test list: add, the offered .gov and .mil, an exemption for this engagement, remove (#84)", async ({ page }) => {
  const api = engagementApi(page);
  const p = `<img src=x onerror="window.__pwned=1">`;
  await page.goto("/#engagement");
  const panel = page.getByRole("region", { name: "Never tested" });
  await expect(panel).toContainText("Empty.");
  await panel.getByLabel("Never-test entry").fill("mycompany.example");
  await panel.getByLabel("Never-test entry").press("Enter");
  const list = panel.getByRole("list", { name: "Never-test list" });
  await expect(list).toContainText("mycompany.example");
  await panel.getByRole("button", { name: "add *.gov and *.mil" }).click();
  await expect(list.locator(".entry")).toHaveText(["mycompany.example", "*.gov", "*.mil"]);
  await expect(panel.getByRole("button", { name: /add \*\.gov/ })).toHaveCount(0);
  // Exempt this engagement from *.gov (a government client).
  await list.locator("li", { hasText: "*.gov" }).getByRole("checkbox").check();
  await expect.poll(() => api.calls.filter((c) => c.path === "/api/engagement").at(-1)?.body).toEqual({ never_test_exceptions: ["*.gov"] });
  await expect(list.locator("li", { hasText: "*.gov" }).getByRole("checkbox")).toBeChecked();
  await panel.getByRole("button", { name: "Remove mycompany.example from the never-test list" }).click();
  await expect(list.locator(".entry")).toHaveText(["*.gov", "*.mil"]);
  // A hostile entry is text.
  await panel.getByLabel("Never-test entry").fill(p);
  await panel.getByLabel("Never-test entry").press("Enter");
  await expect(list).toContainText(p);
  expect(await page.locator(".eng img").count()).toBe(0);
  expect(api.calls.filter((c) => c.path === "/api/never-test").map((c) => c.body)).toEqual([
    { op: "add", entry: "mycompany.example" },
    { op: "add", entry: "*.gov" },
    { op: "add", entry: "*.mil" },
    { op: "remove", entry: "mycompany.example" },
    { op: "add", entry: p },
  ]);
});

test("checks for sweep: built in and imported, and sweep from the console says its cost before it runs (#75)", async ({ page }) => {
  engagementApi(page);
  const checks: { name: string; rule: string; title: string; method: string; paths: string[]; source: string }[] = [
    { name: "admin-interfaces", rule: "WSTG-CONF-05", title: "Admin", method: "GET", paths: ["/admin", "/console/"], source: "built_in" },
  ];
  const posted: unknown[] = [];
  await page.route("**/api/checks", async (r) => {
    if (r.request().method() === "POST") {
      const c = r.request().postDataJSON() as { op: string; text?: string; name?: string };
      posted.push(c);
      if (c.op === "add") checks.push({ name: "mine", rule: "WSTG-CONF-04", title: "mine", method: "GET", paths: ["/old"], source: "engagement" });
      if (c.op === "remove") checks.splice(checks.findIndex((x) => x.name === c.name), 1);
    }
    return r.fulfill({ json: { checks } });
  });
  const sweeps: unknown[] = [];
  await page.route("**/api/sweep", async (r) => {
    const b = r.request().postDataJSON() as { yes?: boolean };
    sweeps.push(b);
    return r.fulfill({
      json: b.yes
        ? { check: "admin-interfaces", rule: "WSTG-CONF-05", base: "https://acme.test", planned: 3, sent: 3, baseline: { status: 404, size: 9 }, hits: [{ path: "/admin", exchange: "s-2", status: 401, size: 14 }], misses: 1, skipped: [], stopped: null }
        : { check: "admin-interfaces", rule: "WSTG-CONF-05", title: "Admin", base: "https://acme.test", method: "GET", planned: 3, cost: "3 GET requests (one is the soft-404 baseline)" },
    });
  });
  await page.goto("/#engagement");
  const panel = page.getByRole("region", { name: "Checks" });
  await expect(panel.getByRole("list", { name: "Checks list" })).toContainText("admin-interfacesWSTG-CONF-05 · 2 GET paths · built in");
  await expect(panel.getByRole("button", { name: "Remove check admin-interfaces" })).toHaveCount(0);
  await panel.getByLabel("Import a check").fill("name mine\nrule WSTG-CONF-04\nhit differs\npath /old");
  await panel.getByRole("button", { name: "import" }).click();
  await expect(panel.getByRole("list", { name: "Checks list" })).toContainText("mine");
  expect(posted).toEqual([{ op: "add", text: "name mine\nrule WSTG-CONF-04\nhit differs\npath /old" }]);
  await panel.getByRole("button", { name: "Remove check mine" }).click();
  await expect(panel.getByRole("list", { name: "Checks list" })).not.toContainText("mine");

  await page.keyboard.press("`");
  const input = page.getByLabel("Console command");
  await input.fill("sweep https://acme.test admin-interfaces");
  await input.press("Enter");
  const log = page.locator(".console");
  await expect(log).toContainText("3 GET requests (one is the soft-404 baseline). sweep https://acme.test admin-interfaces --yes to run it");
  await input.fill("sweep https://acme.test admin-interfaces --yes");
  await input.press("Enter");
  await expect(log).toContainText("hit /admin  401  14 B  open s-2 · flag s-2 --rule WSTG-CONF-05");
  expect(sweeps).toEqual([{ base: "https://acme.test", check: "admin-interfaces" }, { base: "https://acme.test", check: "admin-interfaces", yes: true }]);
});
