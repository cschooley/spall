// The Replays tab, chains and the replay: filter (#21). Entries are
// appended to the tailed log like taphttp would record them; lever-mode
// history is mocked where a test needs it.
import type { Page } from "@playwright/test";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { TrafficEntry } from "../src/types.ts";

const tab = (page: Page) => page.getByRole("tab", { name: /replays/ });

async function open(page: Page, path: string) {
  await page.locator("tr.row").filter({ hasText: path }).first().click();
  await expect(page.locator(".inspector h2")).toContainText(path);
}

function later(e: TrafficEntry, seconds: number): string {
  return new Date(Date.parse(e.ts) + seconds * 1000).toISOString();
}

test("the Replays tab counts and lists replays live, and links both ways", async ({ page }) => {
  const t = token();
  const orig = entry({ method: "POST", host: `${t}.example`, url: `http://${t}.example/orig`, req_body: "{}" });
  append(orig);
  await page.goto("/#traffic");
  await filterTo(page, t);
  await open(page, "/orig");
  await expect(tab(page)).toHaveText("replays0");
  await tab(page).click();
  await expect(page.locator(".pane")).toContainText("No replays yet");
  await expect(page.locator(".pane"), "log-file mode").toContainText("loaded replays only");

  // A replay arrives on the live feed: counted and listed without reopening.
  const r1 = entry({ ...orig, id: `${orig.id}-r1`, url: `http://${t}.example/orig`, ts: later(orig, 1), replay_of: orig.id, status: 201, meta: { actor: "human" } });
  append(r1);
  await expect(tab(page)).toHaveText("replays1");
  const rows = page.locator(".replays li");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("201 Created");
  await expect(rows.first()).toContainText("as captured");
  await expect(rows.first()).toContainText("human");

  // An edited one, newer: listed first, with what changed.
  const r2 = entry({
    ...orig,
    id: `${orig.id}-r2`,
    ts: later(orig, 2),
    method: "PUT",
    req_headers: [...orig.req_headers, ["x-role", "admin"]],
    replay_of: orig.id,
  });
  append(r2);
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText("method PUT · 1 header");

  // Open a replay from the list; "replay of" goes back.
  await rows.nth(1).locator("button[title^='Open ']").click();
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${orig.id}`);
  await page.locator(".inspector .meta button.orig").click();
  await expect(page.locator(".inspector .meta button.orig")).toHaveCount(0);
});

test("a replay of a replay shows the chain back to the first original", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/chain` });
  const r1 = entry({ ...orig, id: `${orig.id}-r1`, ts: later(orig, 1), replay_of: orig.id });
  const r2 = entry({ ...orig, id: `${orig.id}-r2`, ts: later(orig, 2), replay_of: r1.id });
  append(orig, r1, r2);
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").last().click(); // oldest first: the last row is r2
  const chain = page.getByLabel("Replay chain");
  await expect(chain).toContainText(`chain: ${orig.id.slice(0, 8)}`);
  await expect(chain).toContainText("(original)");
  await expect(chain.locator(".here")).toHaveText("this");
  await chain.getByRole("button").first().click();
  await expect(page.locator(".inspector .meta button.orig"), "opened the first original").toHaveCount(0);
  await expect(tab(page), "its own replays: r1 only, not r1's").toHaveText("replays1");

  // A one-step replay doesn't need a chain line.
  await tab(page).click();
  await page.locator(".replays li button[title^='Open ']").first().click();
  await expect(page.getByLabel("Replay chain")).toHaveCount(0);
});

test("with the lever API, the list comes from taphttp's replay_of history", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/hist` });
  append(orig);
  const old = entry({ ...orig, id: `${orig.id}-old`, ts: later(orig, 5), replay_of: orig.id, status: 500 });
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "lever", connected: true, proxy: "127.0.0.1:8080", replay: true, scope: [] } }),
  );
  const asked: string[] = [];
  await page.route("**/api/exchanges?*", (r) => {
    const q = new URL(r.request().url()).searchParams;
    asked.push(q.get("replay_of") ?? "");
    return r.fulfill({ json: q.get("replay_of") === orig.id ? [old] : [] });
  });
  await page.goto("/#traffic");
  await filterTo(page, t);
  await open(page, "/hist");
  await expect(tab(page)).toHaveText("replays1");
  await tab(page).click();
  await expect(page.locator(".pane")).toContainText("newest first");
  await expect(page.locator(".pane")).not.toContainText("loaded replays only");
  await expect(page.locator(".replays li").first()).toContainText("500");
  expect(asked).toContain(orig.id);
});

test("replay:yes and replay:no filter the table", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/f-orig` });
  const r1 = entry({ ...orig, id: `${orig.id}-r`, url: `http://${t}.example/f-rep`, ts: later(orig, 1), replay_of: orig.id });
  append(orig, r1);
  await page.goto("/#traffic");
  await filterTo(page, `${t} replay:yes`);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await expect(page.locator("tr.row")).toContainText("/f-rep");
  await filterTo(page, `${t} replay:no`);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await expect(page.locator("tr.row")).toContainText("/f-orig");
});

test("hostile replay fields stay text in the list", async ({ page }) => {
  const t = token();
  const payload = `<img src=x onerror="window.__pwned=1">`;
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/h` });
  const r1 = entry({ ...orig, id: `${orig.id}-r`, ts: later(orig, 1), replay_of: orig.id, status: null, error: payload, meta: { actor: payload } });
  append(orig, r1);
  await page.goto("/#traffic");
  await filterTo(page, `${t} replay:no`);
  await open(page, "/h");
  await tab(page).click();
  await expect(page.locator(".replays .who")).toHaveText(payload);
  await expect(page.locator(".replays .error")).toHaveText(payload);
  expect(await page.locator(".inspector img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
