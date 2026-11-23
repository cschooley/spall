// The Tasks view (#61) and attribution (#62). spall runs in log mode here,
// so /api/tasks* and /api/candidates are mocked with the server's answers;
// agent-made exchanges come through the log with taphttp's `meta`, as a
// replay recorded by taphttp would (so attribution survives a restart).
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { Page } from "@playwright/test";

const P = `<img src=x onerror="window.__pwned=1">`;

type Json = Record<string, unknown>;

function task(id: string, status: string, over: Json = {}): Json {
  return {
    id,
    engagement: "acme-1a2b",
    goal: `Map the /v1 API ${P}`,
    role: "mapper",
    status,
    reason: null,
    spent: 3,
    asked_by: "agent (claude-code)",
    note: "",
    created: "2026-11-23T20:00:00Z",
    updated: "2026-11-23T20:00:00Z",
    started: status === "draft" ? null : "2026-11-23T20:00:05Z",
    ended: null,
    constraints: { hosts: [], methods: ["GET", "HEAD"], safe_methods: ["GET", "HEAD"], rate_limit: 2, destructive: false, budget: 20, time_limit_minutes: 60, stop: { server_errors: 3 } },
    plan: [
      { text: "baseline the API", done: true },
      { text: `replay GETs ${P}`, done: false },
    ],
    log: [] as Json[],
    ...over,
  };
}

function tasksApi(page: Page, tasks: Json[]) {
  const state = { tasks, calls: [] as [string, string, unknown][], created: 0 };
  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: ["*.acme.test"], scope_all: false, engagement: { id: "acme-1a2b", name: "ACME" }, testing: { allowed: true, reason: null }, to_triage: 0, waiting_approval: 0 },
    }),
  );
  void page.route(/\/api\/tasks$/, async (r) => {
    if (r.request().method() === "POST") {
      const body = r.request().postDataJSON() as Json;
      state.calls.push(["POST", "/api/tasks", body]);
      if ((body.constraints as Json)?.budget === 999999) return r.fulfill({ status: 400, json: { error: "budget: 1 to 100000 requests" } });
      const t = task(`T-${10 + ++state.created}`, "draft", { goal: body.goal, role: body.role });
      state.tasks.unshift(t);
      return r.fulfill({ status: 201, json: t });
    }
    return r.fulfill({ json: { tasks: state.tasks.map(({ log, ...t }) => ({ ...t, log_len: (log as Json[]).length })) } });
  });
  void page.route(/\/api\/tasks\/T-\d+$/, async (r) => {
    const id = new URL(r.request().url()).pathname.split("/").pop()!;
    const t = state.tasks.find((x) => x.id === id);
    if (!t) return r.fulfill({ status: 404, json: { error: "no such task" } });
    if (r.request().method() === "PATCH") {
      const op = r.request().postDataJSON() as Json;
      state.calls.push(["PATCH", id, op]);
      if (op.op === "approve") t.status = "approved";
      if (op.op === "halt") Object.assign(t, { status: "halted", reason: "halted by a human" });
      if (op.op === "note") (t.log as Json[]).push({ ts: "2026-11-23T20:05:00Z", kind: "note", by: "human", text: op.text });
      if (op.op === "approve_once") Object.assign(t, { status: "running", pending: undefined });
    }
    return r.fulfill({ json: t });
  });
  void page.route(/\/api\/candidates(\?.*)?$/, (r) =>
    r.fulfill({
      json: {
        candidates: [
          {
            id: "c-0000000a",
            created: "2026-11-23T20:03:00Z",
            updated: "2026-11-23T20:03:00Z",
            flagged_by: "agent:T-3",
            finding: { ruleId: "WSTG-INFO-05", level: "note", message: { text: `paths in comments ${P}` }, locations: [], properties: { "flux/status": "candidate" }, suppressions: [] },
          },
        ],
      },
    }),
  );
  return state;
}

