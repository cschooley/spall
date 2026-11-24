// Approvals (#56): tasks waiting on a human, in Engagement's "Waiting on
// you" and the status bar. spall runs in log mode here, so /api/tasks is
// mocked with the server's answers; request data renders as text only.
import { expect, test } from "./fixtures.ts";
import { engagementApi } from "./engagement-api.ts";
import type { Page } from "@playwright/test";

const P = `<img src=x onerror="window.__pwned=1">`;

function waitingTask(id: string, ask: Record<string, unknown>, asked: string) {
  return {
    id,
    goal: `map the API ${P}`,
    role: "mapper",
    status: "waiting_on_approval",
    spent: 4,
    constraints: { budget: 20, time_limit_minutes: 60, methods: ["GET"] },
    pending: {
      method: "POST",
      target: `https://app.acme.test/login?next=${P}`,
      host_header: P,
      reason: `the RoE wants POST confirmed ${P}`,
      asked,
      ask,
      fingerprint: "ab",
      request: {},
    },
  };
}

function tasksApi(page: Page) {
  const state = {
    tasks: [waitingTask("T-2", { kind: "unsafe", method: "POST" }, "2026-11-23T20:02:00Z"), waitingTask("T-1", { kind: "confirm" }, "2026-11-23T20:01:00Z")],
    calls: [] as [string, unknown][],
  };
  engagementApi(page);
  // Over engagement-api's status: the count the status bar shows.
  void page.route("**/api/status", (r) =>
    r.fulfill({ json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.acme.test"], scope_all: false, engagement: { id: "acme-1a2b", name: "ACME Q4" }, testing: { allowed: true, reason: null }, to_triage: 0, waiting_approval: state.tasks.length } }),
  );
  void page.route(/\/api\/tasks$/, (r) => r.fulfill({ json: { tasks: state.tasks } }));
  void page.route(/\/api\/tasks\/[^/?]+$/, async (r) => {
    const id = decodeURIComponent(new URL(r.request().url()).pathname.split("/").pop()!);
    const body = r.request().postDataJSON() as { op: string; reason?: string };
    expect(r.request().headers()["content-type"]).toBe("application/json");
    state.calls.push([id, body]);
    const t = state.tasks.find((x) => x.id === id);
    if (!t) return r.fulfill({ status: 409, json: { error: `${id} isn't waiting on approval (it is running)` } });
    if (body.op === "approve_for_task" && t.pending.ask.kind === "confirm") return r.fulfill({ status: 400, json: { error: "only the rules of engagement can lift that: approve it once" } });
    state.tasks = state.tasks.filter((x) => x.id !== id);
    return r.fulfill({ json: { ...t, status: "running", pending: undefined } });
  });
  return state;
}

test("waiting tasks show exactly what would be sent, as text; approve once and approve for the task", async ({ page }) => {
  const api = tasksApi(page);
  await page.goto("/#engagement");
  const group = page.getByRole("group", { name: "Tasks waiting on approval" });
  await expect(group.locator("h4")).toHaveText("Tasks waiting on approval · 2");
  await expect(page.locator(".statusbar")).toContainText("2 waiting on approval");

  const cards = group.locator("article.approval");
  await expect(cards).toHaveCount(2);
  await expect(cards.nth(0)).toHaveAttribute("aria-label", "Approval for T-1");
  const t1 = cards.nth(0);
  await expect(t1).toContainText(`POST https://app.acme.test/login?next=${P}`);
  await expect(t1).toContainText(`map the API ${P}`);
  await expect(t1.locator("dd").nth(1)).toHaveText(P);
  await expect(t1).toContainText("1 request · 4 of 20 spent");
  await expect(t1.getByRole("button", { name: "Approve for the task" })).toBeDisabled();
  // Why, said on the card, not only in a tooltip.
  await expect(t1.locator(".once-only")).toHaveText(
    "Only once: the rules of engagement want this confirmed every time it's sent, and a task can't widen them.",
  );
  await expect(t1.getByRole("button", { name: "Approve for the task" })).toHaveAccessibleDescription(/Only once:/);
  expect(await page.locator(".approval img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

  await t1.getByRole("button", { name: "Approve once" }).click();
  await expect(cards).toHaveCount(1);
  const t2 = cards.nth(0);
  await expect(t2.locator(".once-only")).toHaveCount(0);
  await t2.getByRole("button", { name: "Approve for the task: let T-2 send POST without asking" }).click();
  await expect(group).toContainText("No task is waiting on you.");
  expect(api.calls).toEqual([
    ["T-1", { op: "approve_once" }],
    ["T-2", { op: "approve_for_task" }],
  ]);
});

test("deny takes a reason; a refusal from spall shows as text", async ({ page }) => {
  const api = tasksApi(page);
  await page.goto("/#engagement");
  const t1 = page.getByRole("article", { name: "Approval for T-1" });
  await t1.getByRole("button", { name: "Deny…" }).click();
  const why = t1.getByLabel("Why deny T-1's request");
  await expect(t1.getByRole("button", { name: "Deny", exact: true })).toBeDisabled();
  await why.fill("not on the login form");
  await t1.getByRole("button", { name: "Deny", exact: true }).click();
  await expect(page.getByRole("article", { name: "Approval for T-1" })).toHaveCount(0);
  expect(api.calls).toEqual([["T-1", { op: "deny", reason: "not on the login form" }]]);

  // Decided elsewhere meanwhile: spall says so, as text.
  api.tasks = api.tasks.map((t) => ({ ...t, id: "T-9" }));
  await page.reload();
  const gone = page.getByRole("article", { name: "Approval for T-9" }).first();
  await expect(gone).toBeVisible();
  api.tasks = [];
  await gone.getByRole("button", { name: "Approve once" }).click();
  await expect(gone.getByRole("alert")).toHaveText("T-9 isn't waiting on approval (it is running)");
});
