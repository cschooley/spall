// Traffic table behavior: live updates, filter syntax, clear, follow.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

test("new log lines appear live, with the connection shown as live", async ({ page }) => {
  const t = token();
  await page.goto("/#traffic");
  await expect(page.locator(".conn")).toHaveText("live");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(0);
  await expect(page.locator(".empty")).toHaveText("No exchanges match this filter.");

  append(entry({ host: `${t}.example`, method: "POST", url: `http://${t}.example/login`, status: 401 }));
  const row = page.locator("tr.row");
  await expect(row).toHaveCount(1);
  await expect(row.locator("td").nth(1)).toHaveText("POST");
  await expect(row.locator("td").nth(1)).toHaveClass("m m-POST");
  await expect(row.locator("td").nth(3)).toHaveText("/login");
  await expect(row.locator("td").nth(4)).toHaveText("401");
  await expect(row.locator("td").nth(4)).toHaveClass("st-4");
  await expect(row.locator("td").nth(5)).toHaveText("12");
});

test("filter syntax: host:, method:, status: (exact and class), free text", async ({ page }) => {
  const t = token();
  append(
    entry({ host: `${t}.a`, method: "GET", url: `http://${t}.a/one`, status: 200 }),
    entry({ host: `${t}.b`, method: "DELETE", url: `http://${t}.b/two`, status: 503 }),
  );
  await page.goto("/#traffic");
  for (const [q, n] of [
    [t, 2],
    [`host:${t}.b`, 1],
    [`method:delete ${t}`, 1],
    [`${t}.a/one`, 1],
    [`status:503 ${t}`, 1],
    [`status:5xx ${t}`, 1],
    [`status:2xx ${t}`, 1],
    [`method:put ${t}`, 0],
    [`${t}-nothing`, 0],
  ] as const) {
    await filterTo(page, q);
    await expect(page.locator("tr.row"), q).toHaveCount(n);
  }
  // by:agent is attribution now (#62); an unknown term is still reported.
  await filterTo(page, `by:robot ${t}`);
  await expect(page.locator(".warn")).toHaveText("ignored: by:robot");
});

test("clear empties the table and new traffic still arrives", async ({ page }) => {
  const t = token();
  append(entry({ host: `${t}.before` }));
  await page.goto("/#traffic");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await page.getByRole("button", { name: "clear", exact: true }).click();
  await expect(page.locator("tr.row")).toHaveCount(0);
  append(entry({ host: `${t}.after` }));
  await expect(page.locator("tr.row")).toHaveCount(1);
  await expect(page.locator("tr.row")).toContainText(`${t}.after`);
});

test("scrolling up stops follow; scrolling to the bottom resumes it", async ({ page }) => {
  const t = token();
  append(...Array.from({ length: 150 }, (_, i) => entry({ host: `${t}.example`, url: `http://${t}.example/${i}` })));
  await page.goto("/#traffic");
  await filterTo(page, t);
  const follow = page.getByLabel("follow");
  await expect(follow).toBeChecked();
  const wrap = page.locator(".tablewrap");
  await expect(page.locator("tr.row").last()).toContainText("/149");
  await expect.poll(() => wrap.evaluate((m) => m.scrollHeight - m.scrollTop - m.clientHeight)).toBeLessThan(24);

  await wrap.hover();
  await page.mouse.wheel(0, -1500);
  await expect(follow).not.toBeChecked();
  await wrap.evaluate((m) => (m.scrollTop = m.scrollHeight));
  await expect(follow).toBeChecked();
});

test("the table is virtualized: thousands of rows, a bounded DOM", async ({ page }) => {
  const t = token();
  append(...Array.from({ length: 3000 }, (_, i) => entry({ host: `${t}.bulk`, url: `http://${t}.bulk/${i}` })));
  await page.goto("/#traffic");
  await filterTo(page, t);
  await expect(page.locator(".statusbar")).toContainText("3000 exchanges");
  expect(await page.locator("tr.row").count()).toBeLessThan(150);
  await expect(page.locator("tr.row").last()).toContainText("/2999");
});
