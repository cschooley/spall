// Replay from the inspector, ⏎ and the console (#18). These runs tail a log
// file, where spall can't replay; /api/status and /api/replay are mocked so
// the UI's side is tested on its own. The server side (gate, tokens,
// audit) has its own tests in tests/lever.rs.
import type { Page, Route } from "@playwright/test";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { TrafficEntry } from "../src/types.ts";

/** Pretend spall is in lever mode with replay available. */
async function replayAvailable(page: Page, scope = ["example.test"]) {
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope } }),
  );
}

type Answer = { status: number; json: unknown };

/** Mock /api/replay; `answer` sees each request body. Returns the bodies. */
async function mockReplay(page: Page, answer: (body: { id: string; confirm?: string }) => Answer) {
  const bodies: { id: string; confirm?: string }[] = [];
  await page.route("**/api/replay", async (r: Route) => {
    const body = r.request().postDataJSON() as { id: string; confirm?: string };
    bodies.push(body);
    expect(r.request().headers()["content-type"]).toBe("application/json");
    const a = answer(body);
    await r.fulfill({ status: a.status, json: a.json });
  });
  return bodies;
}

function replayOf(original: TrafficEntry, over: Partial<TrafficEntry> = {}): TrafficEntry {
  return entry({ ...original, id: `${original.id}-r`, replay_of: original.id, duration_ms: 9, ...over });
}

async function openOnly(page: Page, e: TrafficEntry, t: string) {
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").filter({ hasText: e.url.split("/").pop()! }).first().click();
  await expect(page.locator(".inspector")).toBeVisible();
}

const button = (page: Page) => page.locator(".inspector [data-action='replay']");

test("Replay sends the exchange's id, opens the result and links back to the original", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/orig` });
  append(orig);
  await replayAvailable(page);
  const bodies = await mockReplay(page, () => ({ status: 200, json: replayOf(orig) }));
  await openOnly(page, orig, t);

  await expect(button(page)).toHaveText("Replay⏎");
  await button(page).click();
  const insp = page.locator(".inspector");
  await expect(insp.locator(".meta")).toContainText(`replay of ${orig.id}`);
  expect(bodies).toEqual([{ id: orig.id }]);

  // The link opens the original again.
  await insp.locator(".meta button.orig").click();
  await expect(insp.locator(".meta button.orig")).toHaveCount(0);
  await expect(insp.locator("h2")).toContainText("/orig");

  // ⏎ does the same, from anywhere outside a field or button.
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.locator("tr.row").filter({ hasText: "/orig" }).first().click();
  await page.locator(".inspector .pane").click();
  await page.keyboard.press("Enter");
  await expect(insp.locator(".meta")).toContainText(`replay of ${orig.id}`);
  expect(bodies).toEqual([{ id: orig.id }, { id: orig.id }]);
});

test("an unsafe method takes two presses, and the second sends the server's token", async ({ page }) => {
  const t = token();
  const orig = entry({ method: "DELETE", host: `${t}.example`, url: `http://${t}.example/item/7` });
  append(orig);
  await replayAvailable(page);
  const bodies = await mockReplay(page, (b) =>
    b.confirm === "tok-1"
      ? { status: 200, json: replayOf(orig) }
      : {
          status: 428,
          json: { decision: "needs_confirmation", error: "DELETE can change state on the target; confirm to send", confirm: "tok-1", expires_in_secs: 120 },
        },
  );
  await openOnly(page, orig, t);

  await page.locator(".inspector .pane").click();
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveText(`press again to send DELETE to ${t}.example⏎`);
  await expect(button(page)).toHaveClass(/armed/);
  expect(bodies, "first press: nothing confirmed, nothing sent").toEqual([{ id: orig.id }]);

  await page.keyboard.press("Enter");
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${orig.id}`);
  expect(bodies).toEqual([{ id: orig.id }, { id: orig.id, confirm: "tok-1" }]);
});

test("the arming lapses: another key, another click, or a few seconds", async ({ page }) => {
  const t = token();
  const orig = entry({ method: "POST", host: `${t}.example`, url: `http://${t}.example/order` });
  append(orig);
  await page.clock.install();
  await replayAvailable(page);
  let n = 0;
  const bodies = await mockReplay(page, () => ({
    status: 428,
    json: { decision: "needs_confirmation", error: "POST can change state", confirm: `tok-${++n}` },
  }));
  await openOnly(page, orig, t);
  const armed = `press again to send POST to ${t}.example⏎`;

  // Another key.
  await page.locator(".inspector .pane").click();
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveText(armed);
  await page.keyboard.press("x");
  await expect(button(page)).toHaveText("Replay⏎");

  // Another click.
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveText(armed);
  await page.locator(".inspector .pane").click();
  await expect(button(page)).toHaveText("Replay⏎");

  // Time.
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveText(armed);
  await page.clock.fastForward(5_000);
  await expect(button(page)).toHaveText("Replay⏎");

  // Lapsed, so the next press starts over: no token sent.
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveText(armed);
  expect(bodies.every((b) => b.confirm === undefined), JSON.stringify(bodies)).toBe(true);
});

