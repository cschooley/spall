// History over /api/exchanges. The e2e spall tails a log file (no history
// API), so these tests stand in for taphttp's API at the browser's network
// layer, following its contract: newest first, `before=<id>` pages back.
import type { Page, Route } from "@playwright/test";
import { entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { TrafficEntry } from "../src/types.ts";

/** n entries for host `${t}.hist`, oldest first, one second apart. */
function series(t: string, n: number): TrafficEntry[] {
  const start = Date.parse("2026-12-25T00:00:00Z");
  return Array.from({ length: n }, (_, i) =>
    entry({
      id: `${t}-${String(i).padStart(4, "0")}`,
      ts: new Date(start + i * 1000).toISOString(),
      host: `${t}.hist`,
      url: `http://${t}.hist/${i}`,
    }),
  );
}

async function fakeHistory(page: Page, all: TrafficEntry[]) {
  await page.route("**/api/status", (r) => r.fulfill({ json: { source: "lever", connected: true } }));
  const requests: URLSearchParams[] = [];
  await page.route(/\/api\/exchanges\?/, (route: Route) => {
    const qs = new URL(route.request().url()).searchParams;
    requests.push(qs);
    const newest = [...all].reverse();
    const before = qs.get("before");
    const from = before ? newest.findIndex((e) => e.id === before) + 1 : 0;
    const limit = Number(qs.get("limit") ?? 100);
    return route.fulfill({ json: newest.slice(from, from + limit) });
  });
  return requests;
}

test("filters become API parameters, status classes included; replay: stays client-side", async ({ page }) => {
  const t = token();
  const requests = await fakeHistory(page, []);
  await page.goto("/#traffic");
  await filterTo(page, `host:${t} method:post status:4xx login`);
  await expect.poll(() => requests.at(-1)?.toString()).toBe(`host=${t}&method=POST&status=4xx&q=login&limit=200`);
  await filterTo(page, `replay:yes ${t}`);
  await expect.poll(() => requests.at(-1)?.toString()).toBe(`q=${t}&limit=200`);
  await filterTo(page, `status:404 ${t}`);
  await expect.poll(() => requests.at(-1)?.toString()).toBe(`status=404&q=${t}&limit=200`);
});

test("history pages in oldest-first and scrolling to the top loads older pages", async ({ page }) => {
  const t = token();
  const all = series(t, 250);
  const requests = await fakeHistory(page, all);
  await page.goto("/#traffic");
  await filterTo(page, `host:${t}`);

  await expect(page.locator(".statusbar")).toContainText("200 exchanges");
  await expect(page.locator("tr.row").last()).toContainText("/249");
  await expect(page.locator(".note")).toHaveText("scroll up for older");

  const wrap = page.locator(".tablewrap");
  await wrap.evaluate((m) => (m.scrollTop = 0));
  await expect(page.locator(".statusbar")).toContainText("250 exchanges");
  expect(requests.at(-1)?.get("before")).toBe(`${t}-0050`);
  await expect(page.locator(".note")).toHaveText("");
  // The view stays where it was: the old top row (/50) is still in view.
  await expect(page.locator("tr.row", { hasText: `${t}.hist/50` }).first()).toBeInViewport();
  await wrap.evaluate((m) => (m.scrollTop = 0));
  await expect(page.locator("tr.row").first()).toContainText("/0");
});

test("without a history API (log-file mode) the view says live only", async ({ page }) => {
  await page.goto("/#traffic");
  await expect(page.locator(".note")).toHaveText("live only (log-file mode: no history API)");
  await expect(page.locator(".statusbar")).toContainText("taphttp ● log file");
});

test("status bar shows taphttp's connection and proxy address in lever mode", async ({ page }) => {
  await fakeHistory(page, []);
  await page.unroute("**/api/status");
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "lever", connected: true, proxy: "127.0.0.1:18080" } }),
  );
  await page.goto("/#traffic");
  await expect(page.locator(".statusbar .ok")).toHaveText("● connected");
  await expect(page.locator(".statusbar .addr")).toHaveText(" 127.0.0.1:18080");
});
