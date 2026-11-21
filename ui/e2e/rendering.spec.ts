// The inspector rendering rules (#5): repeated headers, binary bodies.
import { createHash } from "node:crypto";
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

test("repeated headers keep wire order and get n/N counters", async ({ page }) => {
  const t = token();
  append(
    entry({
      host: `${t}.example`,
      res_headers: [
        ["set-cookie", "a=1; Secure"],
        ["content-type", "text/html"],
        ["Set-Cookie", "b=2; HttpOnly"],
        ["set-cookie", "c=3"],
      ],
    }),
  );
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();

  const res = page.locator(".detail-grid section").nth(1).locator("pre").first();
  await expect(res).toHaveText(
    "set-cookie: a=1; Secure 1/3\ncontent-type: text/html\nSet-Cookie: b=2; HttpOnly 2/3\nset-cookie: c=3 3/3",
  );
  await expect(res.locator(".hdr-count")).toHaveText(["1/3", "2/3", "3/3"].map((c) => ` ${c}`));
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
  await page.goto("/");
  await filterTo(page, t);
  await page.locator("tr.row").first().click();

  const placeholder = page.locator("pre.binary");
  await expect(placeholder).toContainText("binary · application/octet-stream · 600 bytes");
  const sha = createHash("sha256").update(bytes).digest("hex");
  await expect(placeholder).toContainText(`sha256 ${sha}`);
  await expect(page.locator("main")).not.toContainText("<script>alert(1)</script>");

  await page.getByRole("button", { name: "hex view" }).click();
  const dump = page.locator("pre.hex-dump");
  await expect(dump).toBeVisible();
  const lines = async () => (await dump.textContent())!.split("\n");
  expect(await lines()).toHaveLength(64);
  expect((await lines())[0]).toBe("00000000  00 01 02 03 04 05 06 07  ........");
  expect((await lines())[4]).toBe("00000020  20 21 22 23 24 25 26 27   !\"#$%&'");

  const next = page.getByRole("button", { name: "next 88 bytes (88 left)" });
  await next.click();
  expect(await lines()).toHaveLength(75);
  expect((await lines())[64]).toBe("00000200  3c 73 63 72 69 70 74 3e  <script>");
  await expect(next).toHaveCount(0);

  await page.getByRole("button", { name: "hide hex" }).click();
  await expect(dump).toHaveCount(0);
});

test("JSON bodies are pretty-printed; bad base64 is reported, not rendered", async ({ page }) => {
  const t = token();
  append(
    entry({
      host: `${t}.json`,
      res_headers: [["content-type", "application/json"]],
      res_body: '{"a":{"b":[1,2]}}',
    }),
    entry({
      host: `${t}.bad`,
      res_body: "!!!not base64",
      res_body_encoding: "base64",
    }),
  );
  await page.goto("/");
  await filterTo(page, t);
  const rows = page.locator("tr.row");
  await expect(rows).toHaveCount(2);
  await rows.nth(0).click();
  await rows.nth(1).click();
  await expect(page.locator("main")).toContainText('{\n  "a": {\n    "b": [\n      1,\n      2\n    ]\n  }\n}');
  await expect(page.locator("pre.binary")).toHaveText("binary body (could not decode base64)");
});
