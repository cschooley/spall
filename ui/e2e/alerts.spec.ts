// Tester alerts (#86): off until switched on; then alerts raised while the
// page is open reach the console and the desktop, worded from kinds and ids
// only. Old ones (before the page opened) aren't news. /api/status and
// /api/alerts are mocked; Notification is stubbed to record what's shown.
import { expect, test } from "./fixtures.ts";

test("alerts: off by default, on with the browser's permission, content-free on the desktop", async ({ page }) => {
  const state = { seq: 5, asked: [] as string[] };
  await page.addInitScript(() => {
    const shown: { title: string; body: string }[] = [];
    (window as unknown as { __shown: typeof shown }).__shown = shown;
    class FakeNotification {
      static permission = "default";
      static requestPermission() {
        FakeNotification.permission = "granted";
        return Promise.resolve("granted");
      }
      onclick: (() => void) | null = null;
      constructor(title: string, opts: { body: string }) {
        shown.push({ title, body: opts.body });
      }
      close() {}
    }
    Object.defineProperty(window, "Notification", { value: FakeNotification });
  });
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "lever", connected: true, proxy: "127.0.0.1:8080", replay: true, scope: ["*"], engagement: { id: "acme-1a2b", name: "ACME" }, testing: { allowed: true, reason: null }, alerts: state.seq } }),
  );
  await page.route(/\/api\/alerts\?after=\d+$/, (r) => {
    const after = Number(new URL(r.request().url()).searchParams.get("after"));
    state.asked.push(String(after));
    const all = [
      { seq: 6, at: "t", kind: "waiting", engagement: "acme-1a2b", task: "T-3" },
      { seq: 7, at: "t", kind: "oob_hit", engagement: "acme-1a2b", task: "T-3", item: "I-2" },
    ];
    return r.fulfill({ json: { alerts: all.filter((a) => a.seq > after), latest: state.seq } });
  });
  await page.route(/\/api\/exchanges\?/, (r) => r.fulfill({ json: [] }));
  await page.goto("/#traffic");
  const toggle = page.getByRole("button", { name: "Desktop alerts" });
  await expect(toggle).toHaveText("alerts off");
  await toggle.click();
  await expect(toggle).toHaveText("alerts on");
  await expect(toggle).toHaveAttribute("aria-pressed", "true");

  // Two alerts arrive while the page is open (status polls every 5 s).
  state.seq = 7;
  await expect.poll(() => page.evaluate(() => (window as unknown as { __shown: unknown[] }).__shown), { timeout: 15_000 }).toEqual([
    { title: "spall", body: "T-3 is waiting on your approval" },
    { title: "spall", body: "Out-of-band callback I-2 (T-3)" },
  ]);
  expect(state.asked).toEqual(["5"]);
  await page.keyboard.press("`");
  await expect(page.locator(".console")).toContainText("alert: T-3 is waiting on your approval");
});
