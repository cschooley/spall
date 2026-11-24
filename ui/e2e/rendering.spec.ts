// The inspector rendering rules (#5): repeated headers, binary bodies.
import { createHash } from "node:crypto";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";
import type { Page } from "@playwright/test";

async function openResponse(page: Page, t: string) {
  await page.goto("/#traffic");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: "response" }).click();
}

test("repeated headers keep wire order and get n/N counters", async ({ page }) => {
  const t = token();
  append(
    entry({
      host: `${t}.example`,
      res_headers: [
        ["x-a", "a=1; Secure"],
        ["content-type", "text/html"],
        ["X-A", "b=2; HttpOnly"],
        ["x-a", "c=3"],
      ],
    }),
  );
  await openResponse(page, t);
  const kv = page.locator(".pane dl.kv");
  await expect(kv.locator("dt")).toHaveText(["x-a", "content-type", "X-A", "x-a"]);
  await expect(kv.locator("dd")).toHaveText(["a=1; Secure 1/3", "text/html", "b=2; HttpOnly 2/3", "c=3 3/3"]);
});

/** 0..255 twice, an HTML-looking string, 63 zero bytes: 600 bytes. */
function sample(): Buffer {
  return Buffer.concat([
    Buffer.from([...Array(256).keys()]),
    Buffer.from([...Array(256).keys()]),
    Buffer.from("<script>alert(1)</script>"),
    Buffer.alloc(63),
  ]);
}

test("binary bodies show type, size and sha256, never text, with a paged hex view", async ({ page }) => {
  const t = token();
  const bytes = sample();
  append(
    entry({
      host: `${t}.example`,
      res_headers: [["content-type", "application/octet-stream"]],
      res_body: bytes.toString("base64"),
      res_body_encoding: "base64",
    }),
  );
  await openResponse(page, t);

  const block = page.locator(".binary");
  await expect(block.locator(".binrow")).toHaveText("BINARY application/octet-stream · 600 B · 600 bytes");
  await expect(block.locator(".sha")).toHaveText(`sha256 ${createHash("sha256").update(bytes).digest("hex")}`);
  await expect(page.locator(".pane")).not.toContainText("<script>alert(1)</script>");

  // x toggles the hex view; it opens on the first 32 bytes.
  await page.keyboard.press("x");
  const dump = page.locator("pre.hex-dump");
  const lines = async () => (await dump.textContent())!.split("\n");
  await expect(dump).toBeVisible();
  expect(await lines()).toEqual([
    "00000000  00 01 02 03 04 05 06 07  ........",
    "00000008  08 09 0a 0b 0c 0d 0e 0f  ........",
    "00000010  10 11 12 13 14 15 16 17  ........",
    "00000018  18 19 1a 1b 1c 1d 1e 1f  ........",
  ]);
  const more = block.locator(".more");
  await expect(more).toHaveText("first 32 of 600 bytes · load more");
  await block.getByRole("button", { name: "load more" }).click();
  expect(await lines()).toHaveLength(68);
  expect((await lines())[64]).toBe("00000200  3c 73 63 72 69 70 74 3e  <script>");
  await expect(more).toHaveText("first 544 of 600 bytes · load more");
  await block.getByRole("button", { name: "load more" }).click();
  expect(await lines()).toHaveLength(75);
  await expect(more).toHaveText("first 600 of 600 bytes");
  await block.getByRole("button", { name: /Hide hex/ }).click();
  await expect(dump).toHaveCount(0);
});

test("JSON bodies are pretty-printed; bad base64 is reported, not rendered", async ({ page }) => {
  const t = token();
  append(
    entry({ host: `${t}.json`, res_headers: [["content-type", "application/json"]], res_body: '{"a":{"b":[1,2]}}' }),
    entry({ host: `${t}.bad`, res_body: "!!!not base64", res_body_encoding: "base64" }),
  );
  await openResponse(page, `host:${t}.json`);
  await expect(page.locator(".pane pre")).toHaveText('{\n  "a": {\n    "b": [\n      1,\n      2\n    ]\n  }\n}');
  await filterTo(page, `host:${t}.bad`);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: "response" }).click();
  await expect(page.locator(".binary")).toHaveText("binary body (could not decode base64)");
});

test("a body can be read as JSON or raw, and a binary one as text; every view is text", async ({ page }) => {
  const t = token();
  const hostile = `{"x":"<img src=x onerror=\\"window.__pwned=1\\">","n":1}`;
  append(
    entry({ host: `${t}.json`, res_headers: [["content-type", "application/json"]], res_body: hostile }),
    entry({ host: `${t}.html`, res_headers: [["content-type", "application/json"]], res_body: "<b>not json</b>" }),
    entry({ host: `${t}.bin`, res_body: Buffer.from("hi ✓ <script>window.__pwned=1</script>").toString("base64"), res_body_encoding: "base64" }),
  );
  const views = page.getByRole("group", { name: "Body view" });
  const pre = page.locator(".pane pre");

  await openResponse(page, `host:${t}.json`);
  await expect(views.getByRole("button", { name: "json" })).toHaveAttribute("aria-pressed", "true");
  await expect(pre).toHaveText('{\n  "x": "<img src=x onerror=\\"window.__pwned=1\\">",\n  "n": 1\n}');
  await views.getByRole("button", { name: "raw" }).click();
  await expect(pre).toHaveText(hostile, { useInnerText: false });
  await expect(views.getByRole("button", { name: "raw" })).toHaveAttribute("aria-pressed", "true");

  // Not JSON: raw only, and the mismatch is said.
  await filterTo(page, `host:${t}.html`);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: "response" }).click();
  await expect(views.getByRole("button", { name: "json" })).toBeDisabled();
  await expect(views).toContainText("content-type says JSON, but it doesn't parse");
  await expect(pre).toHaveText("<b>not json</b>");

  // Binary: the hex view by default; as text on request.
  await filterTo(page, `host:${t}.bin`);
  await page.locator("tr.row").first().click();
  await page.getByRole("tab", { name: "response" }).click();
  await expect(page.locator(".binary")).toContainText("BINARY");
  await views.getByRole("button", { name: "text" }).click();
  await expect(pre).toHaveText("hi ✓ <script>window.__pwned=1</script>");

  expect(await page.locator(".pane img, .pane b, .pane script").count()).toBe(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
});
