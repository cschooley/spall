// One command registry (docs/ux.md, "The console and quad mode"): the
// palette, the ` console and, from M5, the MCP server expose the same
// commands with the same names and arguments. Pure: commands act through a
// Ctx the app supplies, so this module is unit-tested without a browser.

import { THEMES, type ThemeName } from "../lib/theme.ts";
import type { TrafficEntry } from "../types.ts";

export type OutKind = "cmd" | "out" | "sys" | "warn" | "stop";

export interface Ctx {
  out(text: string, kind?: OutKind): void;
  setFilter(query: string): void;
  focusFilter(): void;
  /** Select an exchange by id (loaded rows first, then history). */
  openById(id: string): Promise<boolean>;
  selected(): TrafficEntry | null;
  /** The selected exchange as a curl command, or null if none. */
  curlText(): string | null;
  copyCurl(): Promise<boolean>;
  jumpToNewest(): void;
  clearTable(): void;
  setTheme(name: ThemeName): void;
  clearConsole(): void;
  openKeys(): void;
  /** Set the console toggle key; false if not a single character. */
  setConsoleKey(key: string): boolean;
}

export interface Command {
  /** Stable identifier, shared with MCP (M5). Never a UI label. */
  name: string;
  usage: string;
  desc: string;
  group: "traffic" | "view" | "app";
  /** May be offered to the agent over MCP. Never for export or promotion. */
  mcp: boolean;
  hidden?: boolean;
  /** Palette entry; commands that need arguments open the console prefilled. */
  palette?: { label: string; hint?: string; needsArgs?: boolean };
  /** Completions for the argument being typed. */
  complete?: (args: string[]) => readonly string[];
  /** `rest` is the raw text after the name, for commands that keep spacing. */
  run(args: string[], ctx: Ctx, rest: string): void | Promise<void>;
}

const ID = /^[A-Za-z0-9_-]{1,128}$/;
const VIEWS = ["engagement", "traffic", "surface", "tasks", "findings"] as const;

export const COMMANDS: readonly Command[] = [
  {
    name: "help",
    usage: "help",
    desc: "this list",
    group: "app",
    mcp: true,
    run: (_a, ctx) => {
      for (const c of COMMANDS) if (!c.hidden) ctx.out(`  ${c.usage.padEnd(24)} ${c.desc}`);
    },
  },
  {
    name: "filter",
    usage: "filter [query]",
    desc: "filter traffic (host: method: status: text); empty clears",
    group: "traffic",
    mcp: true,
    palette: { label: "Filter traffic…", hint: "filter <query>", needsArgs: true },
    run: (_a, ctx, rest) => ctx.setFilter(rest),
  },
  {
    name: "open",
    usage: "open <id>",
    desc: "open an exchange in the inspector",
    group: "traffic",
    mcp: true,
    palette: { label: "Open exchange by id…", hint: "open <id>", needsArgs: true },
    run: async ([id], ctx) => {
      if (!id) return ctx.out("usage: open <id>", "warn");
      if (!ID.test(id)) return ctx.out(`open: not an exchange id: ${id}`, "warn");
      if (!(await ctx.openById(id))) ctx.out(`open: no exchange ${id}`, "warn");
    },
  },
  {
    name: "curl",
    usage: "curl",
    desc: "the selected exchange as a curl command (also copied)",
    group: "traffic",
    mcp: true,
    palette: { label: "Copy as curl", hint: "curl" },
    run: async (_a, ctx) => {
      const text = ctx.curlText();
      if (!text) return ctx.out("curl: select an exchange first", "warn");
      ctx.out(text);
      ctx.out((await ctx.copyCurl()) ? "copied to the clipboard" : "copy failed (clipboard unavailable)", "sys");
    },
  },
  {
    name: "newest",
    usage: "newest",
    desc: "jump to the newest exchange and follow",
    group: "traffic",
    mcp: false,
    palette: { label: "Jump to newest", hint: "newest" },
    run: (_a, ctx) => ctx.jumpToNewest(),
  },
  {
    name: "clear-table",
    usage: "clear-table",
    desc: "hide what's in the table; new traffic still arrives",
    group: "traffic",
    mcp: false,
    palette: { label: "Clear table", hint: "clear-table" },
    run: (_a, ctx) => ctx.clearTable(),
  },
  {
    name: "goto",
    usage: "goto <view>",
    desc: VIEWS.join(" | "),
    group: "view",
    mcp: false,
    complete: () => VIEWS,
    run: ([view], ctx) => {
      if (view === "traffic") return ctx.out("already on traffic");
      if ((VIEWS as readonly string[]).includes(view ?? "")) return ctx.out(`goto: ${view} is not built yet`, "warn");
      ctx.out(`goto: ${VIEWS.join(" | ")}`, "warn");
    },
  },
  {
    name: "theme",
    usage: "theme <name>",
    desc: THEMES.join(" | "),
    group: "app",
    mcp: false,
    palette: { label: "Theme…", hint: "theme <name>", needsArgs: true },
    complete: () => THEMES,
    run: ([name], ctx) => {
      if (!THEMES.includes(name as ThemeName)) return ctx.out(`theme: ${THEMES.join(" | ")}`, "warn");
      ctx.setTheme(name as ThemeName);
      ctx.out(`theme ${name}`, "sys");
    },
  },
  {
    name: "keys",
    usage: "keys",
    desc: "keyboard shortcuts",
    group: "app",
    mcp: false,
    palette: { label: "Keyboard shortcuts", hint: "?" },
    run: (_a, ctx) => ctx.openKeys(),
  },
  {
    name: "console-key",
    usage: "console-key <char>",
    desc: "key that toggles this console (default `)",
    group: "app",
    mcp: false,
    run: ([key], ctx) => {
      if (!key || !ctx.setConsoleKey(key)) return ctx.out("usage: console-key <one character>", "warn");
      ctx.out(`console key is now ${key}`, "sys");
    },
  },
  {
    name: "clear",
    usage: "clear",
    desc: "clear the console",
    group: "app",
    mcp: false,
    run: (_a, ctx) => ctx.clearConsole(),
  },
  { name: "god", usage: "god", desc: "", group: "app", mcp: false, hidden: true, run: (_a, ctx) => ctx.out("no.") },
];

