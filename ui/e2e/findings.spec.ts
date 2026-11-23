// The Findings view (#44) and the evidence pane (#45), against the
// in-memory candidates API in candidates-api.ts.
import { append, entry, expect, test, token } from "./fixtures.ts";
import { candidatesApi, findingFor, type Stored } from "./candidates-api.ts";
import type { Page } from "@playwright/test";

const P = `<img src=x onerror="window.__pwned=1">`;

function seed(t: string) {
  const a = entry({ host: `${t}.test`, url: `https://${t}.test/p/1?sort=x`, status: 500 });
  const b = entry({ host: `${t}.test`, url: `https://${t}.test/search?q=1`, ts: new Date(Date.parse(a.ts) + 1000).toISOString() });
  const c = entry({ host: `${t}.test`, url: `https://${t}.test/o/7`, ts: new Date(Date.parse(a.ts) + 2000).toISOString() });
  const r = entry({ ...a, id: `${a.id}-r`, ts: new Date(Date.parse(a.ts) + 3000).toISOString(), replay_of: a.id, status: 200, res_body: "ok" });
  const mk = (id: string, rule: string, e: typeof a, param?: string, extra = {}): Stored => ({ id, created: a.ts, updated: a.ts, flagged_by: "human", finding: findingFor(rule, e, param, extra) });
  const list = [
    mk("c-00000001", "WSTG-INPV-05", a, "sort", { message: { text: `SQL error in sort ${P}` } }),
    mk("c-00000002", "WSTG-INPV-01", b, "q"),
    mk("c-00000003", "WSTG-ATHZ-04", c),
  ];
  list[0]!.finding.properties["flux/replayIds"] = [a.id, r.id];
  return { entries: [a, b, c, r], list };
}

const queueIds = (page: Page) => page.getByRole("list", { name: "Candidates" }).locator("li .rule");

