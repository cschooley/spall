// Captured traffic is attacker-controlled (docs/architecture.md, Frontend).
// Every field the Traffic view renders, in the table and in both inspector
// tabs, gets a payload that would execute or inject elements if it were
// ever treated as HTML.
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
      ["authorization", `Bearer ${P.svg}`],
    ],
    req_body: `${P.svg}${P.iframe}`,
    res_headers: [
      ["content-type", "text/html"],
      ["set-cookie", P.bold],
      [P.iframe, P.script],
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
  const app = page.locator(".app");

  // Table and inspector header, request tab.
  await rows.filter({ hasText: `${t}.example` }).click();
  await expect(page.locator(".inspector")).toBeVisible();
  for (const p of [P.script, P.img, P.svg, P.iframe]) await expect(app).toContainText(p);
  await expect(app).toContainText(`id ${t}-${P.bold}`);
  await expect(app).toContainText(`replay of ${P.img}`);
  await expect(app).toContainText(`x-${P.bold}`);

  // Response tab.
  await page.getByRole("tab", { name: "response" }).click();
  await expect(page.locator(".pane")).toContainText(P.iframe);
  await expect(page.locator(".pane")).toContainText(`${P.script}${P.img}`);

  // JSON bodies are pretty-printed with the strings still text.
  await rows.filter({ hasText: `${t}.json` }).click();
  await page.getByRole("tab", { name: "response" }).click();
  await expect(page.locator(".pane")).toContainText(`"html": ${JSON.stringify(P.svg)}`);

  // Nothing executed, nothing was injected.
  expect(await page.evaluate(() => (window as unknown as { __pwned?: string }).__pwned)).toBeUndefined();
  // spall's own icons are <svg data-icon>; any other svg/img/frame was injected.
  expect(await page.locator("img, svg:not([data-icon]), iframe, object, embed").count()).toBe(0);
  expect(await page.locator(".tablewrap svg, .inspector svg").count()).toBe(0);
  expect(await page.evaluate(() => document.scripts.length)).toBe(1);
  expect(
    await page.evaluate(() => [...document.querySelectorAll("b")].filter((b) => b.textContent === "bold").length),
  ).toBe(0);
});
