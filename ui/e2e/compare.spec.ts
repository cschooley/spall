// Compare a replay with its original (#20).
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { TrafficEntry } from "../src/types.ts";

const later = (e: TrafficEntry, s: number) => new Date(Date.parse(e.ts) + s * 1000).toISOString();

test("Compare from the Replays list: verdict, headers and a collapsed body diff", async ({ page }) => {
  const t = token();
  const body = (code: string) =>
    JSON.stringify({ items: Array.from({ length: 30 }, (_, i) => ({ n: i })), status: code, user: "admin" });
  const orig = entry({
    host: `${t}.example`,
    url: `http://${t}.example/cmp`,
    status: 401,
    res_headers: [["content-type", "application/json"], ["date", "Sat, 21 Nov 2026 10:00:00 GMT"], ["x-old", "1"]],
    res_body: body("denied"),
  });
  const rep = entry({
    ...orig,
    id: `${orig.id}-r`,
    ts: later(orig, 3),
    replay_of: orig.id,
    status: 200,
    res_headers: [["content-type", "application/json"], ["date", "Sat, 21 Nov 2026 10:00:03 GMT"], ["x-new", "1"]],
    res_body: body("ok"),
  });
  append(orig, rep);
  await page.goto("/#traffic");
  await filterTo(page, `${t} replay:no`);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: /replays/ }).click();
  await page.locator(".replays li").first().getByRole("button", { name: "Compare" }).click();

  await expect(page.getByRole("tab", { name: "compare" })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${orig.id}`);
  await expect(page.locator(".compare .verdict")).toHaveText("status 401 → 200 · body differs (2 lines) · 2 headers changed (1 volatile)");

  const res = page.getByRole("region", { name: "Response comparison" });
  await expect(res.locator(".hd-removed")).toContainText("x-old");
  await expect(res.locator(".hd-added")).toContainText("x-new");
  await expect(res.locator(".hd.volatile")).toContainText("date");
  await expect(res.locator(".hd-changed.volatile .now")).toContainText("10:00:03");

  const diff = res.getByRole("group", { name: "Body diff" });
  await expect(diff.locator(".l-del")).toHaveText(['-   "status": "denied",']);
  await expect(diff.locator(".l-add")).toHaveText(['+   "status": "ok",']);
  await expect(diff.locator(".skip")).toContainText("unchanged lines (show)");
  await diff.locator(".skip").first().click();
  await expect(diff.locator(".skip")).toHaveCount(0);
  await expect(diff.locator(".l-same").first()).toHaveText("  {");

  // The request was replayed as captured.
  const req = page.getByRole("region", { name: "Request comparison" });
  await expect(req).toContainText("no differences");
  await expect(req).toContainText("no body on either side");
});

test("Compare is only for replays, and says when the original is gone", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/solo` });
  const orphan = entry({ host: `${t}.example`, url: `http://${t}.example/orphan`, ts: later(orig, 1), replay_of: `${t}-gone` });
  append(orig, orphan);
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").filter({ hasText: "/solo" }).click();
  await expect(page.getByRole("tab", { name: "compare" })).toHaveCount(0);
  await page.locator("tr.row").filter({ hasText: "/orphan" }).click();
  await page.getByRole("tab", { name: "compare" }).click();
  await expect(page.locator(".pane")).toContainText(`The original (${t}-gone) isn't in taphttp's history.`);
});

test("hostile bodies and headers stay text inside a diff", async ({ page }) => {
  const t = token();
  const p = `<img src=x onerror="window.__pwned=1">`;
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/hx`, res_headers: [["content-type", "text/html"]], res_body: `<p>a</p>` });
  const rep = entry({
    ...orig,
    id: `${orig.id}-r`,
    ts: later(orig, 1),
    replay_of: orig.id,
    res_headers: [["content-type", "text/html"], [`x-${p}`, p]],
    res_body: `<p>a</p>\n${p}`,
  });
  append(orig, rep);
  await page.goto("/#traffic");
  await filterTo(page, `${t} replay:yes`);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: "compare" }).click();
  await expect(page.locator(".ldiff .l-add")).toHaveText(`+ ${p}`);
  await expect(page.locator(".hdiff .hd-added")).toContainText(`+ ${p}`);
  expect(await page.locator(".inspector img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
