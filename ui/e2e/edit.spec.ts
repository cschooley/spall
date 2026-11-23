// Edit & replay (#19). /api/status and /api/replay are mocked as in
// replay.spec.ts; what's tested is the editor and the body it sends.
import type { Page, Route } from "@playwright/test";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { TrafficEntry } from "../src/types.ts";

type Sent = Record<string, unknown>;

async function setup(page: Page, e: TrafficEntry, t: string, answer?: (b: Sent) => { status: number; json: unknown }) {
  await page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.example"] } }),
  );
  const bodies: Sent[] = [];
  await page.route("**/api/replay", async (r: Route) => {
    const b = r.request().postDataJSON() as Sent;
    bodies.push(b);
    const a = answer?.(b) ?? { status: 200, json: entry({ ...e, id: `${e.id}-r${bodies.length}`, replay_of: e.id }) };
    await r.fulfill({ status: a.status, json: a.json });
  });
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await expect(page.locator(".inspector")).toBeVisible();
  return bodies;
}

const editor = (page: Page) => page.getByRole("form", { name: "Edit request" });
const send = (page: Page) => editor(page).locator("[data-action='replay']");

function post(t: string, over: Partial<TrafficEntry> = {}) {
  return entry({
    method: "GET",
    host: `${t}.example`,
    url: `https://${t}.example/api/cart`,
    req_headers: [
      ["host", `${t}.example`],
      ["content-type", "application/json"],
      ["content-length", "9"],
      ["referer", "https://x.example/"],
    ],
    req_body: '{"qty":1}',
    ...over,
  });
}

test("edit headers and body, see what changed, and send only that", async ({ page }) => {
  const t = token();
  const e = post(t);
  append(e);
  const bodies = await setup(page, e, t);
  await page.locator(".pane").click();
  await page.keyboard.press("e");
  await expect(editor(page)).toBeVisible();
  await expect(editor(page)).toContainText("no changes: replays as captured");
  await expect(editor(page)).toContainText("the URL is fixed");
  await expect(page.locator(".inspector > .actions"), "the editor has its own Send").toHaveCount(0);

  await page.getByLabel("Header 2 value").fill("application/json; charset=utf-8");
  await page.getByLabel("Remove header 4").click();
  await editor(page).getByRole("button", { name: "+ Add header" }).click();
  await page.getByLabel("Header 4 name").fill("X-Role");
  await page.getByLabel("Header 4 value").fill("admin");
  await page.getByLabel("Body").fill('{"qty":1000}');

  await expect(editor(page)).toContainText("4 changes"); // content-type, X-Role, referer gone, body
  await expect(editor(page)).toContainText("1 captured header removed");
  await expect(editor(page).locator(".hrow.changed")).toHaveCount(2); // Content-Length is synced on send, and says so
  await expect(editor(page)).toContainText("Content-Length will be sent as 12.");
  await expect(page.getByLabel("Body")).toHaveClass(/changed/);

  await page.getByLabel("Body").press("Control+Enter");
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${e.id}`);
  expect(bodies).toEqual([
    {
      id: e.id,
      body: '{"qty":1000}',
      body_encoding: "utf8",
      req_headers: [
        ["host", `${t}.example`],
        ["content-type", "application/json; charset=utf-8"],
        ["content-length", "12"],
        ["X-Role", "admin"],
      ],
    },
  ]);
  await expect(editor(page), "the result opens, out of edit mode").toHaveCount(0);
});

test("Esc and Cancel leave without sending; reordering is sent as ordered", async ({ page }) => {
  const t = token();
  const e = post(t);
  append(e);
  const bodies = await setup(page, e, t);
  await page.locator(".inspector [data-action='edit']").click();
  await page.getByLabel("Method").fill("HEAD");
  await page.getByLabel("Method").press("Escape");
  await expect(editor(page)).toHaveCount(0);
  await expect(page.locator(".inspector"), "Esc in the editor cancels, it doesn't close").toBeVisible();

  await page.locator(".inspector [data-action='edit']").click();
  await expect(page.getByLabel("Method"), "a fresh draft").toHaveValue("GET");
  await editor(page).getByRole("button", { name: "Cancel" }).click();
  await expect(editor(page)).toHaveCount(0);
  expect(bodies).toEqual([]);

  await page.locator(".inspector [data-action='edit']").click();
  await page.getByLabel("Move header 4 up").click();
  await send(page).click();
  await expect(page.locator(".inspector .meta")).toContainText("replay of");
  expect(bodies[0]!.req_headers).toEqual([
    ["host", `${t}.example`],
    ["content-type", "application/json"],
    ["referer", "https://x.example/"],
    ["content-length", "9"],
  ]);
  expect(bodies[0]!.body, "body untouched: not sent").toBeUndefined();
});

test("the editor states its limits and refuses what can't be sent", async ({ page }) => {
  const t = token();
  const e = post(t, { req_body: "iVBORw0KGgo=", req_body_encoding: "base64" });
  append(e);
  const bodies = await setup(page, e, t);
  await page.locator(".inspector [data-action='edit']").click();
  await expect(editor(page)).toContainText("Binary body (8 B): sent as captured, not editable here.");
  await expect(page.getByLabel("Body")).toHaveCount(0);

  // A Host that isn't the URL's host is called out before sending.
  await page.getByLabel("Header 1 value").fill("admin.internal");
  await expect(editor(page).locator(".caution")).toContainText(`Host is admin.internal, not the URL's host ${t}.example`);

  // Repeats split by another header are noted.
  await editor(page).getByRole("button", { name: "+ Add header" }).click();
  await page.getByLabel("Header 5 name").fill("Content-Type");
  await page.getByLabel("Header 5 value").fill("text/plain");
  await expect(editor(page)).toContainText("sent together, at the first one's position");

  // An invalid header name blocks sending, with the reason.
  await page.getByLabel("Header 5 name").fill("bad name");
  await expect(editor(page).getByRole("alert")).toContainText('invalid header name "bad name"');
  await expect(send(page)).toBeDisabled();
  // Inputs are single-line: a pasted CR/LF can't make it into a value.
  await page.getByLabel("Header 5 name").fill("X-A");
  await page.getByLabel("Header 5 value").fill("a\r\nInjected: 1");
  expect(await page.getByLabel("Header 5 value").inputValue()).not.toMatch(/[\r\n]/);
  await send(page).click();
  await expect(page.locator(".inspector .meta")).toContainText("replay of");
  expect(bodies).toHaveLength(1);
  expect(bodies[0]!.body).toBeUndefined();
  expect(JSON.stringify(bodies[0]!.req_headers)).not.toMatch(/\\r|\\n/);
});

