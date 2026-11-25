// Out-of-band callbacks (#1) in the browser: the Engagement view's panel
// (hidden until a server is configured), a hit's raw request as text, a
// new payload, and Insert OOB payload in the replay editor. spall runs in
// log mode here, so /api/oob is mocked.
import type { Page } from "@playwright/test";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import { engagementApi } from "./engagement-api.ts";

const P = `<img src=x onerror="window.__pwned=1">`;

function oobApi(page: Page) {
  const state = { minted: 0 };
  void page.route("**/api/oob", async (r) => {
    if (r.request().method() === "POST") {
      state.minted++;
      const id = `c${"0".repeat(19)}n${String(state.minted).padStart(12, "0")}`;
      return r.fulfill({ status: 201, json: { id, host: `${id}.oast.lab`, url: `http://${id}.oast.lab/`, lab_url: `http://interactsh/${id}`, by: "human", at: "2026-11-25T10:00:00Z" } });
    }
    return r.fulfill({
      json: {
        configured: true,
        domain: "oast.lab",
        payloads: [{ id: "abc", host: "abc.oast.lab", url: "http://abc.oast.lab/", lab_url: "http://interactsh/abc", by: "agent:T-1", task: "T-1", at: "t" }],
        hits: [{ id: "I-1", payload: "abc", protocol: "http", remote: "203.0.113.9", at: "2026-11-25T10:01:00Z", raw_request: `GET /abc HTTP/1.1\nX-Evil: ${P}`, task: "T-1", exchange: "s-4" }],
      },
    });
  });
  return state;
}

test("the Out-of-band panel lists hits as text and mints payloads; hidden without a server (#1)", async ({ page }) => {
  engagementApi(page);
  const carrier = token();
  append(entry({ id: "s-4", host: "interactsh", url: `http://interactsh/${carrier}` }));
  await page.goto("/#engagement");
  await expect(page.getByRole("region", { name: "Checks" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Out-of-band" })).toHaveCount(0);

  const api = oobApi(page);
  await page.reload();
  const panel = page.getByRole("region", { name: "Out-of-band" });
  await expect(panel).toContainText("Callbacks to *.oast.lab");
  const hits = panel.getByRole("list", { name: "Out-of-band hits" });
  await expect(hits).toContainText("I-1 · http from 203.0.113.9");
  await expect(hits.getByRole("button", { name: "s-4" })).toBeVisible();
  await hits.locator("summary").click();
  await expect(hits.locator(".oob-raw")).toHaveText(`GET /abc HTTP/1.1\nX-Evil: ${P}`);
  expect(await panel.locator("img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  // Payloads are listed with their lab URL, and copy.
  const payloads = panel.getByRole("list", { name: "Out-of-band payloads" });
  await expect(payloads).toContainText("http://interactsh/abc");
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await panel.getByRole("button", { name: "new payload" }).click();
  expect(api.minted).toBe(1);
  await payloads.getByRole("button", { name: "Copy the lab URL of abc" }).click();
  await expect(payloads.getByRole("button", { name: "Copy the lab URL of abc" })).toHaveText("copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("http://interactsh/abc");
  // The hit's exchange opens in Traffic (the button isn't covered).
  await hits.getByRole("button", { name: "s-4" }).click({ timeout: 3000 });
  await expect(page).toHaveURL(/#traffic$/);
  await expect(page.locator(".inspector h2")).toContainText(`/${carrier}`);
});

test("Insert OOB payload puts a fresh callback host in the body being edited (#1)", async ({ page }) => {
  const t = token();
  const e = entry({ method: "POST", host: `${t}.example`, url: `https://${t}.example/api/fetch`, req_headers: [["host", `${t}.example`], ["content-type", "application/json"]], req_body: '{"url":""}' });
  append(e);
  oobApi(page);
  await page.route("**/api/status", (r) => r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.example"] } }));
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await page.locator(".inspector [data-action='edit']").click();
  const form = page.getByRole("form", { name: "Edit request" });
  const body = form.getByLabel("Body");
  await body.click();
  await body.evaluate((el: HTMLTextAreaElement) => el.setSelectionRange(8, 8));
  await form.getByRole("button", { name: "Insert OOB payload" }).click();
  await expect(body).toHaveValue(/^\{"url":"c0{19}n0{11}1\.oast\.lab"\}$/);
});

test("when the clipboard refuses, the payload is shown selected to copy by hand (#1)", async ({ page }) => {
  engagementApi(page);
  oobApi(page);
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.reject(new Error("denied")) } });
    document.execCommand = () => false;
  });
  await page.goto("/#engagement");
  const payloads = page.getByRole("list", { name: "Out-of-band payloads" });
  await payloads.getByRole("button", { name: "Copy the lab URL of abc" }).click();
  const field = payloads.getByRole("textbox", { name: /the clipboard refused/ });
  await expect(field).toHaveValue("http://interactsh/abc");
  expect(await field.evaluate((el: HTMLInputElement) => [el.selectionStart, el.selectionEnd])).toEqual([0, "http://interactsh/abc".length]);
});

test("the status bar shows the OOB sensor with its domain and hit count (#1)", async ({ page }) => {
  engagementApi(page);
  // A configured sensor: /api/status carries oob.
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.example"], oob: { domain: "oast.lab", hits: 3 } } }),
  );
  await page.goto("/#engagement");
  const chip = page.locator(".statusbar a.oob-status");
  await expect(chip).toContainText("OAST");
  await expect(chip).toContainText("oast.lab");
  await expect(chip).toContainText("3 hits");
  await expect(chip).toHaveAttribute("href", "#engagement");
});

test("the status bar has no OOB chip when no sensor is configured (#1)", async ({ page }) => {
  engagementApi(page); // its /api/status has no oob field
  await page.goto("/#engagement");
  await expect(page.locator(".statusbar")).toBeVisible();
  await expect(page.locator(".statusbar a.oob-status")).toHaveCount(0);
});

test("the sandbox chip shows its state; its popover says why it's unavailable (#13)", async ({ page }) => {
  engagementApi(page);
  const P = `<img src=x onerror="window.__pwned=1">`;
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.example"], sandbox: { state: "unavailable", reason: `gVisor's runsc isn't a Docker runtime here ${P}`, network: { state: "waiting", reason: "taphttp has no credential-only listener yet (taphttp#27)" } } } }),
  );
  await page.goto("/#engagement");
  const chip = page.getByRole("button", { name: "sandbox status" });
  await expect(chip).toContainText("unavailable");
  await chip.click();
  const pop = page.getByRole("dialog", { name: "sandbox details" });
  await expect(pop).toContainText(`runsc isn't a Docker runtime here ${P}`);
  await expect(pop).toContainText("spall sandbox-shell");
  await expect(pop).toContainText("none yet: taphttp has no credential-only listener yet (taphttp#27)");
  expect(await page.locator(".statusbar img").count()).toBe(0);
});

test("the sandbox chip is absent when no sandbox is configured, and counts running ones (#13)", async ({ page }) => {
  engagementApi(page); // no sandbox field: off
  await page.goto("/#engagement");
  await expect(page.locator(".statusbar")).toBeVisible();
  await expect(page.getByRole("button", { name: "sandbox status" })).toHaveCount(0);
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: [], sandbox: { state: "ready", live: 2 } } }),
  );
  await page.reload();
  await expect(page.getByRole("button", { name: "sandbox status" })).toContainText("ready · 2 running");
});