test("5 opens Findings: a keyboard triage queue that clears", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/");
  await expect(page.locator(".statusbar .to-triage")).toHaveText("3 to triage");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("5");
  await expect(page).toHaveURL(/#findings$/);
  await expect(page.locator(".rail [data-view='findings']")).toHaveAttribute("aria-current", "page");
  // Newest first; the top one is selected.
  await expect(queueIds(page)).toHaveText(["WSTG-ATHZ-04", "WSTG-INPV-01", "WSTG-INPV-05"]);
  const cand = page.getByRole("complementary", { name: "Candidate" });
  await expect(cand.locator("h2")).toHaveText("c-00000003 · WSTG-ATHZ-04");

  // c confirms; it leaves the queue and the next one is selected.
  await page.keyboard.press("c");
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-01", "WSTG-INPV-05"]);
  await expect(cand.locator("h2")).toHaveText("c-00000002 · WSTG-INPV-01");
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-00000003", body: { op: "confirm" } });

  // r asks why; nothing without a reason; Enter sends it.
  await page.keyboard.press("r");
  const ask = page.getByRole("form", { name: "Reject (not a real issue)" });
  await expect(ask.getByLabel("Justification")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(ask.getByRole("alert")).toHaveText("a justification is required");
  await ask.getByLabel("Justification").fill(`output is encoded ${P}`);
  await page.keyboard.press("Enter");
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-05"]);
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-00000002", body: { op: "reject", justification: `output is encoded ${P}` } });

  // d: a duplicate of the confirmed one.
  await page.keyboard.press("d");
  const dup = page.getByRole("form", { name: "Duplicate of another candidate" });
  await expect(dup.getByLabel("Duplicate of")).toHaveValue("c-00000003");
  await dup.getByLabel("Justification").fill("same handler");
  await page.keyboard.press("Enter");
  await expect(page.locator(".findings .empty")).toHaveText("Nothing waiting for you: the queue is clear.");
  await expect(page.locator(".statusbar .to-triage")).toHaveCount(0);

  // Decided: the reasons, as text; o reopens.
  await page.getByRole("tab", { name: /decided/ }).click();
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-01", "WSTG-INPV-05"]);
  await page.getByRole("list", { name: "Candidates" }).locator("li", { hasText: "WSTG-INPV-01" }).getByRole("button").click();
  await expect(cand.locator(".why")).toHaveText(`rejected: output is encoded ${P}`);
  await page.keyboard.press("o");
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-05"]);
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-00000002", body: { op: "reopen" } });
  await expect(page.getByRole("tab", { name: /to triage/ })).toContainText("1");

  expect(await page.locator(".findings img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("filter, accept risk and edit", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/#findings");
  await page.getByLabel("Filter candidates").fill("rule:inpv");
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-01", "WSTG-INPV-05"]);
  await page.getByLabel("Filter candidates").fill("sql error");
  await expect(queueIds(page)).toHaveText(["WSTG-INPV-05"]);
  await page.getByRole("button", { name: /Accept risk/ }).click();
  await page.getByRole("form", { name: "Accept the risk" }).getByLabel("Justification").fill("internal only, CISO signed off");
  await page.getByRole("form", { name: "Accept the risk" }).getByRole("button", { name: "accept" }).click();
  await expect(page.locator(".findings .empty")).toHaveText("Nothing here matches.");
  expect(api.calls.at(-1)?.body).toEqual({ op: "accept", justification: "internal only, CISO signed off" });

  await page.getByLabel("Filter candidates").fill("");
  await page.getByRole("button", { name: "Edit" }).click();
  const edit = page.getByRole("form", { name: "Edit" });
  await edit.getByLabel("Severity").selectOption("error");
  await edit.getByLabel("Confidence").fill("85");
  await edit.getByRole("button", { name: "save" }).click();
  await expect.poll(() => api.calls.at(-1)?.body).toMatchObject({ op: "edit", level: "error", rank: 85 });
});

test("evidence: exchanges open in Traffic, a replay compares, more are added (#45)", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto(`/#findings/c-00000001`);
  const ev = page.getByRole("group", { name: "Evidence" });
  const items = ev.getByRole("list", { name: "Evidence exchanges" }).locator("li");
  await expect(items).toHaveCount(2);
  await expect(items.nth(0)).toContainText("decisive");
  await expect(items.nth(0)).toContainText("500");
  await items.nth(1).getByRole("button", { name: "Compare" }).click();
  await expect(ev.locator(".compare .verdict")).toContainText("status 500 → 200");

  await ev.getByLabel("Add evidence").fill(`${s.entries[1]!.id}`);
  await ev.getByLabel("Add evidence").press("Enter");
  await expect(items).toHaveCount(3);
  expect(api.calls.at(-1)?.body).toEqual({ op: "evidence", exchanges: [s.entries[1]!.id] });
  await ev.getByLabel("Add evidence").fill("../etc");
  await ev.getByRole("button", { name: "add" }).click();
  await expect(ev.getByRole("alert")).toHaveText("not an exchange id: ../etc");

  // An exchange opens in Traffic's inspector.
  await items.nth(0).locator("button.link").click();
  await expect(page).toHaveURL(/#traffic$/);
  await expect(page.locator(".inspector h2")).toContainText(`${t}.test/p/1`);
});

test("the inspector's candidate links open it in Findings", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/");
  await page.locator("tr.row", { hasText: `${t}.test` }).filter({ hasText: "/o/7" }).click();
  await page.getByRole("list", { name: "Candidates for this exchange" }).getByRole("link", { name: /c-00000003/ }).click();
  await expect(page).toHaveURL(/#findings\/c-00000003$/);
  await expect(page.getByRole("complementary", { name: "Candidate" }).locator("h2")).toHaveText("c-00000003 · WSTG-ATHZ-04");
});

test("without the lever API, Findings says why", async ({ page }) => {
  await page.goto("/#findings");
  await expect(page.locator(".findings .empty")).toHaveText("Candidates need taphttp's lever API (start spall with --taphttp-api).");
});

test("Waiting on you lists candidates to triage, oldest first, and clears (#47)", async ({ page }) => {
  const { engagementApi } = await import("./engagement-api.ts");
  const t = token();
  const s = seed(t);
  append(...s.entries);
  engagementApi(page);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/#engagement");
  const panel = page.getByRole("group", { name: "Candidates to triage" });
  await expect(panel.getByRole("heading")).toHaveText("Candidates to triage · 3");
  await expect(panel.locator("li a")).toHaveText(["c-00000001 · WSTG-INPV-05", "c-00000002 · WSTG-INPV-01", "c-00000003 · WSTG-ATHZ-04"]);
  // Tasks and coverage stay dimmed until they ship.
  await expect(page.locator(".todo-panels li.todo")).toHaveCount(2);
  await panel.getByRole("link", { name: "c-00000002 · WSTG-INPV-01" }).click();
  await expect(page).toHaveURL(/#findings\/c-00000002$/);
  for (const id of ["c-00000001", "c-00000002", "c-00000003"]) {
    await page.evaluate((i) => fetch(`/api/candidates/${i}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: '{"op":"confirm"}' }), id);
  }
  expect(api.calls.filter((c) => c.method === "PATCH")).toHaveLength(3);
  await page.goto("/#engagement");
  await expect(panel.getByRole("heading")).toHaveText("Candidates to triage · 0");
  await expect(panel).toContainText("Nothing waiting: the queue is clear.");
  await expect(page.locator(".statusbar .to-triage")).toHaveCount(0);
});
