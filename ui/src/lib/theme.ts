// verdigris themes: brand (rust | blued) x mode (dark | light), applied as
// data-theme on <html>. Remembered per browser; storage may be unavailable.

export type Brand = "rust" | "blued";
export type Mode = "dark" | "light";
export type ThemeName = "rust" | "rust-light" | "blued" | "blued-light";

export const THEMES: readonly ThemeName[] = ["rust", "rust-light", "blued", "blued-light"];
/** verdigris: dark is the default, rust is the brand. */
export const DEFAULT_THEME: ThemeName = "rust";
const KEY = "spall.theme";

export function themeName({ brand, mode }: { brand: Brand; mode: Mode }): ThemeName {
  return mode === "dark" ? brand : `${brand}-light`;
}

export function themeParts(name: ThemeName): { brand: Brand; mode: Mode } {
  return {
    brand: name.startsWith("blued") ? "blued" : "rust",
    mode: name.endsWith("-light") ? "light" : "dark",
  };
}

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function loadTheme(storage: StorageLike | null): ThemeName {
  try {
    const v = storage?.getItem(KEY);
    return THEMES.includes(v as ThemeName) ? (v as ThemeName) : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

export function saveTheme(storage: StorageLike | null, name: ThemeName): void {
  try {
    storage?.setItem(KEY, name);
  } catch {
    // storage blocked: the choice lasts for this page only
  }
}

/** The browser's localStorage, or null where touching it throws. */
export function browserStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function applyTheme(name: ThemeName): void {
  // verdigris: no attribute means 'rust'.
  if (name === "rust") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", name);
}
