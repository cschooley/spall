// Captured traffic is attacker-controlled (docs/architecture.md, Frontend).
// Every field the console renders gets a payload that would execute or
// inject elements if it were ever treated as HTML.
import { append, entry, expect, filterTo, test, token } from "./fixtures.ts";

const P = {
  script: "<script>window.__pwned='script'</script>",
  img: `<img src=x onerror="window.__pwned='img'">`,
  svg: `<svg onload="window.__pwned='svg'"></svg>`,
  iframe: `<iframe src="javascript:window.__pwned='iframe'"></iframe>`,
  bold: "<b>bold</b>",
};

test("traffic is rendered as text in every field, and nothing executes", async ({ page }) => {
  const t = token();
  const e = entry({
    id: `${t}-${P.bold}`,
    method: `GET${P.img}`,
    host: `${t}.example${P.svg}`,
    url: `http://${t}.example/${P.script}?q=${P.iframe}`,
    req_headers: [
      [`x-${P.bold}`, P.img],
      ["user-agent", P.script],
    ],
    req_body: `${P.svg}${P.iframe}`,
    res_headers: [
      ["content-type", "text/html"],
      ["set-cookie", P.script],
    ],
    res_body: `<!doctype html><html><body>${P.script}${P.img}</body></html>`,
    replay_of: P.img,
  });
  const jsonEntry = entry({
    host: `${t}.json`,
    url: `http://${t}.json/api`,
    res_headers: [["content-type", "application/json"]],
    res_body: JSON.stringify({ html: P.svg, nested: { s: P.script } }),
  });
  append(e, jsonEntry);

  await page.goto("/");
  await filterTo(page, t);
  const rows = page.locator("tr.row");
  await expect(rows).toHaveCount(2);
  await rows.nth(0).click();
  await rows.nth(1).click();

  // The payloads are visible, literally.
  const body = page.locator("main");
  for (const p of [P.script, P.img, P.svg, P.iframe]) {
    await expect(body).toContainText(p);
  }
  await expect(body).toContainText(`id ${t}-${P.bold}`);
  await expect(body).toContainText(`replay of ${P.img}`);
  // JSON bodies are pretty-printed with the strings still escaped as text.
  await expect(body).toContainText(`"html": ${JSON.stringify(P.svg)}`);

  // Nothing executed, nothing was injected.
  expect(await page.evaluate(() => (window as unknown as { __pwned?: string }).__pwned)).toBeUndefined();
  expect(await page.locator("img, svg, iframe, object, embed").count()).toBe(0);
  expect(await page.evaluate(() => document.scripts.length)).toBe(1);
  // <b>bold</b> stayed text: no element whose own text is just "bold".
  expect(
    await page.evaluate(() => [...document.querySelectorAll("b")].filter((b) => b.textContent === "bold").length),
  ).toBe(0);
});
