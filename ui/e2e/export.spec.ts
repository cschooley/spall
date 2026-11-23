// M5: promotion (#50), the redaction preview (#49), export (#51), and the
// inspector's credential highlighting taken from flux, against the
// in-memory candidates API (candidates-api.ts) and spall's /api/redaction.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import { candidatesApi, findingFor, type Stored } from "./candidates-api.ts";
import { readFile } from "node:fs/promises";

const P = `<img src=x onerror="window.__pwned=1">`;

function seed(t: string) {
  const e = entry({
    host: `${t}.test`,
    method: "POST",
    url: `https://${t}.test/login?next=${encodeURIComponent(P)}`,
    status: 500,
    req_headers: [
      ["cookie", "sid=8f3a91c2d4e5"],
      ["authorization", "Bearer abcdefghijklmnop"],
      ["content-type", "application/json"],
    ],
  });
  const mk = (id: string, status: string): Stored => {
    const f = findingFor("WSTG-INPV-05", e, "user");
    f.properties["flux/status"] = status;
    return { id, created: e.ts, updated: e.ts, flagged_by: "human", finding: f };
  };
  return { e, list: [mk("c-00000001", "confirmed"), mk("c-00000002", "candidate")] };
}

test("p promotes a confirmed candidate; the redaction preview says what goes; Export saves the redacted report", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(s.e);
  const api = candidatesApi(page, { entries: [s.e], seed: s.list });
  await page.goto("/#findings/c-00000001");
  const cand = page.getByRole("complementary", { name: "Candidate" });
  await expect(cand.locator("h2")).toHaveText("c-00000001 · WSTG-INPV-05");

  const red = cand.getByRole("group", { name: "Redaction preview" });
  await expect(red.getByRole("list", { name: "Redactions" }).locator("li")).toHaveText([
    'webRequest.headers["cookie"][REDACTED:cookie]',
    'webRequest.headers["authorization"][REDACTED:authorization]',
  ]);
  await expect(red).toContainText(`exported target: https://${t}.test/login?next=`);

  const exportButton = page.getByRole("button", { name: /^Export \d+ promoted$/ });
  await expect(exportButton).toHaveText("Export 0 promoted");
  await expect(exportButton).toBeDisabled();
  await expect(page.getByRole("button", { name: /unredacted/ })).toHaveCount(0);

  await page.keyboard.press("p");
  await expect(cand).toContainText("in the next export");
  expect(api.calls.at(-1)).toEqual({ method: "PATCH", path: "/api/candidates/c-00000001", body: { op: "promote" } });
  await expect(page.getByRole("tab", { name: /promoted/ })).toContainText("1");
  await expect(exportButton).toHaveText("Export 1 promoted");

  const download = page.waitForEvent("download");
  await exportButton.click();
  const file = await download;
  expect(file.suggestedFilename()).toBe("spall-acme-1a2b-20261123.sarif");
  const report = JSON.parse(await readFile((await file.path())!, "utf8")) as { runs: { results: { properties: Record<string, unknown> }[] }[] };
  expect(report.runs[0]!.results).toHaveLength(1);
  expect(report.runs[0]!.results[0]!.properties["flux/redacted"]).toBe(true);
  await expect(page.locator(".findings .ok-msg")).toHaveText("exported spall-acme-1a2b-20261123.sarif (redacted)");
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/export", body: {} });

  // An unconfirmed one can't be promoted; p again unpromotes.
  await page.keyboard.press("p");
  await expect(exportButton).toHaveText("Export 0 promoted");
  await page.getByRole("tab", { name: /all/ }).click();
  await page.getByRole("list", { name: "Candidates" }).locator("li", { hasText: "to triage" }).getByRole("button").click();
  await expect(cand.getByRole("button", { name: /^Promote/ })).toBeDisabled();
  expect(await page.locator(".findings img").count()).toBe(0);
});

test("an unredacted export takes the server's opt-in and two presses", async ({ page }) => {
  const t = token();
  const s = seed(t);
  s.list[0]!.promoted = "2026-11-23T16:00:00Z";
  append(s.e);
  const api = candidatesApi(page, { entries: [s.e], seed: s.list, unredactedAllowed: true });
  await page.goto("/#findings");
  const raw = page.locator(".findings .export.raw");
  await raw.click();
  await expect(raw).toHaveText("press again: export UNREDACTED");
  expect(api.calls.filter((c) => c.path === "/api/export")).toHaveLength(0);
  const download = page.waitForEvent("download");
  await raw.click();
  expect((await download).suggestedFilename()).toBe("spall-acme-1a2b-20261123-UNREDACTED.sarif");
  expect(api.calls.at(-1)).toEqual({ method: "POST", path: "/api/export", body: { unredacted: true } });
  await expect(page.locator(".findings .replay-msg")).toContainText("UNREDACTED: contains captured credentials");
});

test("the console's promote and export", async ({ page }) => {
  const t = token();
  const s = seed(t);
  append(s.e);
  candidatesApi(page, { entries: [s.e], seed: s.list });
  await page.goto("/");
  await page.locator(".tablewrap").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("`");
  const run = async (line: string) => {
    await page.getByLabel("Console command").fill(line);
    await page.keyboard.press("Enter");
  };
  await run("export");
  await expect(page.locator(".con-log")).toContainText("export: nothing promoted to export");
  await run("promote c-00000002");
  await expect(page.locator(".con-log")).toContainText("promote: only a confirmed candidate");
  await run("promote c-00000001");
  await expect(page.locator(".con-log")).toContainText("c-00000001 promoted: in the next export");
  const download = page.waitForEvent("download");
  await run("export");
  expect((await download).suggestedFilename()).toBe("spall-acme-1a2b-20261123.sarif");
  await run("export --unredacted");
  await expect(page.locator(".con-log")).toContainText("export: an unredacted export needs spall started with --allow-unredacted-export");
});

test("the inspector highlights what flux redacts, x-api-key included", async ({ page }) => {
  const t = token();
  const long = "k".repeat(40);
  append(entry({ host: `${t}.test`, url: `https://${t}.test/x`, req_headers: [["x-api-key", long], ["x-request-id", long]] }));
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  const dd = page.locator(".inspector .kv dd");
  await expect(dd.nth(0)).toHaveClass("secret");
  await expect(dd.nth(0)).toContainText("…");
  await expect(dd.nth(1)).not.toHaveClass("secret");
});
