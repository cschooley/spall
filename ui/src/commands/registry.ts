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
  /**
   * Replay an exchange through spall's gate (#18, #22), reporting through
   * out(). An unsafe method arms on the first call and sends on the second
   * (within a few seconds); `yes` does both at once.
   */
  replay(id: string, opts: { yes: boolean }): Promise<void>;
  /** The engagement scope; null when replay isn't available. */
  scope(): readonly string[] | null;
  /** The newest replay attempts from the audit log, or an error message. */
  audit(last: number): Promise<readonly Record<string, unknown>[] | string>;
}

/**
 * What running a command can do (#22), for MCP grants and approvals (M6):
 * read (the UI or spall's state), sends-traffic (to a target, through the
 * gate), unsafe-method (could change state there), destructive.
 */
export type Risk = "read" | "sends-traffic" | "unsafe-method" | "destructive";

export interface Command {
  /** Stable identifier, shared with MCP (M5). Never a UI label. */
  name: string;
  usage: string;
  desc: string;
  group: "traffic" | "view" | "app";
  /** May be offered to the agent over MCP. Never for export or promotion. */
  mcp: boolean;
  risk: Risk;
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
    risk: "read",
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
    risk: "read",
    palette: { label: "Filter traffic…", hint: "filter <query>", needsArgs: true },
    run: (_a, ctx, rest) => ctx.setFilter(rest),
  },
  {
    name: "open",
    usage: "open <id>",
    desc: "open an exchange in the inspector",
    group: "traffic",
    mcp: true,
    risk: "read",
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
    risk: "read",
    palette: { label: "Copy as curl", hint: "curl" },
    run: async (_a, ctx) => {
      const text = ctx.curlText();
      if (!text) return ctx.out("curl: select an exchange first", "warn");
      ctx.out(text);
      ctx.out((await ctx.copyCurl()) ? "copied to the clipboard" : "copy failed (clipboard unavailable)", "sys");
    },
  },
  {
    name: "replay",
    usage: "replay <id> [--yes]",
    desc: "resend an exchange as captured (unsafe methods: run twice, or --yes)",
    group: "traffic",
    // Agents replay inside task constraints from M6; the gate holds them to
    // the same scope, method policy and audit as a human.
    mcp: true,
    risk: "sends-traffic",
    palette: { label: "Replay exchange…", hint: "replay <id>", needsArgs: true },
    run: async (args, ctx) => {
      const flags = args.filter((a) => a.startsWith("-"));
      const ids = args.filter((a) => !a.startsWith("-"));
      const unknown = flags.find((f) => f !== "--yes");
      if (unknown) return ctx.out(`replay: unknown option ${unknown}`, "warn");
      if (ids.length > 1) return ctx.out("usage: replay <id> [--yes]", "warn");
      const id = ids[0] ?? ctx.selected()?.id;
      if (!id) return ctx.out("usage: replay <id> [--yes] (or select an exchange)", "warn");
      if (!ID.test(id)) return ctx.out(`replay: not an exchange id: ${id}`, "warn");
      await ctx.replay(id, { yes: flags.includes("--yes") });
    },
  },
  {
    name: "scope",
    usage: "scope",
    desc: "hosts replay may send to",
    group: "traffic",
    mcp: true,
    risk: "read",
    palette: { label: "Show scope", hint: "scope" },
    run: (_a, ctx) => {
      const scope = ctx.scope();
      if (scope === null) return ctx.out("scope: replay needs taphttp's lever API", "warn");
      if (scope.length === 0) return ctx.out("scope: no scope set, so every replay is denied (set SPALL_SCOPE)", "warn");
      for (const s of scope) ctx.out(`  ${s}`);
    },
  },
  {
    name: "audit",
    usage: "audit [--last N]",
    desc: "recent replay attempts, denials included",
    group: "traffic",
    mcp: false,
    risk: "read",
    palette: { label: "Audit log", hint: "audit" },
    run: async (args, ctx) => {
      let last = 20;
      const i = args.indexOf("--last");
      if (i >= 0) {
        const n = Number(args[i + 1]);
        if (!Number.isInteger(n) || n < 1) return ctx.out("usage: audit [--last N]", "warn");
        last = Math.min(n, 500);
      }
      const records = await ctx.audit(last);
      if (typeof records === "string") return ctx.out(`audit: ${records}`, "warn");
      if (records.length === 0) return ctx.out("audit: no replay attempts yet", "sys");
      for (const r of records) {
        const outcome = String(r.outcome ?? "?");
        ctx.out(auditLine(r), ["denied", "failed", "confirmation_refused", "error"].includes(outcome) ? "warn" : "out");
      }
    },
  },
  {
    name: "newest",
    usage: "newest",
    desc: "jump to the newest exchange and follow",
    group: "traffic",
    mcp: false,
    risk: "read",
    palette: { label: "Jump to newest", hint: "newest" },
    run: (_a, ctx) => ctx.jumpToNewest(),
  },
  {
    name: "clear-table",
    usage: "clear-table",
    desc: "hide what's in the table; new traffic still arrives",
    group: "traffic",
    mcp: false,
    risk: "read",
    palette: { label: "Clear table", hint: "clear-table" },
    run: (_a, ctx) => ctx.clearTable(),
  },
  {
    name: "goto",
    usage: "goto <view>",
    desc: VIEWS.join(" | "),
    group: "view",
    mcp: false,
    risk: "read",
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
    risk: "read",
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
    risk: "read",
    palette: { label: "Keyboard shortcuts", hint: "?" },
    run: (_a, ctx) => ctx.openKeys(),
  },
  {
    name: "console-key",
    usage: "console-key <char>",
    desc: "key that toggles this console (default `)",
    group: "app",
    mcp: false,
    risk: "read",
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
    risk: "read",
    run: (_a, ctx) => ctx.clearConsole(),
  },
  { name: "god", usage: "god", desc: "", group: "app", mcp: false, risk: "read", hidden: true, run: (_a, ctx) => ctx.out("no.") },
];

/** One audit record as a console line: time, outcome, method, target, why. */
function auditLine(r: Record<string, unknown>): string {
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const time = str(r.ts).match(/T(\d\d:\d\d:\d\d)/)?.[1] ?? "--:--:--";
  const parts = [time, str(r.outcome).padEnd(20), str(r.method).padEnd(7), str(r.target)];
  if (r.confirmed === true) parts.push("(confirmed)");
  const why = str(r.reason) || str(r.error);
  if (why && r.outcome !== "sent") parts.push(`· ${why}`);
  return parts.join(" ");
}

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
