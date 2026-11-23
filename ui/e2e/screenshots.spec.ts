// Screenshots for the README and for side-by-side review against the
// verdigris mockup (#12). Not a pixel diff: run `npm run screenshots`, then
// compare docs/screenshots/traffic.png with mockups/spall-console.html at
// the same size (1440x820). Skipped in normal test runs. spall runs in log
// mode here, so the engagement, hosts and status APIs are mocked
// (engagement-api.ts) to look like a lever-connected spall mid-engagement.
import { append, entry, expect, test } from "./fixtures.ts";
import { days, engagementApi } from "./engagement-api.ts";
import type { Page } from "@playwright/test";
import type { TrafficEntry } from "../src/types.ts";

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
        // Fake fixture token ("sig" is not a signature), shown in the screenshots.
        // nosemgrep: generic.secrets.security.detected-jwt-token.detected-jwt-token
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

const SCOPE = ["*.example.test", "!cdn.example.test"];

const HOSTS = [
  ["shop.example.test", 7, "in", "*.example.test"],
  ["api.example.test", 5, "in", "*.example.test"],
  ["example.com", 1, "unknown", null],
  ["www.google-analytics.com", 4, "unknown", null],
  ["fonts.gstatic.com", 2, "unknown", null],
  ["cdn.example.test", 1, "out", "!cdn.example.test"],
] as const;

// A lever-connected spall with an engagement under way. `traffic` is what
// taphttp's history holds (the same entries are appended to the log).
async function midEngagement(page: Page, traffic: TrafficEntry[] = []) {
  const api = engagementApi(page, {
    name: "Example Shop Q4",
    client: "Example Corp",
    expires: days(12),
    scope: SCOPE,
    notes: "Staging only. VPN through the jump host; SSO is out of scope.",
    target_notes: { "*.example.test": "staging; prod is out" },
    rules: {
      methods: [],
      safe_methods: ["GET", "HEAD", "OPTIONS"],
      rate_limit: 5,
      time_zone: "America/Chicago",
      windows: [{ days: ["mon", "tue", "wed", "thu", "fri"], start: "08:00", end: "18:00" }],
      blackouts: [{ from: "2026-12-24T00:00", to: "2026-12-27T00:00", note: "holiday freeze" }],
      agents_allowed: false,
      destructive_allowed: false,
    },
  });
  const now = Date.now();
  const hosts = HOSTS.map(([host, count, state, by], i) => ({
    host,
    count,
    first_seen: new Date(now - 3_600_000).toISOString(),
    last_seen: new Date(now - i * 60_000).toISOString(),
    state,
    by,
  }));
  const counts = { in: 2, out: 1, unknown: 3 };
  await page.route("**/api/hosts", (r) => r.fulfill({ json: { hosts, counts, dropped: 0 } }));
  await page.route("**/api/exchanges?*", (r) => {
    const q = new URL(r.request().url()).searchParams;
    const of = q.get("replay_of");
    const page1 = q.get("before") ? [] : traffic.filter((e) => (of ? e.replay_of === of : true));
    return r.fulfill({ json: [...page1].sort((a, b) => b.ts.localeCompare(a.ts)) });
  });
  await page.route("**/api/ca", (r) =>
    r.fulfill({
      json: {
        sha256: "3F:9C:A1:07:5B:E2:48:D0:6C:91:2E:7F:B4:05:C8:63:1A:DE:90:47:F2:3B:6D:85:0C:E9:71:2A:B6:4F:D8:13",
        subject: "CN=taphttp CA, O=taphttp",
        not_before: "2026-11-01T00:00:00Z",
        not_after: "2036-11-01T00:00:00Z",
        proxy: "127.0.0.1:8080",
      },
    }),
  );
  // Registered last, so it wins over engagementApi's log-mode status.
  await page.route("**/api/status", (r) =>
    r.fulfill({
      json: {
        source: "lever",
        connected: true,
        proxy: "127.0.0.1:8080",
        replay: true,
        scope: api.e.scope,
        scope_all: false,
        engagement: { id: api.e.id, name: api.e.name },
        testing: { allowed: true, reason: null },
      },
    }),
  );
  return api;
}

