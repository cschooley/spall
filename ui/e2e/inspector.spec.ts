// The shared inspector: tabs, secrets, copy as curl, save body, close.
import { readFileSync } from "node:fs";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

test.use({ permissions: ["clipboard-read", "clipboard-write"] });

const SECRET = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload.signature";

test("selecting a row opens the inspector with request and response tabs", async ({ page }) => {
  const t = token();
  append(
    entry({
      host: `${t}.example`,
      method: "POST",
      url: `https://${t}.example/api/v2/session`,
      status: 401,
      duration_ms: 184,
      req_headers: [["content-type", "application/json"], ["authorization", SECRET]],
      req_body: '{"username":"admin"}',
      res_headers: [["content-type", "text/plain"], ["set-cookie", "s=1"]],
      res_body: "nope",
    }),
  );
  await page.goto("/");
  await filterTo(page, t);
  await expect(page.locator(".inspector")).toHaveCount(0);
  await page.locator("tr.row").first().click();

  const insp = page.locator(".inspector");
  await expect(insp.locator("h2")).toHaveText(`POST https://${t}.example/api/v2/session`);
  await expect(insp.locator(".meta")).toContainText("401 Unauthorized");
  await expect(insp.locator(".meta")).toContainText("184 ms");
  await expect(page.locator("tr.row.sel")).toHaveCount(1);

  // Request tab: credential header highlighted and shortened until revealed.
  const auth = insp.locator("dd.secret").first();
  await expect(auth).toContainText("Bearer eyJhbGciOiJIUzI1N…");
  await expect(auth).not.toContainText("signature");
  await auth.getByRole("button", { name: "show" }).click();
  await expect(auth).toContainText(SECRET);
  await expect(insp.locator(".pane pre")).toHaveText('{\n  "username": "admin"\n}');

  // Response tab.
  await page.getByRole("tab", { name: "response" }).click();
  await expect(insp.locator("dd.secret")).toHaveText("s=1");
  await expect(insp.locator(".pane pre")).toHaveText("nope");

  // Close with the button, reopen, close with Esc.
  await page.getByRole("button", { name: "Close inspector" }).click();
  await expect(insp).toHaveCount(0);
  await page.locator("tr.row").first().click();
  await page.keyboard.press("Escape");
  await expect(insp).toHaveCount(0);
});

test("copy as curl puts a shell-safe command on the clipboard", async ({ page }) => {
  const t = token();
  append(
    entry({
      host: `${t}.example`,
      method: "POST",
      url: `https://${t}.example/x?a='b'`,
      req_headers: [["content-type", "application/json"]],
      req_body: `{"q":"it's"}`,
    }),
  );
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await page.getByRole("button", { name: "Copy as curl" }).click();
  await expect(page.getByRole("button", { name: "copied" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    `curl -X 'POST' 'https://${t}.example/x?a='\\''b'\\''' -H 'content-type: application/json' --data-binary '{"q":"it'\\''s"}'`,
  );
});

test("save body downloads the exact captured bytes", async ({ page }) => {
  const t = token();
  const bytes = Buffer.from([0, 1, 2, 255, 0x3c, 0x62, 0x3e]);
  append(entry({ host: `${t}.example`, res_body: bytes.toString("base64"), res_body_encoding: "base64" }));
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await expect(page.getByRole("button", { name: "Save request body" })).toBeDisabled();
  await page.getByRole("tab", { name: "response" }).click();
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Save response body" }).click(),
  ]);
  expect(download.suggestedFilename()).toMatch(/^spall-[A-Za-z0-9_-]+-response\.bin$/);
  expect(readFileSync((await download.path())!)).toEqual(bytes);
});
