// M0 console behavior: live updates, filter, clear, follow.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

test("new log lines appear live, with the connection shown as live", async ({ page }) => {
  const t = token();
  await page.goto("/");
  await expect(page.locator(".conn")).toHaveText("live");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(0);

  append(entry({ host: `${t}.example`, method: "POST", url: `http://${t}.example/login`, status: 401 }));
  const row = page.locator("tr.row");
  await expect(row).toHaveCount(1);
  await expect(row.locator("td").nth(1)).toHaveText("POST");
  await expect(row.locator("td").nth(3)).toHaveText("/login");
  await expect(row.locator("td").nth(4)).toHaveText("401");
  await expect(row.locator("td").nth(4)).toHaveClass("st-4");
  await expect(row.locator("td").nth(5)).toHaveText("12 ms");
});

test("filter matches method, host, path and status", async ({ page }) => {
  const t = token();
  append(
    entry({ host: `${t}.a`, method: "GET", url: `http://${t}.a/one`, status: 200 }),
    entry({ host: `${t}.b`, method: "DELETE", url: `http://${t}.b/two`, status: 503 }),
  );
  await page.goto("/");
  for (const [q, n] of [
    [t, 2],
    [`${t}.b`, 1],
    [`delete ${t}`, 1],
    [`/one`, 1],
    [`${t}.b /two 503`, 1],
    [`${t}-nothing`, 0],
  ] as const) {
    await filterTo(page, q);
    await expect(page.locator("tr.row"), q).toHaveCount(n);
  }
});

test("clear empties the table and new traffic still arrives", async ({ page }) => {
  const t = token();
  append(entry({ host: `${t}.before` }));
  await page.goto("/");
  await expect(page.locator("tr.row").filter({ hasText: `${t}.before` })).toHaveCount(1);
  await page.getByRole("button", { name: "clear" }).click();
  await expect(page.locator("tr.row")).toHaveCount(0);
  await expect(page.locator(".count")).toHaveText("0 requests");
  append(entry({ host: `${t}.after` }));
  await expect(page.locator("tr.row")).toHaveCount(1);
  await expect(page.locator("tr.row")).toContainText(`${t}.after`);
});

test("scrolling up stops follow; scrolling to the bottom resumes it", async ({ page }) => {
  const t = token();
  append(...Array.from({ length: 150 }, (_, i) => entry({ host: `${t}.example`, url: `http://${t}.example/${i}` })));
  await page.goto("/");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(150);
  const follow = page.getByLabel("follow");
  await expect(follow).toBeChecked();
  const main = page.locator("main");
  await expect
    .poll(() => main.evaluate((m) => m.scrollHeight - m.scrollTop - m.clientHeight))
    .toBeLessThan(24);

  await main.hover();
  await page.mouse.wheel(0, -2000);
  await expect(follow).not.toBeChecked();

  await main.evaluate((m) => (m.scrollTop = m.scrollHeight));
  await expect(follow).toBeChecked();
});
