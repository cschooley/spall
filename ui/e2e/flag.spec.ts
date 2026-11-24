// Flag as candidate (#43) and decisions that stick (#46), against the
// in-memory candidates API in candidates-api.ts.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import { candidatesApi, findingFor } from "./candidates-api.ts";

const P = `<img src=x onerror="window.__pwned=1">`;

test("f opens the flag form; a rule, a parameter, and it's filed with this exchange", async ({ page }) => {
  const t = token();
  const e = entry({ host: `${t}.test`, method: "GET", url: `https://${t}.test/api/p/17?sort=x'&page=2`, status: 500 });
  const r = entry({ id: `${e.id}-r`, host: e.host, url: e.url, ts: new Date(Date.parse(e.ts) + 1000).toISOString(), replay_of: e.id, status: 500 });
  append(e, r);
  const api = candidatesApi(page, { entries: [e, r] });
  await page.goto("/#traffic");
  await filterTo(page, `${t} replay:no`);
  await page.locator("tr.row").first().click();
  await expect(page.locator(".inspector [data-action='flag']")).toBeEnabled();
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("f");

  const form = page.getByRole("form", { name: "Flag as candidate" });
  await expect(form).toBeVisible();
  await form.getByLabel("Find a rule").fill("sql injection");
  await expect(form.getByLabel("Rule", { exact: true }).locator("option")).toHaveText(["WSTG-INPV-05 Testing for SQL Injection (CWE-89)"]);
  await form.getByLabel("Rule", { exact: true }).selectOption("WSTG-INPV-05");
  await expect(form).toContainText("05-Testing_for_SQL_Injection");
  // Parameter suggestions come from the URL's query.
  await expect(form.locator(`datalist option`)).toHaveCount(2);
  await form.getByLabel("Parameter").fill("sort");
  await form.getByLabel("Severity").selectOption("error");
  await form.getByLabel(/replay/).check();
  await page.keyboard.press("Control+Enter");

  await expect(page.getByRole("status")).toHaveText("✓ Flagged as c-00000001: WSTG-INPV-05, waiting for triage.");
  expect(api.calls[0]).toEqual({
    method: "POST",
    path: "/api/candidates",
    body: { exchange: e.id, rule: "WSTG-INPV-05", parameter: "sort", level: "error", replays: [r.id] },
  });
  await expect(page.getByRole("link", { name: "open in Findings" })).toHaveAttribute("href", "#findings/c-00000001");
  await page.getByRole("button", { name: "close", exact: true }).click();
  // The inspector lists this exchange's candidates.
  const strip = page.getByRole("list", { name: "Candidates for this exchange" });
  await expect(strip).toContainText("c-00000001 · WSTG-INPV-05to triage");
  // The status bar counts what waits for triage.
  await expect(page.locator(".statusbar .to-triage")).toHaveText("1 to triage");
});

test("flagging the same thing again offers to add evidence instead", async ({ page }) => {
  const t = token();
  const a = entry({ host: `${t}.test`, url: `https://${t}.test/u/1?id=1` });
  const b = entry({ host: `${t}.test`, url: `https://${t}.test/u/1?id=2`, ts: new Date(Date.parse(a.ts) + 1000).toISOString() });
  append(a, b);
  const api = candidatesApi(page, {
    entries: [a, b],
    seed: [{ id: "c-00000009", created: "", updated: "", flagged_by: "human", finding: findingFor("WSTG-ATHZ-04", a, "id") }],
  });
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row", { hasText: "id=2" }).click();
  await page.locator(".inspector [data-action='flag']").click();
  const form = page.getByRole("form", { name: "Flag as candidate" });
  await form.getByLabel("Find a rule").fill("athz-04");
  await form.getByLabel("Rule", { exact: true }).selectOption("WSTG-ATHZ-04");
  await form.getByLabel("Parameter").fill("id");
  await form.getByRole("button", { name: /^flag/ }).click();
  await expect(page.getByRole("status")).toHaveText("Already flagged as c-00000009 (WSTG-ATHZ-04, to triage). Add this exchange to its evidence instead?");
  await page.getByRole("button", { name: "add as evidence" }).click();
  await expect(page.getByRole("status")).toHaveText("✓ Added to c-00000009's evidence (2 exchanges).");
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-00000009", body: { op: "evidence", exchanges: [b.id] } });
});

