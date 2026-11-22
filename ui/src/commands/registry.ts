// One command registry (docs/ux.md, "The console and quad mode"): the
// palette, the ` console and, from M5, the MCP server expose the same
// commands with the same names and arguments. Pure: commands act through a
// Ctx the app supplies, so this module is unit-tested without a browser.

import { THEMES, type ThemeName } from "../lib/theme.ts";
import type { TrafficEntry } from "../types.ts";
import { describeRules, parseDays, parseMethods, parseSpan, type Rules } from "../lib/roe.ts";

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
  replay(id: string, opts: { yes: boolean; edit?: ReplayEdit }): Promise<void>;
  /** The engagement scope; null when replay isn't available. */
  scope(): { entries: readonly string[]; all: boolean } | null;
  /** Change the scope for this run (in memory until restart; audited). */
  changeScope(change: ScopeChange): Promise<{ scope: readonly string[]; all: boolean; changed: boolean } | string>;
  /** The newest replay attempts from the audit log, or an error message. */
  audit(last: number): Promise<readonly Record<string, unknown>[] | string>;
  /** Open the Engagement view's paste-a-scope-list panel (#38). */
  openScopePaste(): void;
  /** Where a URL or host stands in scope (#34), or an error message. */
  checkScope(target: string): Promise<ScopeVerdict | string>;
  /** The active engagement (#35), or an error message. */
  engagement(): Promise<EngagementInfo | string>;
  listEngagements(): Promise<readonly EngagementSummary[] | string>;
  /** Create an engagement and make it active. */
  newEngagement(name: string): Promise<EngagementInfo | string>;
  /** Make an engagement active, by id or name. */
  useEngagement(which: string): Promise<EngagementInfo | string>;
  /** Set one detail on the active engagement; "" clears a date. */
  setEngagement(field: EngagementField, value: string): Promise<{ engagement: EngagementInfo; changed: boolean } | string>;
  /** Change the active engagement's rules of engagement (#36); `rate_limit: 0` is off. */
  setRules(patch: Partial<Rules>): Promise<{ engagement: EngagementInfo; changed: boolean } | string>;
}

export interface EngagementInfo {
  id: string;
  name: string;
  client: string;
  starts: string | null;
  ends: string | null;
  expires: string | null;
  scope: readonly string[];
  notes: string;
  /** A note per scope entry (`!entry` for an exclusion). */
  targetNotes: Record<string, string>;
  rules: Rules;
  /** Whether the rules allow testing right now, and why not. */
  testingNow: { allowed: boolean; reason: string | null } | null;
}

export interface EngagementSummary {
  id: string;
  name: string;
  client: string;
  active: boolean;
}

export const ENGAGEMENT_FIELDS = ["name", "client", "starts", "ends", "expires", "notes"] as const;
export type EngagementField = (typeof ENGAGEMENT_FIELDS)[number];
const ROE_USAGE =
  "usage: roe [set <methods|safe|rate|tz|agents|destructive> <value> | window add <days> <HH:MM-HH:MM> | window remove <n> | window clear | blackout add <from> <to> [note] | blackout remove <n> | blackout clear]";
const ROE_FIELDS = ["methods", "safe", "rate", "tz", "agents", "destructive"];
const LOCAL = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/;

/** A `roe set` value as a rules change, or a message. */
function roeSet(field: string, value: string): Partial<Rules> | string {
  const yesNo = (v: string) => (/^(yes|on|true)$/i.test(v) ? true : /^(no|off|false)$/i.test(v) ? false : null);
  switch (field) {
    case "methods":
    case "safe": {
      const list = parseMethods(value);
      if (list === null) return "methods: comma-separated names like GET,POST (or any)";
      return field === "methods" ? { methods: list } : { safe_methods: list };
    }
    case "rate": {
      if (/^(off|none|0)$/i.test(value)) return { rate_limit: 0 };
      const n = Number(value);
      return Number.isFinite(n) && n > 0 ? { rate_limit: n } : "rate: sends per second, like 5 or 0.5 (or off)";
    }
    case "tz":
      return value ? { time_zone: value } : "tz: an IANA zone like Europe/Berlin";
    case "agents":
    case "destructive": {
      const b = yesNo(value);
      if (b === null) return `${field}: yes or no`;
      return field === "agents" ? { agents_allowed: b } : { destructive_allowed: b };
    }
    default:
      return `fields: ${ROE_FIELDS.join(", ")}`;
  }
}

