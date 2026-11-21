import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_THEME, loadTheme, saveTheme, themeName, themeParts } from "./theme.ts";

test("brand x mode maps onto verdigris's four theme names", () => {
  assert.equal(themeName({ brand: "rust", mode: "dark" }), "rust");
  assert.equal(themeName({ brand: "rust", mode: "light" }), "rust-light");
  assert.equal(themeName({ brand: "blued", mode: "dark" }), "blued");
  assert.equal(themeName({ brand: "blued", mode: "light" }), "blued-light");
  for (const n of ["rust", "rust-light", "blued", "blued-light"] as const) {
    assert.equal(themeName(themeParts(n)), n);
  }
});

test("dark rust is the default (verdigris: dark is the default)", () => {
  assert.equal(DEFAULT_THEME, "rust");
});

function memoryStorage(initial: Record<string, string> = {}) {
  const m = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    dump: () => Object.fromEntries(m),
  };
}

test("load returns a saved valid theme, else the default", () => {
  assert.equal(loadTheme(memoryStorage({ "spall.theme": "blued-light" })), "blued-light");
  assert.equal(loadTheme(memoryStorage({ "spall.theme": "neon" })), "rust");
  assert.equal(loadTheme(memoryStorage()), "rust");
  assert.equal(loadTheme(null), "rust");
});

test("storage failures never break the page", () => {
  const throwing = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(loadTheme(throwing), "rust");
  assert.doesNotThrow(() => saveTheme(throwing, "blued"));
});

test("save stores the theme name", () => {
  const s = memoryStorage();
  saveTheme(s, "rust-light");
  assert.deepEqual(s.dump(), { "spall.theme": "rust-light" });
});