test("the same issue in other words warns; flag anyway marks it a possible duplicate (#78)", async ({ page }) => {
  const t = token();
  const a = entry({ host: `${t}.test`, url: `https://${t}.test/` });
  const b = entry({ host: `${t}.test`, method: "HEAD", url: `https://${t}.test/missing`, ts: new Date(Date.parse(a.ts) + 1000).toISOString() });
  append(a, b);
  const api = candidatesApi(page, {
    entries: [a, b],
    seed: [{ id: "c-00000009", created: "", updated: "", flagged_by: "human", finding: findingFor("WSTG-INFO-02", a, "Server header") }],
  });
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row", { hasText: "/missing" }).click();
  await page.locator(".inspector [data-action='flag']").click();
  const form = page.getByRole("form", { name: "Flag as candidate" });
  await form.getByLabel("Find a rule").fill("info-02");
  await form.getByLabel("Rule", { exact: true }).selectOption("WSTG-INFO-02");
  await form.getByLabel("Parameter").fill("X-Powered-By");
  await form.getByRole("button", { name: /^flag/ }).click();
  const group = page.getByRole("group", { name: "Flag as candidate" });
  await expect(group.getByRole("status")).toHaveText("Possibly a duplicate of c-00000009 (WSTG-INFO-02, to triage)");
  await expect(group).toContainText(`the same rule on host ${t}.test`);
  await expect(group.getByRole("button", { name: "add as evidence" })).toBeEnabled();
  await group.getByRole("button", { name: "flag anyway" }).click();
  await expect(page.getByRole("status")).toHaveText("✓ Flagged as c-00000002: WSTG-INFO-02, waiting for triage.");
  expect(api.calls.at(-1)).toEqual({
    method: "POST",
    path: "/api/candidates",
    body: { exchange: b.id, rule: "WSTG-INFO-02", parameter: "X-Powered-By", level: "warning", force: true },
  });
  expect(api.list.at(-1)!.possible_duplicate_of).toBe("c-00000009");
});

test("a rejected match is badged with its reason, as text, and can be reopened (#46)", async ({ page }) => {
  const t = token();
  const e = entry({ host: `${t}.test`, url: `https://${t}.test/search?q=${encodeURIComponent(P)}` });
  append(e);
  const api = candidatesApi(page, {
    entries: [e],
    seed: [
      {
        id: "c-0000000a",
        created: "",
        updated: "",
        flagged_by: "human",
        finding: findingFor("WSTG-INPV-01", e, "q", {
          suppressions: [{ kind: "external", status: "accepted", justification: `output is encoded ${P}`, properties: { "flux/disposition": "falsePositive" } }],
        }),
      },
    ],
  });
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await page.locator(".inspector [data-action='flag']").click();
  const form = page.getByRole("form", { name: "Flag as candidate" });
  await form.getByLabel("Find a rule").fill("reflected");
  await form.getByLabel("Rule", { exact: true }).selectOption("WSTG-INPV-01");
  await form.getByLabel("Parameter").fill("q");
  await form.getByRole("button", { name: /^flag/ }).click();
  await expect(page.getByRole("status")).toHaveText("Previously rejected: c-0000000a (WSTG-INPV-01)");
  await expect(page.locator(".flag .why")).toHaveText(`output is encoded ${P}`);
  expect(await page.locator(".inspector img").count()).toBe(0);
  await page.getByRole("button", { name: "reopen it" }).click();
  await expect(page.getByRole("status")).toHaveText("✓ Reopened c-0000000a: back in the queue.");
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-0000000a", body: { op: "reopen" } });
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});

test("Esc cancels the form; Flag is off without the lever API", async ({ page }) => {
  const t = token();
  const e = entry({ host: `${t}.test`, url: `https://${t}.test/x` });
  append(e);
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await expect(page.locator(".inspector [data-action='flag']")).toBeDisabled();

  candidatesApi(page, { entries: [e] });
  await page.reload();
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await page.locator(".inspector [data-action='flag']").click();
  await expect(page.getByRole("form", { name: "Flag as candidate" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("form", { name: "Flag as candidate" })).toHaveCount(0);
  await expect(page.locator(".inspector")).toBeVisible();
});

test("the console's flag files one and reports a match", async ({ page }) => {
  const t = token();
  const e = entry({ host: `${t}.test`, url: `https://${t}.test/p?sort=1` });
  append(e);
  const api = candidatesApi(page, { entries: [e] });
  await page.goto("/#traffic");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const run = async (line: string) => {
    await page.getByLabel("Console command").fill(line);
    await page.keyboard.press("Enter");
  };
  await run(`flag ${e.id} --rule WSTG-INPV-05 --param sort`);
  await expect(page.locator(".con-log")).toContainText(`flagged c-00000001: WSTG-INPV-05 · GET https://${t}.test/p · parameter sort`);
  await run(`flag ${e.id} --rule WSTG-INPV-05 --param sort`);
  await expect(page.locator(".con-log")).toContainText("flag: already flagged as c-00000001 (WSTG-INPV-05, to triage)");
  expect(api.calls.filter((c) => c.method === "POST")).toHaveLength(2);
});
