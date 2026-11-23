// Key columns must never be cut off (the M0 time-column bug), with or
// without the inspector open.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

for (const [width, inspector] of [
  [1440, false],
  [1100, false],
  [800, false],
  [1440, true],
  [1100, true],
] as const) {
  test(`time, status and ms are not truncated at ${width}px${inspector ? " with the inspector open" : ""}`, async ({
    page,
  }) => {
    const t = token();
    append(
      entry({ host: `${t}.example`, status: 503, duration_ms: 123456 }),
      entry({ host: `${t}.example`, status: null, duration_ms: null }),
    );
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/#traffic");
    await filterTo(page, t);
    await expect(page.locator("tr.row")).toHaveCount(2);
    if (inspector) await page.locator("tr.row").first().click();
    const clipped = await page.locator("tr.row td").evaluateAll((cells) =>
      cells
        .filter((_, i) => [0, 4, 5].includes(i % 6))
        .filter((c) => c.scrollWidth > c.clientWidth)
        .map((c) => c.textContent),
    );
    expect(clipped).toEqual([]);
  });
}