test("a denial is shown as text in the inspector and lands in the console", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/cdn` });
  append(orig);
  await replayAvailable(page);
  const hostile = `"<img src=x onerror=\\"window.__pwned=1\\">" is not in scope`;
  await mockReplay(page, () => ({ status: 403, json: { decision: "deny", error: hostile } }));
  await openOnly(page, orig, t);
  await button(page).click();
  await expect(page.locator(".inspector .replay-msg")).toHaveText(hostile);
  expect(await page.locator(".inspector img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await expect(page.locator(".con-log")).toContainText(`replay ${orig.id.slice(0, 8)}`);
  await expect(page.locator(".con-log")).toContainText("is not in scope");
});

test("a failed replay opens the recorded attempt with its error", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/down` });
  append(orig);
  await replayAvailable(page);
  const failed = replayOf(orig, { status: null, duration_ms: null, res_headers: [], error: "connect error: Connection refused" });
  await mockReplay(page, () => ({ status: 502, json: failed }));
  await openOnly(page, orig, t);
  await button(page).click();
  const meta = page.locator(".inspector .meta");
  await expect(meta).toContainText(`replay of ${orig.id}`);
  await expect(meta.locator(".error")).toHaveText("connect error: Connection refused");
});

test("the console's replay matches the button, and --yes confirms in one step", async ({ page }) => {
  const t = token();
  const orig = entry({ method: "PUT", host: `${t}.example`, url: `http://${t}.example/profile` });
  append(orig);
  await replayAvailable(page, ["*.example"]);
  const bodies = await mockReplay(page, (b) =>
    b.confirm === "tok-y"
      ? { status: 200, json: replayOf(orig) }
      : { status: 428, json: { decision: "needs_confirmation", error: "PUT can change state", confirm: "tok-y" } },
  );
  await page.goto("/");
  await filterTo(page, t);
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const input = page.getByLabel("Console command");

  await input.fill(`replay ${orig.id} --yes`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("recorded as");
  expect(bodies).toEqual([{ id: orig.id }, { id: orig.id, confirm: "tok-y" }]);
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${orig.id}`);

  await input.fill("scope");
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("*.example");
});

test("without the lever API, Replay is offered but disabled, and says why", async ({ page }) => {
  const t = token();
  const orig = entry({ host: `${t}.example`, url: `http://${t}.example/plain` });
  append(orig);
  await openOnly(page, orig, t);
  await expect(button(page)).toBeDisabled();
  await expect(button(page)).toHaveAttribute("title", "Replay needs taphttp's lever API");
  await page.locator(".inspector .pane").click();
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-unread")).toBeVisible();
  await page.keyboard.press("`");
  await expect(page.locator(".con-log")).toContainText("replay needs taphttp's lever API");
});

test("Enter on the focused Replay button is one press, not two", async ({ page }) => {
  const t = token();
  const orig = entry({ method: "PATCH", host: `${t}.example`, url: `http://${t}.example/acct` });
  append(orig);
  await replayAvailable(page);
  const bodies = await mockReplay(page, (b) =>
    b.confirm === "tok-f"
      ? { status: 200, json: replayOf(orig) }
      : { status: 428, json: { decision: "needs_confirmation", error: "PATCH can change state", confirm: "tok-f" } },
  );
  await openOnly(page, orig, t);
  await button(page).focus();
  await page.keyboard.press("Enter");
  await expect(button(page)).toHaveClass(/armed/);
  expect(bodies, "one press: one request").toEqual([{ id: orig.id }]);
  await page.keyboard.press("Enter");
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${orig.id}`);
  expect(bodies).toEqual([{ id: orig.id }, { id: orig.id, confirm: "tok-f" }]);
});

test("scope add and scope all on|off from the console; the status bar warns while all hosts are in", async ({ page }) => {
  const state = { scope: ["example.test"], all: false };
  await page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: [...state.scope, ...(state.all ? ["*"] : [])], scope_all: state.all },
    }),
  );
  const changes: unknown[] = [];
  await page.route("**/api/scope", async (r) => {
    const c = r.request().postDataJSON() as { op: string; entry?: string; on?: boolean };
    expect(r.request().headers()["content-type"]).toBe("application/json");
    changes.push(c);
    if (c.op === "add") state.scope.push(c.entry!);
    if (c.op === "all") state.all = c.on!;
    await r.fulfill({ json: { scope: [...state.scope, ...(state.all ? ["*"] : [])], all: state.all, changed: true } });
  });
  await page.goto("/");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const input = page.getByLabel("Console command");
  const log = page.locator(".con-log");

  await input.fill("scope add *.whatever.com");
  await page.keyboard.press("Enter");
  await expect(log).toContainText("scope: example.test, *.whatever.com");
  await expect(page.locator(".statusbar .scope-all")).toHaveCount(0);

  await input.fill("scope all on");
  await page.keyboard.press("Enter");
  await expect(log).toContainText("every host, third parties included");
  await expect(page.locator(".statusbar .scope-all")).toHaveText("● scope: all hosts");

  await input.fill("scope all off");
  await page.keyboard.press("Enter");
  await expect(page.locator(".statusbar .scope-all")).toHaveCount(0);
  expect(changes).toEqual([
    { op: "add", entry: "*.whatever.com" },
    { op: "all", on: true },
    { op: "all", on: false },
  ]);
});

test("scope exclude and scope check from the console (#34); hostile hosts stay text", async ({ page }) => {
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.a.test"], scope_all: false } }),
  );
  const changes: unknown[] = [];
  await page.route("**/api/scope", async (r) => {
    changes.push(r.request().postDataJSON());
    await r.fulfill({ json: { scope: ["*.a.test", "!sso.a.test"], all: false, changed: true } });
  });
  const p = `<img src=x onerror="window.__pwned=1">`;
  const checked: string[] = [];
  await page.route("**/api/scope/check?*", async (r) => {
    const target = new URL(r.request().url()).searchParams.get("target")!;
    checked.push(target);
    if (target.includes("sso")) await r.fulfill({ json: { host: "sso.a.test", state: "out", by: "!sso.a.test" } });
    else await r.fulfill({ json: { host: p, state: "unknown", by: null } });
  });
  await page.goto("/");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const input = page.getByLabel("Console command");
  const log = page.locator(".con-log");

  await input.fill("scope exclude sso.a.test");
  await page.keyboard.press("Enter");
  await expect(log).toContainText("excluded: sso.a.test (wins over the scope)");
  expect(changes).toEqual([{ op: "exclude", entry: "sso.a.test" }]);

  await input.fill("scope check https://sso.a.test/login?x=1&y=2");
  await page.keyboard.press("Enter");
  await expect(log).toContainText("sso.a.test: out of scope (excluded by !sso.a.test)");
  expect(checked).toEqual(["https://sso.a.test/login?x=1&y=2"]);

  await input.fill("scope check hostile");
  await page.keyboard.press("Enter");
  await expect(log).toContainText(`${p}: not in scope`);
  expect(await page.locator(".con-log img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("engagement commands from the console (#35); the status bar names the engagement, as text", async ({ page }) => {
  const p = `<img src=x onerror="window.__pwned=1">`;
  let name = `ACME ${p}`;
  await page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["acme.test"], scope_all: false, engagement: { id: "acme-1a2b", name } },
    }),
  );
  const eng = () => ({ id: "acme-1a2b", name, client: "", starts: null, ends: null, expires: null, scope: ["acme.test"], notes: "" });
  const sent: unknown[] = [];
  await page.route("**/api/engagement", async (r) => {
    if (r.request().method() === "PATCH") {
      expect(r.request().headers()["content-type"]).toBe("application/json");
      const body = r.request().postDataJSON() as { name?: string };
      sent.push(body);
      if (body.name) name = body.name;
      return r.fulfill({ json: { engagement: eng(), changed: true } });
    }
    return r.fulfill({ json: eng() });
  });
  await page.goto("/");
  await expect(page.locator(".statusbar .engagement-name")).toHaveText(name);
  expect(await page.locator(".statusbar img").count()).toBe(0);

  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const input = page.getByLabel("Console command");
  await input.fill("engagement");
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText(`engagement: ${name} (acme-1a2b)`);
  await input.fill("engagement set name ACME Q4");
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("engagement: name is now ACME Q4");
  expect(sent).toEqual([{ name: "ACME Q4" }]);
  await expect(page.locator(".statusbar .engagement-name")).toHaveText("ACME Q4");
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
