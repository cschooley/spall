// The Surface view (#95, #94): endpoints with who touched them and their
// coverage by WSTG category; untested filters; mark tested; hostile
// templates and params rendered as text. /api/surface is mocked.
import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures.ts";
import { engagementApi } from "./engagement-api.ts";

const P = `<img src=x onerror="window.__pwned=1">`;

function surfaceApi(page: Page) {
  const state = {
    marks: [] as { key: string; category: string; note: string }[],
    asked: [] as string[],
  };
  void page.route(/\/api\/coverage$/, async (r) => {
    state.marks.push(r.request().postDataJSON() as { key: string; category: string; note: string });
    return r.fulfill({ json: { ok: true } });
  });
  void page.route(/\/api\/surface(\?.*)?$/, (r) => {
    state.asked.push(r.request().url());
    const all = r.request().url().includes("scope=all");
    const tested = (k: string, c: string) => state.marks.some((m) => m.key === k && m.category === c);
    const row = (o: Record<string, unknown>) => ({ statuses: [200], params: [], first_seen: "2026-11-24T10:00:00Z", last_seen: "2026-11-24T10:05:00Z", by_you: 0, tasks: [], documented: false, scope: "in", ...o });
    const endpoints = [
      row({
        key: "app.test|GET|/users/{}",
        host: "app.test",
        method: "GET",
        template: "/users/{id}",
        count: 7,
        params: ["sort", `x${P}`],
        example: "s-1",
        by_you: 5,
        tasks: ["T-2"],
        coverage: {
          ATHZ: { state: "found", candidates: ["c-00000001"] },
          ...(tested("app.test|GET|/users/{}", "INPV") ? { INPV: { state: "tested", by: ["human"], how: ["marked"] } } : {}),
        },
      }),
      row({ key: "app.test|POST|/login", host: "app.test", method: "POST", template: "/login", count: 2, example: "s-2", by_you: 2, coverage: { ATHN: { state: "tested", by: ["agent:T-2"], how: ["sweep default-creds"] } } }),
      row({ key: `app.test|ANY|/x${P}`, host: "app.test", method: "ANY", template: `/x${P}`, count: 0, example: null, documented: true, coverage: {} }),
      ...(all ? [row({ key: "cdn.other|GET|/a.js", host: "cdn.other", method: "GET", template: "/a.js", count: 1, example: "s-3", by_you: 1, scope: "unknown", coverage: {} })] : []),
    ];
    return r.fulfill({
      json: {
        categories: [
          { code: "ATHN", name: "Authentication" },
          { code: "ATHZ", name: "Authorization" },
          { code: "INPV", name: "Input validation" },
        ],
        endpoints,
        dropped: 0,
      },
    });
  });
  return state;
}

const templates = (page: Page) => page.getByRole("table", { name: "Endpoints" }).locator("button.surf-row .where");

test("3 opens the Surface: endpoints, who touched them, coverage marks", async ({ page }) => {
  engagementApi(page);
  surfaceApi(page);
  await page.goto("/#engagement");
  await page.keyboard.press("3");
  await expect(page).toHaveURL(/#surface$/);
  await expect(templates(page)).toHaveText(["app.test/users/{id}", "app.test/login", `app.test/x${P}`]);
  const first = page.getByRole("table", { name: "Endpoints" }).locator("button.surf-row").first();
  await expect(first.locator(".who")).toHaveText("you · ◆ T-2");
  // Marks are characters, readable without color: ATHN untested, ATHZ found, INPV untested.
  await expect(first.locator(".marks .m")).toHaveText(["·", "!", "·"]);
  await expect(page.getByRole("table", { name: "Endpoints" }).locator("button.surf-row").nth(2).locator(".who")).toHaveText("documented");

  // The pane: the selected endpoint, its coverage, a link to the finding.
  const pane = page.getByRole("complementary", { name: "Endpoint" });
  await expect(pane.locator("h2")).toHaveText("GET app.test/users/{id}");
  await expect(pane.getByRole("link", { name: "c-00000001" })).toHaveAttribute("href", "#findings/c-00000001");
  await expect(pane.locator(".kv dd").nth(1)).toHaveText(`sort, x${P}`);

  // j moves; the documented row has no exchange, so no agent verbs.
  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await expect(pane.locator("h2")).toHaveText(`ANY app.test/x${P}`);
  await expect(pane.getByText("No exchange on it yet")).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(page.locator(".surface img")).toHaveCount(0);
});

test("the strip filters to what's untested; out of scope is opt-in", async ({ page }) => {
  engagementApi(page);
  const api = surfaceApi(page);
  await page.goto("/#surface");
  const strip = page.getByRole("group", { name: "Untested by category" });
  await expect(strip.locator("button")).toHaveText(["ATHN2", "ATHZ2", "INPV3"]);
  await strip.getByRole("button", { name: /ATHN/ }).click();
  await expect(page.getByLabel("Filter endpoints")).toHaveValue("untested:ATHN");
  await expect(templates(page)).toHaveText(["app.test/users/{id}", `app.test/x${P}`]);
  await page.getByLabel("Filter endpoints").fill("found");
  await expect(templates(page)).toHaveText(["app.test/users/{id}"]);
  await page.getByLabel("Filter endpoints").fill("");
  await page.getByLabel("out of scope too").check();
  await expect(templates(page)).toHaveCount(4);
  await expect(page.locator("button.surf-row.oos .where")).toHaveText("cdn.other/a.js");
  expect(api.asked.some((u) => u.includes("scope=all"))).toBe(true);
});

test("mark an endpoint tested for a category, with what you did", async ({ page }) => {
  engagementApi(page);
  const api = surfaceApi(page);
  await page.goto("/#surface");
  const pane = page.getByRole("complementary", { name: "Endpoint" });
  await expect(pane.locator("h2")).toHaveText("GET app.test/users/{id}");
  // The first untested category is preselected.
  await expect(pane.getByLabel("Category tested")).toHaveValue("ATHN");
  await pane.getByLabel("Category tested").selectOption("INPV");
  await pane.getByLabel("What you tested").fill("sort: quotes and boolean pairs, no change");
  await pane.getByRole("button", { name: "Mark tested" }).click();
  await expect(pane.getByRole("status")).toHaveText("marked INPV tested");
  expect(api.marks).toEqual([{ key: "app.test|GET|/users/{}", category: "INPV", note: "sort: quotes and boolean pairs, no change" }]);
  await expect(pane.locator(".cov-list li.tested .code")).toHaveText("INPV");
  await expect(pane.locator(".cov-list li.tested .detail")).toHaveText("human (marked)");
});
