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
  await page.goto("/#traffic");
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

test("duplicates sit under their original, which shows where else it was seen (#78)", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  const [a, b] = s.entries as [ReturnType<typeof entry>, ReturnType<typeof entry>];
  const orig: Stored = {
    id: "c-00000010",
    created: a.ts,
    updated: a.ts,
    flagged_by: "human",
    seen_again: [{ by: "agent:T-4", at: "2026-11-24T03:00:00Z", exchanges: [b.id] }],
    finding: findingFor("WSTG-INFO-02", a, undefined, { correlationGuid: "6f1c2e3a-0000-4000-8000-00000000aaaa" }),
  };
  orig.finding.properties["flux/status"] = "confirmed";
  const maybe: Stored = { id: "c-00000011", created: b.ts, updated: b.ts, flagged_by: "human", possible_duplicate_of: orig.id, finding: findingFor("WSTG-INFO-02", b, "X-Powered-By") };
  const decided: Stored = {
    id: "c-00000012",
    created: b.ts,
    updated: b.ts,
    flagged_by: "agent:T-3",
    finding: {
      ...findingFor("WSTG-INFO-02", b),
      suppressions: [{ kind: "external", status: "accepted", justification: "same banner", properties: { "flux/disposition": "duplicate", "flux/duplicateOf": "6f1c2e3a-0000-4000-8000-00000000aaaa" } }],
    },
  };
  candidatesApi(page, { entries: s.entries, seed: [orig, ...s.list, maybe, decided] });
  await page.goto("/#findings");
  await page.getByRole("tab", { name: /^all/ }).click();
  const rows = page.getByRole("list", { name: "Candidates" }).locator("li");
  const ids = await rows.evaluateAll((ls) => ls.map((l) => (l.querySelector(".rule")?.textContent ?? "") + (l.classList.contains("dup") ? " ↳" : "")));
  const at = ids.indexOf("WSTG-INFO-02");
  expect(ids.slice(at, at + 3), ids.join(", ")).toEqual(["WSTG-INFO-02", "WSTG-INFO-02 ↳", "WSTG-INFO-02 ↳"]);
  await expect(rows.nth(at)).toContainText("seen ×2");
  await expect(rows.nth(at + 1)).toContainText(/(possible )?duplicate of c-00000010/);

  const cand = page.getByRole("complementary", { name: "Candidate" });
  await rows.filter({ hasText: "possible duplicate of c-00000010" }).getByRole("button").click();
  await expect(cand.locator("h2")).toHaveText("c-00000011 · WSTG-INFO-02");
  await expect(cand.locator(".kv")).toContainText("possible duplicate ofc-00000010 · WSTG-INFO-02 · confirmed");
  // d starts from the likely original.
  await page.keyboard.press("d");
  await expect(page.getByRole("form", { name: "Duplicate of another candidate" }).getByLabel("Duplicate of")).toHaveValue("c-00000010");
  await page.keyboard.press("Escape");

  await cand.getByRole("link", { name: /c-00000010/ }).click();
  await expect(cand.locator("h2")).toHaveText("c-00000010 · WSTG-INFO-02");
  await expect(cand.locator(".seen-again")).toContainText(`agent · T-4 · ${b.id}`);
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
  await page.goto("/#traffic");
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
  // Coverage is live (#94), and so are tasks waiting on approval (#56).
  await expect(page.getByRole("group", { name: "Coverage" })).toBeVisible();
  await expect(page.getByRole("group", { name: "Tasks waiting on approval" })).toContainText("No task is waiting on you.");
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

test("a long candidate line stays inside Waiting on you, at two columns and at phone width (#72)", async ({ page }) => {
  const { engagementApi } = await import("./engagement-api.ts");
  const t = token();
  const e = entry({ host: `host.docker.internal`, url: `http://host.docker.internal:8080/${t}/a/rather/long/path/to/something` });
  append(e);
  engagementApi(page);
  const long = "Server response header / error-page footer, and the X-Powered-By header on every response";
  const list: Stored[] = [{ id: "c-0000000a", created: e.ts, updated: e.ts, flagged_by: "agent:T-3", finding: findingFor("WSTG-INFO-02", e, long) }];
  candidatesApi(page, { entries: [e], seed: list });
  for (const width of [1100, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/#engagement");
    const panel = page.getByRole("group", { name: "Candidates to triage" });
    const line = panel.locator("li .note");
    await expect(line).toContainText("Server response header");
    const box = (await panel.boundingBox())!;
    for (const el of [line, panel.locator("li a")]) {
      const b = (await el.boundingBox())!;
      expect(b.x + b.width, `${width}px: ${await el.textContent()}`).toBeLessThanOrEqual(box.x + box.width + 0.5);
    }
    // Whatever is cut is still there to read.
    await expect(line).toHaveAttribute("title", /X-Powered-By header on every response/);
  }
});

test("notes: added, shown as text, deleted, and in the export only when ticked (#82)", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(...s.entries);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/#findings/c-00000001");
  const notes = page.getByRole("region", { name: "Notes" });
  await expect(notes).toContainText("None yet.");
  const box = notes.getByRole("checkbox", { name: /Include with export/ });
  await expect(box).not.toBeChecked();

  // Typing c, r or a in the note doesn't triage.
  await notes.getByLabel("New note").fill(`car? ad network ${P}\nask the client`);
  await notes.getByRole("button", { name: "Add note" }).click();
  await expect(notes.locator(".note-text")).toHaveText(`car? ad network ${P}\nask the client`);
  expect(await notes.locator("img").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(notes.getByLabel("New note")).toHaveValue("");
  expect(api.calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([{ op: "note", text: `car? ad network ${P}\nask the client` }]);
  await expect(page.locator(".findings")).toContainText("to triage");

  await box.check();
  await expect.poll(() => api.calls.at(-1)?.body).toEqual({ op: "notes_in_export", on: true });
  await expect(box).toBeChecked();

  await notes.getByRole("button", { name: /^Delete note/ }).click();
  await expect(notes).toContainText("None yet.");
  expect(api.calls.at(-1)?.body).toMatchObject({ op: "unnote" });
});

test("agent buttons draft a pre-filled task with its cost shown first; retests show as text (#67)", async ({ page }) => {
  const t = token();
  const s = seed(t);
  s.list[0]!.retests = [{ by: "agent:T-4", at: s.entries[0]!.ts, verdict: "fixed", exchange: s.entries[3]!.id, note: `no SQL error now ${P}` }];
  append(...s.entries);
  const api = candidatesApi(page, { entries: s.entries, seed: s.list });
  await page.goto("/#findings/c-00000001");
  const pane = page.getByRole("complementary", { name: "Candidate" });
  const retests = pane.getByRole("list", { name: "Retests" });
  await expect(retests).toContainText(`◆ agent · T-4 · fixed · ${s.entries[3]!.id} · no SQL error now ${P}`);
  expect(await pane.locator("img").count()).toBe(0);

  const agent = page.getByRole("region", { name: "Agent" });
  // Cost first, before anything is drafted: retest of 2 exchanges.
  await expect(agent).toContainText("~6 requests, the evidence's methods");
  await expect(agent.getByRole("button")).toHaveText(["Explain for devs", "Retest", "Find variants", "Escalate impact", "Build a PoC"]);
  await expect(agent.getByRole("button", { name: "Build a PoC" })).toBeVisible();
  await agent.getByRole("button", { name: "Retest" }).click();
  await expect(agent.getByRole("status")).toContainText("Drafted T-7 (tester, ~5 requests, safe methods): approve it in Tasks");
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/verbs/retest", body: { id: "c-00000001" } });
  // Typing in the pane didn't triage it.
  await expect(page.locator(".findings")).toContainText("to triage");
  await expect(agent.getByRole("link", { name: "T-7" })).toHaveAttribute("href", "#tasks/T-7");
  // The PoC button drafts a tester task like the others (non-destructive).
  // On an exchange, in the inspector: Test for… takes a category.
  await page.goto("/#traffic");
  await page.locator("tr.row", { hasText: `${t}.test` }).filter({ hasText: "/o/7" }).click();
  const onX = page.getByRole("region", { name: "Agent" });
  await expect(onX.getByRole("button")).toHaveText(["Why is this interesting?", "Map auth", "Test for…"]);
  await onX.getByLabel("WSTG category").selectOption("INPV");
  await onX.getByRole("button", { name: "Test for…" }).click();
  await expect(onX.getByRole("status")).toContainText("Drafted T-8");
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/verbs/test_for", body: { id: s.entries[2]!.id, category: "INPV" } });

  // The PoC button (candidate verb) drafts a tester task like the others.
  await page.goto("/#findings/c-00000001");
  const pocAgent = page.getByRole("region", { name: "Agent" });
  await pocAgent.getByRole("button", { name: "Build a PoC" }).click();
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/verbs/poc", body: { id: "c-00000001" } });
});
