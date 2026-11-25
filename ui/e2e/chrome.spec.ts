// App chrome from the verdigris mockup: brand mark, view rail, suite
// launcher. Unbuilt views and tools are visible placeholders, inert until
// they ship (docs/ux.md).
import { expect, test } from "./fixtures.ts";

test("the brand shows the spall mark from verdigris", async ({ page }) => {
  await page.goto("/#traffic");
  const mark = page.locator(".brand svg[data-icon='icons/spall']");
  await expect(mark).toBeVisible();
  expect(await mark.locator("path").count()).toBe(2);
  await expect(page.locator(".brand")).toHaveText("spall");
});

test("the rail shows all five views in order, all live", async ({ page }) => {
  await page.goto("/#traffic");
  const items = page.locator(".rail > *");
  await expect(items).toHaveCount(5);
  expect(await items.evaluateAll((els) => els.map((e) => e.getAttribute("data-view")))).toEqual([
    "engagement",
    "traffic",
    "surface",
    "tasks",
    "findings",
  ]);
  for (const view of ["engagement", "traffic", "surface", "tasks", "findings"]) {
    const icon = page.locator(`.rail [data-view='${view}'] svg[data-icon='icons/views/${view}']`);
    await expect(icon, view).toBeVisible();
    expect(await icon.locator("path, rect").count(), view).toBeGreaterThan(0);
  }
  const traffic = page.getByRole("link", { name: "Traffic" });
  await expect(traffic).toHaveAttribute("aria-current", "page");
  await expect(traffic).toHaveAttribute("title", "Traffic (2)");
  await expect(page.getByRole("link", { name: "Engagement" })).toHaveAttribute("title", "Engagement (1)");
  await expect(page.getByRole("link", { name: "Findings" })).toHaveAttribute("title", "Findings (5)");
  await expect(page.getByRole("link", { name: "Tasks" })).toHaveAttribute("title", "Tasks (4)");
  await expect(page.getByRole("link", { name: "Surface" })).toHaveAttribute("title", "Surface (3)");
  await expect(page.locator(".rail .todo")).toHaveCount(0);
  // 3 opens the Surface (#95).
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("3");
  await expect(page.getByRole("link", { name: "Surface" })).toHaveAttribute("aria-current", "page");
});

test("the launcher lists the suite; placeholders are off and disabled", async ({ page }) => {
  await page.route("**/api/status", (r) => r.fulfill({ json: { source: "lever", connected: true } }));
  await page.route(/\/api\/exchanges\?/, (r) => r.fulfill({ json: [] }));
  await page.goto("/#traffic");
  const button = page.getByRole("button", { name: "Switch tool" });
  await expect(button).toHaveAttribute("aria-expanded", "false");
  await button.click();
  const panel = page.getByRole("group", { name: "Suite tools" });
  await expect(panel).toBeVisible();

  const tiles = panel.locator(".tile");
  await expect(tiles).toHaveText(["spall", "kudzu", "scurf", "taphttp", "flux"]);
  const tile = (id: string) => panel.locator(`[data-tool='${id}']`);
  await expect(tile("spall")).toHaveAttribute("aria-current", "true");
  await expect(tile("spall").locator(".led")).toHaveClass("led up");
  await expect(tile("taphttp").locator(".led")).toHaveClass("led up");
  await expect(tile("taphttp")).toHaveAttribute("title", "taphttp: headless: status only · connected");
  for (const id of ["kudzu", "scurf"]) {
    await expect(tile(id)).toHaveAttribute("aria-disabled", "true");
    await expect(tile(id)).toHaveClass(/todo/);
    await expect(tile(id).locator(".led")).toHaveClass("led");
    await expect(tile(id)).toHaveAttribute("title", `${id}: not built yet`);
  }
  await expect(tile("flux")).toHaveAttribute("title", "flux: library: no service");
  for (const id of ["spall", "kudzu", "scurf", "taphttp", "flux"]) {
    await expect(tile(id).locator(`svg[data-icon='icons/${id}']`)).toBeVisible();
  }

  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await button.click();
  await page.locator(".tablewrap").click();
  await expect(panel).toHaveCount(0);
});

test("taphttp's light is off when spall tails a log file", async ({ page }) => {
  await page.goto("/#traffic");
  await page.getByRole("button", { name: "Switch tool" }).click();
  const taphttp = page.locator("[data-tool='taphttp']");
  await expect(taphttp.locator(".led")).toHaveClass("led");
  await expect(taphttp).toHaveAttribute("title", "taphttp: headless: status only · log file");
});

test("taphttp and flux tiles show their details in place (#16)", async ({ page }) => {
  const P = `<img src=x onerror="window.__pwned=1">`;
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "lever", connected: true, proxy: "127.0.0.1:8080", flux: { crate: "v0.1.0", spec: "draft v0.4", rules: "OWASP WSTG v4.2" } } }),
  );
  await page.route(/\/api\/exchanges\?/, (r) => r.fulfill({ json: [] }));
  await page.route("**/api/ca", (r) =>
    r.fulfill({ json: { sha256: "3F:9C:A1", subject: "CN=taphttp CA", not_after: "2036-11-01T00:00:00Z", taphttp_version: `0.6.1 ${P}`, proxy: "127.0.0.1:8080" } }),
  );
  await page.goto("/#traffic");
  await page.getByRole("button", { name: "Switch tool" }).click();
  const panel = page.getByRole("group", { name: "Suite tools" });
  await panel.getByRole("button", { name: /taphttp/ }).click();
  const tap = panel.getByRole("region", { name: "taphttp details" });
  await expect(tap).toContainText("lever API, feed connected");
  await expect(tap).toContainText("3F:9C:A1");
  await expect(tap).toContainText(`0.6.1 ${P}`);
  await expect(tap.getByRole("link", { name: "Set up a browser" })).toHaveAttribute("href", "#engagement/ca");
  await panel.getByRole("button", { name: /flux/ }).click();
  await expect(tap).toHaveCount(0);
  const flux = panel.getByRole("region", { name: "flux details" });
  await expect(flux).toContainText("v0.1.0");
  await expect(flux).toContainText("draft v0.4");
  await expect(flux).toContainText("OWASP WSTG v4.2");
  await panel.getByRole("button", { name: /flux/ }).click();
  await expect(flux).toHaveCount(0);
  expect(await page.locator(".panel img").count()).toBe(0);
});