test("traffic view", async ({ page }) => {
  const rows = mockupTraffic();
  await midEngagement(page, rows);
  append(...rows);
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

test("replay compared with its original", async ({ page }) => {
  const rows = mockupTraffic();
  const orig = rows[2]!;
  const replay = entry({
    ...orig,
    id: `${orig.id}R`,
    ts: new Date(Date.parse(orig.ts) + 90_000).toISOString(),
    replay_of: orig.id,
    status: 200,
    duration_ms: 152,
    req_headers: orig.req_headers.map(([k, v]) => [k, k === "cookie" ? "sid=0a77e1f9b3c2; csrf=11c0e7" : v] as [string, string]),
    req_body: JSON.stringify({ username: "admin", password: "admin", remember: true }),
    res_headers: [["content-type", "application/json"], ["set-cookie", "sid=5d2c0b; Path=/; HttpOnly; Secure"]],
    res_body: JSON.stringify({ ok: true, user: { id: 1, name: "admin", role: "owner" } }),
  });
  await midEngagement(page, [...rows, replay]);
  append(...rows, replay);
  await page.setViewportSize({ width: 1440, height: 820 });
  await page.goto("/");
  await expect(page.locator("tr.row")).toHaveCount(15);
  await page.evaluate(() => document.fonts.ready);
  await page.locator("tr.row").filter({ hasText: "/api/v2/session" }).filter({ hasText: "401" }).first().click();
  await page.getByRole("tab", { name: /replays/ }).click();
  await page.locator(".replays li").first().getByRole("button", { name: "Compare" }).click();
  await expect(page.locator(".compare .verdict")).toBeVisible();
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${OUT}replay-compare.png` });
});

test("engagement view", async ({ page }) => {
  const rows = mockupTraffic();
  await midEngagement(page, rows);
  append(...rows);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/#engagement");
  await expect(page.getByRole("heading", { name: "Example Shop Q4" })).toBeVisible();
  await expect(page.getByRole("list", { name: "Undecided hosts" }).locator("li")).toHaveCount(3);
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${OUT}engagement.png` });
});

test("paste a scope list", async ({ page }) => {
  await midEngagement(page);
  await page.route("**/api/scope/parse", (r) =>
    r.fulfill({
      json: {
        items: [
          { line: 2, source: "https://shop.example.test/login", entry: "shop.example.test", exclude: false, error: null, notes: ['path "/login" dropped: scope is by host', "already covered by *.example.test"], present: false },
          { line: 3, source: "api.example.test (REST, v2)", entry: "api.example.test", exclude: false, error: null, notes: ["aside dropped"], present: false },
          { line: 4, source: "10.20.0.0/16", entry: "10.20.0.0/16", exclude: false, error: null, notes: [], present: false },
          { line: 5, source: "*.example.test", entry: "*.example.test", exclude: false, error: null, notes: ["already in the scope"], present: true },
          { line: 7, source: "- sso.example.test", entry: "sso.example.test", exclude: true, error: null, notes: [], present: false },
          { line: 8, source: "payments via Stripe", entry: null, exclude: false, error: "not a hostname, *.wildcard, IP or CIDR", notes: [], present: false },
        ],
        add: ["shop.example.test", "api.example.test", "10.20.0.0/16"],
        exclude: ["sso.example.test"],
      },
    }),
  );
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/#engagement/paste");
  const panel = page.getByRole("group", { name: "Paste a scope list" });
  await panel
    .getByLabel("Scope list")
    .fill("In scope:\n1. https://shop.example.test/login\n2. api.example.test (REST, v2)\n3. 10.20.0.0/16\n4. *.example.test\nOut of scope:\n- sso.example.test\npayments via Stripe");
  await expect(panel.getByRole("table", { name: "Preview" }).locator("tbody tr")).toHaveCount(6);
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${OUT}engagement-paste.png` });
});
