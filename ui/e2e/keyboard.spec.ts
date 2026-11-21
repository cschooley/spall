// Keyboard first (docs/ux.md): / filter, j/k move, Esc close, t light/dark,
// Ctrl+K command palette.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

test.use({ permissions: ["clipboard-read", "clipboard-write"] });

test("j/k move the selection, Esc closes, / focuses the filter, t toggles light/dark", async ({ page }) => {
  const t = token();
  append(...[0, 1, 2].map((i) => entry({ host: `${t}.example`, url: `http://${t}.example/${i}` })));
  await page.goto("/");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(3);
  await page.locator(".tablewrap").click({ position: { x: 5, y: 200 } });

  await page.keyboard.press("j");
  await expect(page.locator("tr.row.sel")).toContainText("/0");
  await page.keyboard.press("j");
  await expect(page.locator("tr.row.sel")).toContainText("/1");
  await page.keyboard.press("k");
  await expect(page.locator("tr.row.sel")).toContainText("/0");
  await expect(page.locator(".inspector h2")).toContainText("/0");
  await page.keyboard.press("Escape");
  await expect(page.locator(".inspector")).toHaveCount(0);

  await page.keyboard.press("/");
  await expect(page.getByLabel("Filter")).toBeFocused();
  await page.keyboard.press("j"); // typing, not moving
  await expect(page.getByLabel("Filter")).toHaveValue(`${t}j`);
  await page.keyboard.press("Escape");
  await expect(page.getByLabel("Filter")).not.toBeFocused();

  await page.keyboard.press("t");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "rust-light");
  await page.keyboard.press("t");
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /.+/);
  await page.evaluate(() => localStorage.clear());
});

test("Ctrl+K palette: filter commands, run with Enter, Esc closes", async ({ page }) => {
  const t = token();
  append(entry({ host: `${t}.example`, url: `https://${t}.example/p` }));
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();

  await page.keyboard.press("Control+k");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await expect(palette.locator(".pal-foot")).toContainText("move");
  await page.keyboard.type("copy as");
  await expect(palette.getByRole("option")).toHaveCount(1);
  await expect(palette.getByRole("option")).toContainText("Copy as curl");
  await page.keyboard.press("Enter");
  await expect(palette).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`curl 'https://${t}.example/p' -H 'host: example.test'`);

  await page.keyboard.press("Control+k");
  await page.keyboard.type("zzz");
  await expect(palette.getByRole("option")).toHaveCount(0);
  await expect(palette.getByRole("listbox")).toHaveText("No match. The console takes any command: press `");
  await page.keyboard.press("Escape");
  await expect(palette).toHaveCount(0);
});

test("palette: commands that need arguments open the console prefilled (open by id from history)", async ({ page }) => {
  const t = token();
  const remote = entry({ id: `${t}-remote`, host: `${t}.remote`, url: `http://${t}.remote/old` });
  await page.route("**/api/status", (r) => r.fulfill({ json: { source: "lever", connected: true } }));
  await page.route(/\/api\/exchanges\?/, (r) => r.fulfill({ json: [] }));
  await page.route(`**/api/exchanges/${t}-remote`, (r) => r.fulfill({ json: remote }));
  await page.goto("/");
  await page.keyboard.press("Control+k");
  await page.keyboard.type("open exchange");
  await page.keyboard.press("Enter");
  const input = page.getByLabel("Console command");
  await expect(input).toBeFocused();
  await expect(input).toHaveValue("open ");
  await page.keyboard.type(`${t}-remote`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".inspector h2")).toHaveText(`GET http://${t}.remote/old`);
  await expect(page.locator(".con-log")).toContainText(`open ${t}-remote`);
});