const ENGAGEMENT_USAGE = "usage: engagement [list | new <name> | use <id|name> | set <field> <value>]";

export type ScopeChange = { op: "add" | "exclude" | "remove"; entry: string } | { op: "all"; on: boolean };

/** The server's `Scope::explain`: `by` is the deciding entry. */
export interface ScopeVerdict {
  host: string;
  state: "in" | "out" | "unknown";
  by: string | null;
}

const SCOPE_USAGE = "usage: scope [add|exclude|remove <entry> | check <url|host> | paste | all on|off]";

/** Console edits for `replay` (#19), applied to the captured request. */
export interface ReplayEdit {
  method?: string;
  /** `--header 'Name: value'`: replaces every value for the name. */
  sets: [string, string][];
  /** `--drop-header Name`: removes every value for the name. */
  drops: string[];
  body?: string;
}

const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** Parse replay's arguments; a string is a usage error. */
function parseReplayArgs(args: string[]): { id?: string; yes: boolean; edit?: ReplayEdit } | string {
  let id: string | undefined;
  let yes = false;
  const edit: ReplayEdit = { sets: [], drops: [] };
  let edited = false;
  for (let i = 0; i < args.length; i++) {
    const a = args[i]!;
    const value = () => {
      const v = args[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    try {
      if (a === "--yes") yes = true;
      else if (a === "--method") {
        const m = value();
        if (!TOKEN.test(m)) return `replay: invalid method ${m}`;
        edit.method = m;
        edited = true;
      } else if (a === "--header") {
        const h = value();
        const colon = h.indexOf(":");
        const name = colon < 0 ? "" : h.slice(0, colon).trim();
        if (!TOKEN.test(name)) return `replay: --header wants 'Name: value', got ${h}`;
        edit.sets.push([name, h.slice(colon + 1).trim()]);
        edited = true;
      } else if (a === "--drop-header") {
        const name = value();
        if (!TOKEN.test(name)) return `replay: invalid header name ${name}`;
        edit.drops.push(name);
        edited = true;
      } else if (a === "--body") {
        edit.body = value();
        edited = true;
      } else if (a.startsWith("-")) return `replay: unknown option ${a}`;
      else if (id === undefined) id = a;
      else return "usage: replay <id> [--method M] [--header 'N: v'] [--drop-header N] [--body B] [--yes]";
    } catch (e) {
      return `replay: ${(e as Error).message}`;
    }
  }
  return { id, yes, edit: edited ? edit : undefined };
}

/**
 * What running a command can do (#22), for MCP grants and approvals (M6):
 * read (the UI or spall's state), sends-traffic (to a target, through the
 * gate), unsafe-method (could change state there), destructive, policy
 * (changes the rules of engagement: never offered to an agent).
 */
export type Risk = "read" | "sends-traffic" | "unsafe-method" | "destructive" | "policy";

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
    desc: "resend an exchange (edits: --method M --header 'N: v' --drop-header N --body B; unsafe methods: run twice, or --yes)",
    group: "traffic",
    // Agents replay inside task constraints from M6; the gate holds them to
    // the same scope, method policy and audit as a human.
    mcp: true,
    risk: "sends-traffic",
    palette: { label: "Replay exchange…", hint: "replay <id>", needsArgs: true },
    run: async (args, ctx) => {
      const parsed = parseReplayArgs(args);
      if (typeof parsed === "string") return ctx.out(parsed, "warn");
      const id = parsed.id ?? ctx.selected()?.id;
      if (!id) return ctx.out("usage: replay <id> [--yes] (or select an exchange)", "warn");
      if (!ID.test(id)) return ctx.out(`replay: not an exchange id: ${id}`, "warn");
      await ctx.replay(id, parsed.edit ? { yes: parsed.yes, edit: parsed.edit } : { yes: parsed.yes });
    },
  },
  {
    name: "scope",
    usage: "scope [add|exclude|remove <entry> | check <url|host> | paste | all on|off]",
    desc: "hosts replay may send to; exclusions (!entry) win; changes last until restart",
    group: "traffic",
    // Changing the scope changes the rules of engagement: humans only.
    // (Agents will read scope through a separate read-only tool, M6.)
    mcp: false,
    risk: "policy",
    palette: { label: "Show scope", hint: "scope" },
    complete: (args) => (args.length <= 1 ? ["add", "exclude", "remove", "check", "paste", "all"] : args[0] === "all" ? ["on", "off"] : []),
    run: async (args, ctx) => {
      const show = (entries: readonly string[], all: boolean) => {
        const listed = entries.filter((e) => e !== "*" && !e.startsWith("!"));
        const excluded = entries.filter((e) => e.startsWith("!")).map((e) => e.slice(1));
        if (all) ctx.out("scope: every host, third parties included (scope all off to go back)", "warn");
        else if (listed.length === 0) ctx.out("scope: no scope set, so every replay is denied (scope add <host>, or SPALL_SCOPE)", "warn");
        if (listed.length) ctx.out(`scope: ${listed.join(", ")}`);
        if (excluded.length) ctx.out(`excluded: ${excluded.join(", ")} (wins over the scope)`);
      };
      if (args.length === 0) {
        const scope = ctx.scope();
        if (scope === null) return ctx.out("scope: replay needs taphttp's lever API", "warn");
        return show(scope.entries, scope.all);
      }
      const [sub, arg, ...extra] = args;
      if (sub === "paste" && !arg) {
        ctx.openScopePaste();
        return ctx.out("scope: paste a list in the Engagement view; it previews every line before anything changes", "sys");
      }
      if (sub === "check") {
        if (!arg || extra.length) return ctx.out("usage: scope check <url|host>", "warn");
        const v = await ctx.checkScope(arg);
        if (typeof v === "string") return ctx.out(`scope check: ${v}`, "warn");
        if (v.state === "in") return ctx.out(`${v.host}: in scope (by ${v.by ?? "?"})`);
        if (v.state === "out") return ctx.out(`${v.host}: out of scope (excluded by ${v.by ?? "?"})`, "warn");
        return ctx.out(`${v.host}: not in scope (no entry matches; scope add ${v.host}, or scope exclude ${v.host})`, "warn");
      }
      let change: ScopeChange;
      if ((sub === "add" || sub === "exclude" || sub === "remove") && arg && extra.length === 0) change = { op: sub, entry: arg };
      else if (sub === "all" && (arg === "on" || arg === "off") && extra.length === 0) change = { op: "all", on: arg === "on" };
      else return ctx.out(SCOPE_USAGE, "warn");
      const res = await ctx.changeScope(change);
      if (typeof res === "string") return ctx.out(`scope: ${res}`, "warn");
      if (!res.changed) ctx.out("scope: no change", "sys");
      show(res.scope, res.all);
    },
  },
  {
    name: "engagement",
    usage: "engagement [list | new <name> | use <id|name> | set <field> <value>]",
    desc: "the engagement you're testing under: details, and switching between engagements",
    group: "traffic",
    // Creating, switching and editing engagements set the rules: humans only.
    mcp: false,
    risk: "policy",
    palette: { label: "Show engagement", hint: "engagement" },
    complete: (args) =>
      args.length <= 1 ? ["list", "new", "use", "set"] : args[0] === "set" && args.length === 2 ? [...ENGAGEMENT_FIELDS] : [],
    run: async (args, ctx) => {
      const [sub, ...rest] = args;
      const warn = (msg: string) => ctx.out(`engagement: ${msg}`, "warn");
      if (sub === undefined) {
        const e = await ctx.engagement();
        if (typeof e === "string") return warn(e);
        ctx.out(`engagement: ${e.name} (${e.id})${e.client ? ` · client ${e.client}` : ""}`);
        const dates = e.starts || e.ends ? `${e.starts ?? "?"} → ${e.ends ?? "?"}` : "not set";
        ctx.out(`dates: ${dates} · authorization ${e.expires ? `expires ${e.expires}` : "expiry not set"}`);
        const excluded = e.scope.filter((x) => x.startsWith("!")).length;
        const n = e.scope.length - excluded;
        ctx.out(`scope: ${n} entr${n === 1 ? "y" : "ies"}${excluded ? `, ${excluded} excluded` : ""} (scope lists them)`);
        return;
      }
      if (sub === "list" && rest.length === 0) {
        const list = await ctx.listEngagements();
        if (typeof list === "string") return warn(list);
        for (const e of list) ctx.out(`${e.active ? "*" : " "} ${e.name} (${e.id})${e.client ? ` · ${e.client}` : ""}`);
        return;
      }
      if (sub === "new" && rest.length > 0) {
        const e = await ctx.newEngagement(rest.join(" "));
        if (typeof e === "string") return warn(e);
        ctx.out(`engagement: created ${e.name} (${e.id}), now active${e.scope.length ? "" : "; its scope is empty: scope add <host>"}`);
        return;
      }
      if (sub === "use" && rest.length > 0) {
        const e = await ctx.useEngagement(rest.join(" "));
        if (typeof e === "string") return warn(e);
        ctx.out(`engagement: now ${e.name} (${e.id})`);
        return;
      }
      if (sub === "set" && rest.length >= 2) {
        const [field, ...value] = rest;
        if (!(ENGAGEMENT_FIELDS as readonly string[]).includes(field!)) return warn(`fields: ${ENGAGEMENT_FIELDS.join(", ")}`);
        const f = field as EngagementField;
        const res = await ctx.setEngagement(f, value.join(" "));
        if (typeof res === "string") return warn(res);
        const now = res.engagement[f];
        if (!res.changed) ctx.out("engagement: no change", "sys");
        else ctx.out(`engagement: ${f} ${now ? `is now ${now}` : "cleared"}`);
        return;
      }
      ctx.out(ENGAGEMENT_USAGE, "warn");
    },
  },
  {
    name: "roe",
    usage: "roe [set <field> <value> | window add|remove|clear | blackout add|remove|clear]",
    desc: "rules of engagement: methods, confirmation, rate, time zone, testing and no-test windows, agents",
    group: "traffic",
    // The rules bound every action, a human's or an agent's: humans only.
    mcp: false,
    risk: "policy",
    palette: { label: "Rules of engagement", hint: "roe" },
    complete: (args) =>
      args.length <= 1
        ? ["set", "window", "blackout"]
        : args[0] === "set" && args.length === 2
          ? ROE_FIELDS
          : (args[0] === "window" || args[0] === "blackout") && args.length === 2
            ? ["add", "remove", "clear"]
            : [],
    run: async (args, ctx) => {
      const warn = (msg: string) => ctx.out(`roe: ${msg}`, "warn");
      const show = (e: EngagementInfo) => {
        for (const line of describeRules(e.rules)) ctx.out(line);
        const now = e.testingNow;
        if (now) ctx.out(now.allowed ? "testing now: allowed" : `testing now: not allowed: ${now.reason ?? "?"}`, now.allowed ? "out" : "warn");
      };
      const apply = async (patch: Partial<Rules>) => {
        const res = await ctx.setRules(patch);
        if (typeof res === "string") return warn(res);
        if (!res.changed) ctx.out("roe: no change", "sys");
        show(res.engagement);
      };
      const [sub, ...rest] = args;
      if (sub === undefined) {
        const e = await ctx.engagement();
        return typeof e === "string" ? warn(e) : show(e);
      }
      if (sub === "set" && rest.length >= 2) {
        const [field, ...value] = rest;
        const patch = roeSet(field!, value.join(" ").trim());
        return typeof patch === "string" ? warn(patch) : apply(patch);
      }
      if ((sub === "window" || sub === "blackout") && rest.length >= 1) {
        const [op, ...more] = rest;
        const key = sub === "window" ? "windows" : "blackouts";
        if (op === "clear" && more.length === 0) return apply({ [key]: [] });
        if (op !== "add" && op !== "remove") return ctx.out(ROE_USAGE, "warn");
        const e = await ctx.engagement();
        if (typeof e === "string") return warn(e);
        if (op === "remove") {
          const n = Number(more[0]);
          const list = e.rules[key];
          if (more.length !== 1 || !Number.isInteger(n) || n < 1 || n > list.length) return warn(`${sub} remove: a number from 1 to ${list.length} (roe lists them)`);
          return apply({ [key]: list.filter((_, i) => i !== n - 1) });
        }
        if (sub === "window") {
          const days = more[0] ? parseDays(more[0]) : null;
          const span = more[1] ? parseSpan(more[1]) : null;
          if (more.length !== 2 || !days || !span) return warn("window add <days: mon-fri, sat,sun, daily> <HH:MM-HH:MM>");
          return apply({ windows: [...e.rules.windows, { days, ...span }] });
        }
        const [from, to, ...note] = more;
        if (!from || !to || !LOCAL.test(from) || !LOCAL.test(to)) return warn("blackout add <YYYY-MM-DDTHH:MM> <YYYY-MM-DDTHH:MM> [note]");
        return apply({ blackouts: [...e.rules.blackouts, { from, to, note: note.join(" ") }] });
      }
      ctx.out(ROE_USAGE, "warn");
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