test("an edited unsafe method takes two presses; any edit disarms", async ({ page }) => {
  const t = token();
  const e = post(t);
  append(e);
  let n = 0;
  const bodies = await setup(page, e, t, (b) =>
    b.confirm === `tok-${n}`
      ? { status: 200, json: entry({ ...e, id: `${e.id}-r`, method: "DELETE", replay_of: e.id }) }
      : { status: 428, json: { decision: "needs_confirmation", error: "DELETE can change state", confirm: `tok-${++n}` } },
  );
  await page.locator(".inspector [data-action='edit']").click();
  await page.getByLabel("Method").fill("delete");
  await expect(page.getByLabel("Method"), "upper-cased as typed").toHaveValue("DELETE");
  await send(page).click();
  await expect(send(page)).toHaveText(/press again to send DELETE to/);

  await page.getByLabel("Header 4 value").fill("https://y.example/");
  await expect(send(page), "the token was for the old request").toHaveText(/^Send/);
  await send(page).click();
  await expect(send(page)).toHaveText(/press again/);
  await send(page).click();
  await expect(page.locator(".inspector .meta")).toContainText(`replay of ${e.id}`);
  expect(bodies.map((b) => [b.method, b.confirm ?? null])).toEqual([
    ["DELETE", null],
    ["DELETE", null],
    ["DELETE", "tok-2"],
  ]);
});

test("console flags send what the editor would", async ({ page }) => {
  const t = token();
  const e = post(t);
  append(e);
  const bodies = await setup(page, e, t);
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  await page.getByLabel("Console command").fill(
    `replay ${e.id} --header 'Content-Type: text/plain' --drop-header referer --header 'X-Role: admin' --body 'qty=2'`,
  );
  await page.keyboard.press("Enter");
  await expect(page.locator(".con-log")).toContainText("Content-Length set to 5");
  await expect(page.locator(".con-log")).toContainText("recorded as");
  expect(bodies).toEqual([
    {
      id: e.id,
      body: "qty=2",
      body_encoding: "utf8",
      req_headers: [
        ["host", `${t}.example`],
        ["Content-Type", "text/plain"],
        ["content-length", "5"],
        ["X-Role", "admin"],
      ],
    },
  ]);
});

test("hostile captured values stay text inside the editor", async ({ page }) => {
  const t = token();
  const payload = `<img src=x onerror="window.__pwned=1">`;
  const e = post(t, { req_headers: [["host", `${t}.example`], [`x-${payload}`, payload]], req_body: `</textarea>${payload}` });
  append(e);
  await setup(page, e, t);
  await page.locator(".inspector [data-action='edit']").click();
  await expect(page.getByLabel("Header 2 value")).toHaveValue(payload);
  await expect(page.getByLabel("Body")).toHaveValue(`</textarea>${payload}`);
  expect(await page.locator(".inspector img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
