// verdigris styling (#6): four themes, remembered, colors from the tokens.
import { readFileSync } from "node:fs";
import { expect, test } from "./fixtures.ts";

const tokens = readFileSync(new URL("../src/vendor/verdigris/tokens.css", import.meta.url), "utf8");

/** --vg-bg-base for a theme, from the vendored tokens, as the browser's rgb(). */
function bgBase(theme: string): string {
  const selector = theme === "rust" ? ':root, [data-theme="rust"]' : `[data-theme="${theme}"]`;
  const block = tokens.slice(tokens.indexOf(selector));
  const hex = /--vg-bg-base:\s*#([0-9a-fA-F]{6})/.exec(block)![1]!;
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return `rgb(${r}, ${g}, ${b})`;
}

const bodyBg = (page: import("@playwright/test").Page) =>
  page.evaluate(() => getComputedStyle(document.body).backgroundColor);

test("dark rust is the default, with no data-theme attribute", async ({ page }) => {
  await page.goto("/");
  expect(await page.evaluate(() => document.documentElement.getAttribute("data-theme"))).toBeNull();
  expect(await bodyBg(page)).toBe(bgBase("rust"));
  await expect(page.getByRole("group", { name: "Metal" }).getByRole("button", { name: "Rust" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

test("the picker reaches all four themes, each with its own tokens, and is remembered", async ({ page }) => {
  await page.goto("/");
  const brand = page.getByRole("group", { name: "Metal" });
  const mode = page.getByRole("group", { name: "Brightness" });
  for (const [b, m, theme] of [
    ["Blued", "Dark", "blued"],
    ["Blued", "Light", "blued-light"],
    ["Rust", "Light", "rust-light"],
    ["Rust", "Dark", "rust"],
    ["Blued", "Light", "blued-light"],
  ] as const) {
    await brand.getByRole("button", { name: b }).click();
    await mode.getByRole("button", { name: m }).click();
    const attr = await page.evaluate(() => document.documentElement.getAttribute("data-theme"));
    expect(attr ?? "rust", `${b}/${m}`).toBe(theme);
    await expect.poll(() => bodyBg(page), `${b}/${m}`).toBe(bgBase(theme));
  }
  await page.reload();
  expect(await page.evaluate(() => document.documentElement.getAttribute("data-theme"))).toBe("blued-light");
  expect(await bodyBg(page)).toBe(bgBase("blued-light"));
  // Leave the default for the other tests sharing this browser storage.
  await page.evaluate(() => localStorage.clear());
});

test("IBM Plex is bundled and loads under the CSP", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => document.fonts.check('13px "IBM Plex Mono"'))).toBe(true);
  expect(await page.evaluate(() => document.fonts.check('500 11px "IBM Plex Sans"'))).toBe(true);
  const loaded = await page.evaluate(() =>
    [...document.fonts].filter((f) => f.status === "loaded").map((f) => `${f.family} ${f.weight}`),
  );
  expect(loaded.some((f) => f.includes("IBM Plex Mono"))).toBe(true);
});
