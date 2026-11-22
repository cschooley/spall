// The ` console (#11): one command registry shared with the palette.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

const openConsole = async (page: import("@playwright/test").Page) => {
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await expect(page.getByLabel("Console command")).toBeFocused();
};

const run = async (page: import("@playwright/test").Page, line: string) => {
  await page.getByLabel("Console command").fill(line);
  await page.keyboard.press("Enter");
};

test("` opens and closes the console, but not while typing in the filter", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Filter").click();
  await page.keyboard.press("`");
  await expect(page.getByLabel("Filter")).toHaveValue("`");
  await expect(page.locator(".console")).toHaveCount(0);
  await page.getByLabel("Filter").fill("");
  await page.keyboard.press("Escape");

  await openConsole(page);
  await expect(page.locator(".con-log")).toContainText("spall console · type help");
  await run(page, "help");
  for (const usage of ["filter [query]", "open <id>", "curl", "theme <name>", "goto <view>", "console-key <char>"]) {
    await expect(page.locator(".con-log")).toContainText(usage);
  }
  await expect(page.locator(".con-log")).not.toContainText("god");
  await page.keyboard.press("`");
  await expect(page.locator(".console")).toHaveCount(0);
});

test("console commands drive the same actions as the UI", async ({ page }) => {
  const t = token();
  append(entry({ id: `${t}-one`, host: `${t}.a`, url: `http://${t}.a/one` }), entry({ host: `${t}.b` }));
  await page.goto("/");
  await openConsole(page);

  await run(page, `filter host:${t}.a`);
  await expect(page.getByLabel("Filter")).toHaveValue(`host:${t}.a`);
  await expect(page.locator(".tablewrap")).toHaveAttribute("data-filter", `host:${t}.a`);
  await expect(page.locator("tr.row")).toHaveCount(1);

  await run(page, `open ${t}-one`);
  await expect(page.locator(".inspector h2")).toHaveText(`GET http://${t}.a/one`);

  await run(page, "theme blued-light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "blued-light");
  await run(page, "goto findings");
  await run(page, "frobnicate");
  const log = page.locator(".con-log");
  await expect(log.locator(".l-warn")).toHaveText(["goto: findings is not built yet", "unknown command: frobnicate. try help"]);
  await run(page, "god");
  await expect(log.locator(".l-out").last()).toHaveText("no.");
  await run(page, "clear");
  await expect(log.locator("div")).toHaveCount(0);
  await page.evaluate(() => localStorage.clear());
});

test("history (up/down) and tab completion", async ({ page }) => {
  await page.goto("/");
  await openConsole(page);
  const input = page.getByLabel("Console command");
  await run(page, "goto traffic");
  await run(page, "help");
  await page.keyboard.press("ArrowUp");
  await expect(input).toHaveValue("help");
  await page.keyboard.press("ArrowUp");
  await expect(input).toHaveValue("goto traffic");
  await page.keyboard.press("ArrowDown");
  await expect(input).toHaveValue("help");

  await input.fill("th");
  await page.keyboard.press("Tab");
  await expect(input).toHaveValue("theme ");
  await input.fill("theme blu");
  await page.keyboard.press("Tab");
  await expect(page.locator(".con-log .l-sys").last()).toHaveText("blued  blued-light");
  await input.fill("theme blued-l");
  await page.keyboard.press("Tab");
  await expect(input).toHaveValue("theme blued-light ");
});

test("system messages while closed show an unread count in the status bar", async ({ page }) => {
  const t = token();
  await page.goto("/");
  await expect(page.locator(".con-unread")).toHaveCount(0);
  await filterTo(page, `by:agent ${t}`);
  const badge = page.locator(".con-unread");
  await expect(badge).toHaveText("` 1");
  await badge.click();
  await expect(page.locator(".con-log .l-warn")).toContainText("filter: ignored by:agent");
  await page.keyboard.press("Escape");
  await expect(badge).toHaveCount(0);
});

test("the toggle key is configurable (non-US layouts)", async ({ page }) => {
  await page.goto("/");
  await openConsole(page);
  await run(page, "console-key \\");
  await expect(page.locator(".con-log .l-sys").last()).toHaveText("console key is now \\");
  await page.keyboard.press("Escape");
  await expect(page.locator(".console")).toHaveCount(0);
  await page.reload();
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("\\");
  await expect(page.getByLabel("Console command")).toBeFocused();
  await expect(page.locator(".con-hint")).toContainText("\\ close");
  await page.evaluate(() => localStorage.clear());
});

test("? opens the shortcut sheet (working keys only); Esc closes", async ({ page }) => {
  await page.goto("/");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet).toBeVisible();
  for (const what of ["Command palette", "Console", "Switch view", "Light / dark", "Filter", "Next / previous row", "Hex view (binary body)", "Replay"]) {
    await expect(sheet).toContainText(what);
  }
  for (const later of ["Flag candidate", "Task agent", "Halt"]) {
    await expect(sheet).not.toContainText(later);
  }
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
});
