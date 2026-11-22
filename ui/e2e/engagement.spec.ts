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
    const c = r.request().postDataJSON() as { op: string; entry: string };
    state.calls.push({ method: "POST", path: "/api/scope", body: c });
    if (c.op === "add") state.e.scope = [c.entry, ...state.e.scope];
    if (c.op === "exclude") state.e.scope = [...state.e.scope, `!${c.entry}`];
    if (c.op === "remove") state.e.scope = state.e.scope.filter((x) => x !== c.entry);
    await r.fulfill({ json: { scope: state.e.scope, all: false, changed: true } });
  });
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
  await expect(todo).toHaveCount(4);
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