export function findCommand(name: string): Command | undefined {
  return COMMANDS.find((c) => c.name === name);
}

/** Split a command line on whitespace, honoring "double" and 'single' quotes. */
export function tokenize(raw: string): string[] {
  const out: string[] = [];
  for (const m of raw.matchAll(/"([^"]*)"|'([^']*)'|(\S+)/g)) out.push(m[1] ?? m[2] ?? m[3]!);
  return out;
}

/** Echo and run one command line. Output only through ctx.out (text). */
export async function execute(raw: string, ctx: Ctx): Promise<void> {
  const line = raw.trim();
  if (!line) return;
  ctx.out(line, "cmd");
  const name = line.split(/\s+/, 1)[0]!;
  const rest = line.slice(name.length).trim();
  const cmd = findCommand(name);
  if (!cmd) return ctx.out(`unknown command: ${name}. try help`, "warn");
  try {
    await cmd.run(tokenize(rest), ctx, rest);
  } catch (e) {
    ctx.out(`${name}: ${(e as Error).message}`, "warn");
  }
}

/**
 * Tab completion. `line` is the completed input when there's exactly one
 * match (with a trailing space), otherwise null; `options` lists matches.
 */
export function complete(input: string): { line: string | null; options: string[] } {
  const parts = input.split(/\s+/);
  if (parts.length <= 1) {
    const options = COMMANDS.filter((c) => !c.hidden && c.name.startsWith(parts[0] ?? "")).map((c) => c.name);
    return { line: options.length === 1 ? `${options[0]} ` : null, options };
  }
  const cmd = findCommand(parts[0]!);
  const prefix = parts.at(-1)!;
  const options = (cmd?.complete?.(parts.slice(1)) ?? []).filter((o) => o.startsWith(prefix));
  return {
    line: options.length === 1 ? `${[...parts.slice(0, -1), options[0]].join(" ")} ` : null,
    options: [...options],
  };
}

export interface PaletteItem {
  command: string;
  group: Command["group"];
  label: string;
  hint: string;
  /** Set for commands that need arguments: open the console with this text. */
  prefill?: string;
}

export function paletteItems(): PaletteItem[] {
  return COMMANDS.filter((c) => c.palette).map((c) => ({
    command: c.name,
    group: c.group,
    label: c.palette!.label,
    hint: c.palette!.hint ?? c.name,
    prefill: c.palette!.needsArgs ? `${c.name} ` : undefined,
  }));
}