test("4 opens Tasks: the list, the task card, its request log and the agent's proposals, all as text", async ({ page }) => {
  const t = token();
  const sent = entry({ host: `${t}.test`, url: `https://${t}.test/v1/items`, replay_of: "orig-1", meta: { actor: "agent", task: "T-3" } as Record<string, string>, schema_version: 5 });
  append(sent);
  const running = task("T-3", "running", {
    log: [
      { ts: "2026-11-23T20:00:05Z", kind: "status", from: "approved", to: "running" },
      { ts: "2026-11-23T20:00:07Z", kind: "request", exchange: sent.id, method: "GET", target: `https://${t}.test/v1/items?q=${P}`, outcome: "sent", status: 200 },
      { ts: "2026-11-23T20:00:08Z", kind: "request", exchange: null, method: "DELETE", target: `https://${t}.test/v1/items/1`, outcome: "denied" },
      { ts: "2026-11-23T20:00:09Z", kind: "turn", text: `Baseline done ${P}` },
    ],
  });
  const api = tasksApi(page, [running, task("T-2", "done")]);
  await page.goto("/#traffic");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("4");
  await expect(page).toHaveURL(/#tasks$/);
  const list = page.getByRole("region", { name: "Tasks" });
  await expect(list.locator("a.task")).toHaveCount(2);
  await expect(list.locator("a.task").first()).toContainText(`T-3Running`);
  await expect(list.locator("a.task").first()).toContainText(`Map the /v1 API ${P}`);

  await list.locator("a.task").first().click();
  await expect(page).toHaveURL(/#tasks\/T-3$/);
  const card = page.getByRole("region", { name: "Task detail" });
  await expect(card.locator("h2")).toHaveText(`Map the /v1 API ${P}`);
  await expect(card).toContainText("GET, HEAD");
  await expect(card).toContainText("≤ 2 req/s");
  await expect(card).toContainText("3 × 5xx");
  await expect(card.locator(".plan li")).toHaveText(["✓baseline the API", `○replay GETs ${P}`]);
  await expect(card.getByRole("link", { name: "c-0000000a · WSTG-INFO-05" })).toHaveAttribute("href", "#findings/c-0000000a");
  const log = card.getByRole("table", { name: "T-3 request log" });
  await expect(log.locator("tbody tr")).toHaveCount(2);
  await expect(log.locator("tbody tr").nth(1)).toContainText("denied");
  const steer = page.getByRole("complementary", { name: "Steer T-3" });
  await expect(steer).toContainText(`◆ agent · 20:00:09Baseline done ${P}`);
  expect(await page.locator(".tasks img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

  // Steering is recorded in the task's log.
  await steer.getByRole("textbox", { name: "Steer T-3" }).fill("skip password reset");
  await steer.getByRole("button", { name: "Send" }).click();
  await expect(steer).toContainText("you · 20:05:00skip password reset");
  expect(api.calls).toContainEqual(["PATCH", "T-3", { op: "note", text: "skip password reset" }]);

  // Halt takes two presses and is for good.
  const halt = card.getByRole("button", { name: "Halt" });
  await halt.click();
  await expect(card.locator(".btn.halt")).toHaveText("press again: halt for good");
  expect(api.calls.filter(([, , b]) => (b as Json).op === "halt")).toHaveLength(0);
  await card.locator(".btn.halt").click();
  await expect(card.locator(".chip").first()).toHaveText("Halted");
  await expect(card.locator(".btn.halt")).toHaveCount(0);
  await expect(page.getByRole("complementary", { name: "Steer T-3" })).toContainText("T-3 is halted: nothing more runs under it.");

  // The log opens the exchange in Traffic, where the agent's row is marked.
  await page.goto("/#tasks/T-3");
  await page.getByRole("table", { name: "T-3 request log" }).getByRole("button").first().click();
  await expect(page).toHaveURL(/#traffic$/);
  await expect(page.locator(".inspector .by.agent")).toHaveText("by ◆ agent · T-3");
});

test("a new task is a draft you approve; a waiting one shows its approval inline", async ({ page }) => {
  const waiting = task("T-4", "waiting_on_approval", {
    pending: { method: "PUT", target: `https://app.acme.test/v1/cart/${P}`, host_header: null, reason: "T-4: PUT isn't allowed by the task's constraints", asked: "2026-11-23T20:02:00Z", ask: { kind: "method", method: "PUT" }, fingerprint: "ab", request: {} },
  });
  const api = tasksApi(page, [waiting]);
  await page.goto("/#tasks/T-4");
  const card = page.getByRole("region", { name: "Task detail" });
  const approval = card.getByRole("article", { name: "Approval for T-4" });
  await expect(approval).toContainText(`PUT https://app.acme.test/v1/cart/${P}`);
  await approval.getByRole("button", { name: "Approve once" }).click();
  await expect(card.locator(".chip").first()).toHaveText("Running");
  expect(api.calls).toContainEqual(["PATCH", "T-4", { op: "approve_once" }]);

  await page.getByRole("button", { name: "+ New task" }).click();
  const form = page.getByRole("region", { name: "New task" });
  await form.getByLabel("Goal").fill("Replay the cart GETs");
  await form.getByLabel("Budget (requests)").fill("999999");
  await form.getByRole("button", { name: "Create draft" }).click();
  await expect(form.getByRole("alert")).toHaveText("budget: 1 to 100000 requests");
  await form.getByLabel("Budget (requests)").fill("40");
  await form.getByLabel("Role").selectOption("tester");
  await form.getByLabel(/^Methods/).fill("get, head");
  await form.getByRole("button", { name: "Create draft" }).click();
  await expect(page).toHaveURL(/#tasks\/T-11$/);
  expect(api.calls.filter(([m]) => m === "POST").at(-1)).toEqual([
    "POST",
    "/api/tasks",
    { goal: "Replay the cart GETs", role: "tester", constraints: { budget: 40, time_limit_minutes: 60, methods: ["GET", "HEAD"] }, note: "" },
  ]);
  await card.getByRole("button", { name: "Approve" }).click();
  await expect(card.locator(".chip").first()).toHaveText("Approved");
});

test("Traffic marks what an agent made, with ◆ and a label; actor: and task: filter it (#62)", async ({ page }) => {
  tasksApi(page, []);
  const t = token();
  const mine = entry({ host: `${t}.test`, url: `https://${t}.test/mine` });
  const agent = entry({ host: `${t}.test`, url: `https://${t}.test/agent`, replay_of: mine.id, schema_version: 5, meta: { actor: "agent", task: "T-7" } as Record<string, string> });
  const other = entry({ host: `${t}.test`, url: `https://${t}.test/other`, schema_version: 5, meta: { actor: "agent", task: "T-8" } as Record<string, string> });
  append(mine, agent, other);
  await page.goto("/#traffic");
  await filterTo(page, t);
  await expect(page.locator("tr.row")).toHaveCount(3);
  const mark = page.locator("tr.row .by-agent");
  await expect(mark).toHaveText(["◆ T-7", "◆ T-8"]);
  await expect(mark.first()).toHaveAttribute("aria-label", "made by agent · T-7");

  await filterTo(page, `${t} actor:agent`);
  await expect(page.locator("tr.row")).toHaveCount(2);
  await filterTo(page, `${t} task:T-7`);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await filterTo(page, `${t} actor:you`);
  await expect(page.locator("tr.row")).toHaveCount(1);
  await page.locator("tr.row").first().click();
  await expect(page.locator(".inspector .by")).toHaveText("by you");

  // After a reload, attribution is still there: it comes from the record.
  await page.reload();
  await filterTo(page, `${t} task:T-7`);
  await page.locator("tr.row").first().click();
  const by = page.locator(".inspector .by.agent a");
  await expect(by).toHaveText("◆ agent · T-7");
  await expect(by).toHaveAttribute("href", "#tasks/T-7");
});

test("Findings shows an agent's proposal as ◆ agent · T-n, linked to its task (#60, #62)", async ({ page }) => {
  const { candidatesApi, findingFor } = await import("./candidates-api.ts");
  const t = token();
  const e = entry({ host: `${t}.test`, url: `https://${t}.test/login` });
  append(e);
  const mk = (id: string, by: string) => ({ id, created: e.ts, updated: e.ts, flagged_by: by, finding: findingFor("WSTG-INPV-05", e, id) });
  candidatesApi(page, { entries: [e], seed: [mk("c-00000001", "agent:T-3"), mk("c-00000002", "human")] });
  await page.goto("/#findings/c-00000001");
  const cand = page.getByRole("complementary", { name: "Candidate" });
  const by = cand.locator(".by.agent a");
  await expect(by).toHaveText("◆ agent · T-3");
  await expect(by).toHaveAttribute("href", "#tasks/T-3");
  await expect(cand.locator(".by")).toContainText("proposed by");
  const queue = page.getByRole("list", { name: "Candidates" });
  await expect(queue.locator(".by-agent")).toHaveText(["◆ T-3"]);
  await page.goto("/#findings/c-00000002");
  await expect(page.getByRole("complementary", { name: "Candidate" }).locator(".by")).toContainText("flagged by you");
});
