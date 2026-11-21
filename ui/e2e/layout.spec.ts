// Key columns must never be cut off (the M0 time-column bug).
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

for (const width of [1440, 1100, 800]) {
  test(`time, status and duration are not truncated at ${width}px`, async ({ page }) => {
    const t = token();
    append(
      entry({ host: `${t}.example`, status: 503, duration_ms: 123456 }),
      entry({ host: `${t}.example`, status: null, duration_ms: null }),
    );
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/");
    await filterTo(page, t);
    await expect(page.locator("tr.row")).toHaveCount(2);
    const clipped = await page.locator("tr.row td").evaluateAll((cells) =>
      cells
        .filter((_, i) => [0, 4, 5].includes(i % 6))
        .filter((c) => c.scrollWidth > c.clientWidth)
        .map((c) => c.textContent),
    );
    expect(clipped).toEqual([]);
  });
}
