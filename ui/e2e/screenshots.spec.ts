// Screenshots for the README and for side-by-side review against the
// verdigris mockup (#12). Not a pixel diff: run `npm run screenshots`, then
// compare docs/screenshots/traffic.png with mockups/spall-console.html at
// the same size (1440x820). Skipped in normal test runs.
import { append, entry, expect, test } from "./fixtures.ts";

test.skip(!process.env.SPALL_SCREENSHOTS, "set SPALL_SCREENSHOTS=1 (npm run screenshots)");

const OUT = new URL("../../docs/screenshots/", import.meta.url).pathname;

function mockupTraffic() {
  const start = Date.now() - 60_000;
  const rows: [string, string, string, number, number][] = [
    ["GET", "example.com", "/", 200, 42],
    ["GET", "shop.example.test", "/static/app.3f9c.js", 200, 18],
    ["POST", "shop.example.test", "/api/v2/session", 401, 184],
    ["GET", "shop.example.test", "/api/v2/products?page=2", 200, 96],
    ["OPTIONS", "api.example.test", "/v1/orders", 204, 9],
    ["PUT", "api.example.test", "/v1/cart/8841", 200, 131],
    ["GET", "shop.example.test", "/account", 302, 27],
    ["DELETE", "api.example.test", "/v1/cart/8841/items/3", 204, 64],
    ["PATCH", "api.example.test", "/v1/users/me", 403, 58],
    ["GET", "cdn.example.test", "/img/hero@2x.webp", 304, 12],
    ["POST", "shop.example.test", "/api/v2/session", 200, 171],
    ["GET", "shop.example.test", "/api/v2/me", 200, 44],
    ["POST", "api.example.test", "/v1/graphql", 500, 911],
    ["POST", "shop.example.test", "/api/v2/session/refresh", 401, 66],
  ];
  return rows.map(([method, host, path, status, ms], i) =>
    entry({
      id: `01JD8Q${String(i).padStart(2, "0")}K${"X".repeat(18)}7F${i}`,
      ts: new Date(start + i * 2200).toISOString(),
      method,
      host,
      url: `https://${host}:443${path}`,
      status,
      duration_ms: ms,
      req_headers: [
        ["host", host],
        ["content-type", "application/json"],
        ["authorization", "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhZG1pbiJ9.sig"],
        ["cookie", "sid=8f3a91c2d4e5; csrf=11c0e7"],
        ["user-agent", "spall-agent/0.1"],
      ],
      req_body: method === "POST" ? JSON.stringify({ username: "admin", password: "hunter2", remember: true }) : null,
      res_headers: [["content-type", "application/json"]],
      res_body: '{"error":"invalid_credentials"}',
    }),
  );
}

test("traffic view", async ({ page }) => {
  append(...mockupTraffic());
  await page.setViewportSize({ width: 1440, height: 820 });
  await page.goto("/");
  await expect(page.locator("tr.row")).toHaveCount(14);
  await page.evaluate(() => document.fonts.ready);
  await page.locator("tr.row").nth(2).click();
  await expect(page.locator(".inspector")).toBeVisible();
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${OUT}traffic.png` });

  await page.keyboard.press("`");
  await page.getByLabel("Console command").fill("help");
  await page.keyboard.press("Enter");
  await page.screenshot({ path: `${OUT}traffic-console.png` });
});
