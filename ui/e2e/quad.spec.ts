// Quad mode (#66) in the browser: the console starts it, the top bar shows
// a QUAD m:ss countdown, the screen edge and request meter glow in boost
// color, and when it ends the scorecard and its numbers-only card appear.
// spall runs in log mode here, so /api/status and /api/quad are mocked.
import { expect, test } from "./fixtures.ts";
import type { Page } from "@playwright/test";
import { candidatesApi } from "./candidates-api.ts";

const CARD = "QUAD 2:00 · 8× agents · 137 requests · 412.0k tokens · 3 candidates · spall";

function quadApi(page: Page) {
  const state = { quad: null as unknown, last: null as unknown, calls: [] as unknown[] };
  candidatesApi(page);
  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "lever", connected: true, proxy: "127.0.0.1:8080", replay: true, scope: ["example.test"], scope_all: false, engagement: { id: "acme-1a2b", name: "ACME" }, testing: { allowed: true, reason: null }, to_triage: 0, estop: null, quad: state.quad, last_quad: state.last },
    }),
  );
  void page.route("**/api/quad", async (r) => {
    const body = r.request().postDataJSON() as { seconds?: number; stop?: boolean };
    state.calls.push(body);
    if (body.stop) {
      state.quad = null;
      state.last = { seconds: 45, workers: 8, requests: 137, tokens: 412000, candidates: 3, ended: "stopped", card: CARD };
      return r.fulfill({ json: { scorecard: state.last } });
    }
    const until = new Date(Date.now() + (body.seconds ?? 120) * 1000).toISOString();
    state.quad = { started: new Date().toISOString(), until, seconds: body.seconds ?? 120, workers: 8, capped: "capped at the rules of engagement's 10 sends per second", remaining_secs: body.seconds ?? 120 };
    return r.fulfill({ status: 201, json: { quad: state.quad } });
  });
  return state;
}

test("quad from the console: a countdown, the glow, and a numbers-only scorecard when it ends", async ({ page }) => {
  const api = quadApi(page);
  await page.goto("/#traffic");
  await expect(page.locator(".quad-chip")).toHaveCount(0);
  await expect(page.locator(".quad-glow")).toHaveCount(0);

  await page.keyboard.press("`");
  const input = page.getByRole("textbox", { name: /console/i });
  await input.fill("quad 90");
  await input.press("Enter");
  expect(api.calls).toEqual([{ seconds: 90 }]);
  const log = page.locator(".console");
  await expect(log).toContainText("QUAD for 1:30: 8 agent workers, 4x rate");
  await expect(log).toContainText("capped at the rules of engagement's 10 sends per second");

  const chip = page.locator("header.bar .quad-chip");
  await expect(chip).toHaveText(/^QUAD 1:(2\d|30)$/);
  await expect(page.locator(".quad-glow")).toHaveCount(1);
  await expect(page.locator(".app.quad")).toHaveCount(1);

  await input.fill("quad stop");
  await input.press("Enter");
  await expect(log).toContainText("quad over (ended early): 0:45, 8 workers");
  await expect(chip).toHaveCount(0);
  await expect(page.locator(".quad-glow")).toHaveCount(0);
  const card = page.getByRole("region", { name: "Quad scorecard" });
  await expect(card).toContainText("137");
  await expect(card.locator("code")).toHaveText(CARD);
  await card.getByRole("button", { name: "Close" }).click();
  await expect(card).toHaveCount(0);
});

test("an old scorecard isn't shown on a page that never saw the burst", async ({ page }) => {
  const api = quadApi(page);
  api.last = { seconds: 120, workers: 8, requests: 1, tokens: 1, candidates: 0, ended: "time", card: "QUAD 2:00 · 8× agents · 1 requests · 1 tokens · 0 candidates · spall" };
  await page.goto("/#traffic");
  await expect(page.locator("header.bar .meter")).toBeVisible();
  await expect(page.getByRole("region", { name: "Quad scorecard" })).toHaveCount(0);
});
