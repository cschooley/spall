// The agent e-stop (#57) in the browser: always in the top bar, one press
// or Shift+Esc (even while typing) engages it, clearing takes two presses,
// and the server's reason renders as text. spall runs in log mode here, so
// /api/status and /api/estop* are mocked with the server's behavior.
import { expect, test } from "./fixtures.ts";
import type { Page } from "@playwright/test";
import { candidatesApi } from "./candidates-api.ts";

const P = `<img src=x onerror="window.__pwned=1">`;

function estopApi(page: Page, initial: { since: string; by: string; reason: string } | null = null) {
  const state = { estop: initial, calls: [] as string[] };
  // The rest of the lever-mode API; the routes below take precedence.
  candidatesApi(page);
  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "lever", connected: true, proxy: "127.0.0.1:8080", replay: true, scope: ["example.test"], scope_all: false, engagement: { id: "acme-1a2b", name: "ACME" }, testing: { allowed: true, reason: null }, to_triage: 0, estop: state.estop },
    }),
  );
  void page.route("**/api/estop", async (r) => {
    const body = (r.request().postDataJSON() ?? {}) as { reason?: string };
    state.calls.push(`engage:${body.reason ?? ""}`);
    state.estop ??= { since: "2026-11-23T20:00:00Z", by: "human", reason: body.reason || "stopped by a human" };
    await r.fulfill({ json: { estop: state.estop, halted: ["acme-1a2b/T-1", "acme-1a2b/T-2"], errors: [] } });
  });
  void page.route("**/api/estop/clear", async (r) => {
    state.calls.push("clear");
    const was = state.estop;
    state.estop = null;
    await r.fulfill({ json: { estop: null, was } });
  });
  return state;
}

test("one press stops every agent task; the banner says so; two presses clear it", async ({ page }) => {
  const api = estopApi(page);
  await page.goto("/#traffic");
  const button = page.locator("header.bar .estop");
  await expect(button).toHaveText("STOP AGENTS");
  await expect(page.locator(".halt-banner")).toHaveCount(0);

  await button.click();
  expect(api.calls).toEqual(["engage:"]);
  await expect(button).toHaveText("AGENTS STOPPED");
  const banner = page.getByRole("alert").filter({ hasText: "ALL AGENT TASKS HALTED" });
  await expect(banner).toContainText("stopped by a human");
  await expect(banner).toContainText("halted tasks never run again");

  // One press only arms the clear.
  await button.click();
  await expect(button).toHaveText("PRESS AGAIN TO CLEAR");
  expect(api.calls).toEqual(["engage:"]);
  await button.click();
  await expect(button).toHaveText("STOP AGENTS");
  expect(api.calls).toEqual(["engage:", "clear"]);
  await expect(page.locator(".halt-banner")).toHaveCount(0);
});

test("Shift+Esc engages it from anywhere, even while typing, and never clears it", async ({ page }) => {
  const api = estopApi(page);
  await page.goto("/#traffic");
  await page.getByLabel("Filter", { exact: true }).click();
  await page.keyboard.type("host:example");
  await page.keyboard.press("Shift+Escape");
  await expect(page.locator("header.bar .estop")).toHaveText("AGENTS STOPPED");
  await page.keyboard.press("Shift+Escape");
  await expect(page.locator("header.bar .estop")).toHaveText("AGENTS STOPPED");
  expect(api.calls).toEqual(["engage:", "engage:"]);
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("?");
  await expect(page.getByText("Stop every agent task (e-stop)")).toBeVisible();
});

test("a stop from before shows on load, with the server's reason as text; the console clears it", async ({ page }) => {
  const api = estopApi(page, { since: "2026-11-23T19:00:00Z", by: "human", reason: P });
  await page.goto("/#traffic");
  const banner = page.locator(".halt-banner");
  await expect(banner).toContainText(P);
  expect(await page.locator(".halt-banner img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await page.getByLabel("Console command").fill("estop clear");
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("e-stop cleared: new tasks may be approved. Halted tasks stay halted.");
  await expect(banner).toHaveCount(0);
  await page.getByLabel("Console command").fill("estop odd 500s from the target");
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("e-stop engaged: 2 agent tasks halted for good.");
  expect(api.calls).toEqual(["clear", "engage:odd 500s from the target"]);
});
