// CSRF (#17): a page on another origin must not be able to make spall
// replay anything. The attacker page is real HTML from a second local
// server, so the browser sends its genuine Origin and Sec-Fetch-Site.
//
// These runs use spall in log-file mode, where a request that got past
// the guard ends at replay's 503 ("needs the lever API"). The guard's
// answer is a 403, so the status tells the two apart.
//
// Plain @playwright/test, not ./fixtures.ts: the attacker page is meant to
// fail, so its console errors are expected here.
import { expect, test, type Page, type Response } from "@playwright/test";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

let attacker: Server;
let attackerPort = 0;

test.beforeAll(async () => {
  attacker = createServer((req, res) => {
    const target = new URL(req.url ?? "/", "http://x").searchParams.get("target") ?? "";
    const action = JSON.stringify(`${target}/api/replay`);
    const pages: Record<string, string> = {
      // The classic: a text/plain form whose single field reads as JSON.
      "/form": `<form method="post" enctype="text/plain" action=${action}>
        <input name='{"id":"e1","x":"' value='"}'></form>
        <script>document.forms[0].submit()</script>`,
      // no-cors drops the non-safelisted Content-Type: the request goes as text/plain.
      "/no-cors": `<script>fetch(${action}, {method: "POST", mode: "no-cors",
        headers: {"Content-Type": "application/json"}, body: '{"id":"e1"}'}).catch(() => {})</script>`,
      // A real JSON request needs a preflight, which spall never approves.
      // Chromium runs preflights internally (Playwright doesn't see them),
      // so the page reports the outcome in its title.
      "/cors": `<script>fetch(${action}, {method: "POST",
        headers: {"Content-Type": "application/json"}, body: '{"id":"e1"}'})
        .then((r) => { document.title = "status " + r.status }, () => { document.title = "blocked" })</script>`,
    };
    const body = pages[new URL(req.url ?? "/", "http://x").pathname];
    res.writeHead(body ? 200 : 404, { "content-type": "text/html" });
    res.end(body ? `<!doctype html><title>attacker</title>${body}` : "");
  });
  await new Promise<void>((done) => attacker.listen(0, "127.0.0.1", done));
  attackerPort = (attacker.address() as AddressInfo).port;
});

test.afterAll(async () => {
  await new Promise((done) => attacker.close(done));
});

/** Every request the page made to spall's /api/replay, with its response. */
function replayTraffic(page: Page) {
  const seen: { method: string; status: number | null }[] = [];
  const pending: Promise<void>[] = [];
  page.on("request", (r) => {
    if (!r.url().endsWith("/api/replay")) return;
    pending.push(
      r.response().then(
        (res: Response | null) => void seen.push({ method: r.method(), status: res?.status() ?? null }),
        () => void seen.push({ method: r.method(), status: null }),
      ),
    );
  });
  return async () => {
    await Promise.all(pending);
    return seen;
  };
}

for (const [label, host] of [
  ["cross-origin, same site", "127.0.0.1"],
  ["cross-site", "localhost"],
] as const) {
  for (const attack of ["form", "no-cors", "cors"] as const) {
    test(`${label}: a ${attack} POST is refused`, async ({ page, baseURL }) => {
      const traffic = replayTraffic(page);
      const url = `http://${host}:${attackerPort}/${attack}?target=${encodeURIComponent(baseURL ?? "")}`;
      if (attack === "cors") {
        await page.goto(url);
        await expect(page, "the preflight is refused, so the fetch fails").toHaveTitle("blocked");
        await page.waitForTimeout(300);
        const seen = await traffic();
        expect(
          seen.filter((s) => s.status !== null),
          "no POST ever got an answer from spall",
        ).toEqual([]);
        return;
      }
      const done = page.waitForResponse((r) => r.url().endsWith("/api/replay"));
      await page.goto(url);
      expect((await done).status(), `${attack} from ${host}`).toBe(403);
      await page.waitForTimeout(300);
      expect(await traffic(), "one POST, refused by the guard").toEqual([{ method: "POST", status: 403 }]);
    });
  }
}

test("control: spall's own page gets past the guard", async ({ page }) => {
  await page.goto("/");
  const status = await page.evaluate(async () => {
    const res = await fetch("/api/replay", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: '{"id":"e1"}',
    });
    return res.status;
  });
  expect(status, "503: reached replay, which needs the lever API").toBe(503);
});
