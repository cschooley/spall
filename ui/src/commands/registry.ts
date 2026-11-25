// One command registry (docs/ux.md, "The console and quad mode"): the
// palette, the ` console and the MCP server expose the same commands with
// the same names and arguments. Names, usage, risk, `mcp` and argument
// schemas come from the server's registry (src/commands.rs, generated into
// specs.gen.ts, #53); this module adds how each runs in the browser.
// Pure: commands act through a Ctx the app supplies, so this module is
// unit-tested without a browser.

import { THEMES, type ThemeName } from "../lib/theme.ts";
import type { TrafficEntry } from "../types.ts";
import type { HostListing } from "../lib/hosts.ts";
import type { FetchBody } from "../lib/replay.ts";
import { ruleLine, searchRules, type Catalog } from "../lib/rules.ts";
import { parseFlagArgs, stateOf, where, type Candidate, type FlagInput, type FlagResult } from "../lib/candidates.ts";
import type { CaInfo } from "../lib/ca.ts";
import type { ApiSpec } from "../lib/engagement.ts";
import { describeRules, parseDays, parseMethods, parseSpan, type Rules } from "../lib/roe.ts";
import { RISKS, SPECS } from "./specs.gen.ts";
import { CATEGORIES, draftedLine, VERBS, type Drafted } from "../lib/verbs.ts";
import { sweepLines, type CheckInfo, type SweepAnswer } from "../lib/sweep.ts";
import { hitLine, type OobPayload, type OobState } from "../lib/oob.ts";

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
  /** A new GET or HEAD through the same gate (#74), reporting through out(); `yes` confirms if the rules ask. */
  fetchUrl(body: FetchBody, yes: boolean): Promise<void>;
  /** The engagement scope; null when replay isn't available. */
  scope(): { entries: readonly string[]; all: boolean } | null;
  /** Change the scope for this run (in memory until restart; audited). */
  changeScope(change: ScopeChange): Promise<{ scope: readonly string[]; all: boolean; changed: boolean } | string>;
  /** The newest replay attempts from the audit log, or an error message. */
  audit(last: number): Promise<readonly Record<string, unknown>[] | string>;
  /** The CA's public details (#32), or an error message. */
  ca(): Promise<CaInfo | string>;
  /** Open the Engagement view's browser setup (#32). */
  openSetup(): void;
  /** Hosts seen in traffic with their scope state (#39), or an error message. */
  hosts(): Promise<HostListing | string>;
  /** Show a view; false if it isn't built yet. */
  goto(view: string): boolean;
  /** Promote a candidate for export, or undo it (#50). */
  promote(id: string, undo: boolean): Promise<Candidate | string>;
  /** Export the promoted candidates as a flux report and save it (#51); what happened. */
  exportFindings(unredacted: boolean): Promise<{ ok: boolean; text: string }>;
  /** Halt a task for good (#61); what happened. */
  haltTask(id: string, reason: string): Promise<{ ok: boolean; text: string }>;
  /** A task's details as console lines, or every task's one-liner with no id (#54); or why not. */
  task(id?: string): Promise<string[] | string>;
  /** OOB callbacks (#1): the state, or why not. */
  oob(): Promise<OobState | string>;
  /** Mint an OOB payload (#1), or why not. */
  oobMint(): Promise<OobPayload | string>;
  /** A sweep's estimate (`yes` false) or its run (#75), or why not. */
  sweep(base: string, check: string, yes: boolean): Promise<SweepAnswer | string>;
  /** The checks a sweep can run (#75), or why not. */
  checks(): Promise<CheckInfo[] | string>;
  /** Start quad mode, or end it (#66); what happened, as lines. */
  quad(what: { seconds?: number } | { stop: true }): Promise<{ ok: boolean; lines: string[] }>;
  /** The never-test list (#84), changed when `op` is given; or why not. */
  neverTest(op?: "add" | "remove", entry?: string): Promise<{ entries: string[]; exceptions: string[] } | string>;
  /** Engage the agent e-stop, or clear it (#57); what happened. */
  estop(action: "engage" | "clear", reason?: string): Promise<{ ok: boolean; text: string }>;
  /** Flag an exchange as a candidate (#43). */
  flag(i: FlagInput): Promise<FlagResult>;
  /** The rule catalog (#41), or why not. */
  rules(): Promise<Catalog | string>;
  /** Open the Engagement view's paste-a-scope-list panel (#38), or the OpenAPI import (#40). */
  openScopePaste(what?: "paste" | "import"): void;
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
  /** Draft verb `name`'s task for object `id` (#67), or why not. */
  verb(name: string, id: string, category?: string): Promise<Drafted | string>;
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
  /** Imported API descriptions (#40). */
  apiSpecs: ApiSpec[];
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
  "usage: roe [set <methods|safe|rate|tz|agents|destructive|looks-destructive|sweep> <value> | window add <days> <HH:MM-HH:MM> | window remove <n> | window clear | blackout add <from> <to> [note] | blackout remove <n> | blackout clear]";
const ROE_FIELDS = ["methods", "safe", "rate", "tz", "agents", "destructive", "looks-destructive", "sweep"];
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
    case "sweep": {
      // #75: requests one sweep may send; 0 forbids sweeps.
      const n = Number(value);
      return Number.isInteger(n) && n >= 0 && n <= 5000 ? { sweep_max: n } : "sweep: 0 (no sweeps) to 5000 requests";
    }
    case "looks-destructive":
      // #85: a request that looks destructive asks first, or is refused.
      return /^ask$/i.test(value) ? { refuse_destructive_looking: false } : /^refuse$/i.test(value) ? { refuse_destructive_looking: true } : "looks-destructive: ask or refuse";
    default:
      return `fields: ${ROE_FIELDS.join(", ")}`;
  }
}

const ENGAGEMENT_USAGE = "usage: engagement [list | new <name> | use <id|name> | set <field> <value>]";

export type ScopeChange = { op: "add"; entry: string; wide?: boolean } | { op: "exclude" | "remove"; entry: string } | { op: "all"; on: boolean };

/** The server's `Scope::explain`: `by` is the deciding entry. */
export interface ScopeVerdict {
  host: string;
  state: "in" | "out" | "unknown";
  by: string | null;
}

const SCOPE_USAGE = "usage: scope [add <entry> [--wide] | exclude|remove <entry> | check <url|host> | paste | import | all on|off]";

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

/** What running a command can do: the server's risk classes (src/commands.rs). */
export type Risk = (typeof RISKS)[number];

/**
 * A command as the server's registry describes it (#53): stable name
 * (shared with MCP), console usage, description, risk class, whether an
 * agent may be offered it (`mcp`; the server refuses policy and export to
 * agents whatever this says), and its arguments as JSON Schema.
 */
export type Spec = (typeof SPECS)[number];

/** What only the UI knows about a command: where it sits and how it runs. */
interface Impl {
  /** A name from the server's registry. */
  name: string;
  group: "traffic" | "view" | "app";
  /** Palette entry; commands that need arguments open the console prefilled. */
  palette?: { label: string; hint?: string; needsArgs?: boolean };
  /** Completions for the argument being typed. */
  complete?: (args: string[]) => readonly string[];
  /** `rest` is the raw text after the name, for commands that keep spacing. */
  run(args: string[], ctx: Ctx, rest: string): void | Promise<void>;
}

export interface Command extends Impl {
  usage: string;
  desc: string;
  mcp: boolean;
  risk: Risk;
  hidden: boolean;
  args: Spec["args"];
}

const ID = /^[A-Za-z0-9_-]{1,128}$/;

const FETCH_USAGE = "usage: fetch <url> [--head] [--header 'N: v'] [--yes]";

/** Parse fetch's arguments (#74); a string is a usage error. */
function parseFetchArgs(args: string[]): { body: FetchBody; yes: boolean } | string {
  let url: string | undefined;
  let yes = false;
  let head = false;
  const headers: [string, string][] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i]!;
    if (a === "--yes") yes = true;
    else if (a === "--head") head = true;
    else if (a === "--header") {
      const h = args[++i];
      if (h === undefined) return "fetch: --header needs a value";
      const colon = h.indexOf(":");
      const name = colon < 0 ? "" : h.slice(0, colon).trim();
      if (!TOKEN.test(name)) return `fetch: --header wants 'Name: value', got ${h}`;
      headers.push([name, h.slice(colon + 1).trim()]);
    } else if (a.startsWith("-")) return `fetch: unknown option ${a} (GET, or HEAD with --head; other methods are replay's)`;
    else if (url === undefined) url = a;
    else return FETCH_USAGE;
  }
  if (url === undefined) return FETCH_USAGE;
  if (!/^https?:\/\/[^/\s]/i.test(url)) return `fetch: ${url} isn't an absolute http or https URL`;
  const body: FetchBody = { url };
  if (head) body.method = "HEAD";
  if (headers.length > 0) body.req_headers = headers;
  return { body, yes };
}
export const VIEWS = ["engagement", "traffic", "surface", "tasks", "findings"] as const;

const IMPLS: readonly Impl[] = [
  {
    name: "help",
    group: "app",
    run: (_a, ctx) => {
      for (const c of COMMANDS) if (!c.hidden) ctx.out(`  ${c.usage.padEnd(24)} ${c.desc}`);
    },
  },
  {
    name: "filter",
    group: "traffic",
    palette: { label: "Filter traffic…", hint: "filter <query>", needsArgs: true },
    run: (_a, ctx, rest) => ctx.setFilter(rest),
  },
  {
    name: "open",
    group: "traffic",
    palette: { label: "Open exchange by id…", hint: "open <id>", needsArgs: true },
    run: async ([id], ctx) => {
      if (!id) return ctx.out("usage: open <id>", "warn");
      if (!ID.test(id)) return ctx.out(`open: not an exchange id: ${id}`, "warn");
      if (!(await ctx.openById(id))) ctx.out(`open: no exchange ${id}`, "warn");
    },
  },
  {
    name: "curl",
    group: "traffic",
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
    group: "traffic",
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
    name: "fetch",
    group: "traffic",
    palette: { label: "Fetch URL…", hint: "fetch <url>", needsArgs: true },
    run: async (args, ctx) => {
      const parsed = parseFetchArgs(args);
      if (typeof parsed === "string") return ctx.out(parsed, "warn");
      await ctx.fetchUrl(parsed.body, parsed.yes);
    },
  },
  {
    name: "scope",
    group: "traffic",
    palette: { label: "Show scope", hint: "scope" },
    complete: (args) => (args.length <= 1 ? ["add", "exclude", "remove", "check", "paste", "import", "all"] : args[0] === "all" ? ["on", "off"] : []),
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
        ctx.openScopePaste("paste");
        return ctx.out("scope: paste a list in the Engagement view; it previews every line before anything changes", "sys");
      }
      if (sub === "import" && !arg) {
        ctx.openScopePaste("import");
        return ctx.out("scope: choose an OpenAPI or Swagger file in the Engagement view; its servers are previewed before anything changes", "sys");
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
      if (sub === "add" && arg && extra.length === 1 && extra[0] === "--wide") change = { op: "add", entry: arg, wide: true };
      else if ((sub === "add" || sub === "exclude" || sub === "remove") && arg && extra.length === 0) change = { op: sub, entry: arg };
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
    group: "traffic",
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
    group: "traffic",
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
    name: "estop",
    group: "app",
    palette: { label: "E-stop: halt every agent task", hint: "estop" },
    run: async (args, ctx, rest) => {
      if (args[0] === "clear") {
        if (args.length > 1) return ctx.out("usage: estop [reason] | estop clear", "warn");
        const r = await ctx.estop("clear");
        return ctx.out(r.text, r.ok ? "out" : "warn");
      }
      const r = await ctx.estop("engage", rest);
      ctx.out(r.text, r.ok ? "stop" : "warn");
    },
  },
  {
    name: "never-test",
    group: "app",
    palette: { label: "Never-test list (every engagement)", hint: "never-test" },
    run: async (args, ctx) => {
      const [op, entry, ...extra] = args;
      if (extra.length || (op !== undefined && (!(op === "add" || op === "remove") || !entry))) return ctx.out("usage: never-test [add|remove <entry>]", "warn");
      const r = await ctx.neverTest(op as "add" | "remove" | undefined, entry);
      if (typeof r === "string") return ctx.out(`never-test: ${r}`, "warn");
      if (!r.entries.length) return ctx.out("never-test: empty (never-test add <entry>; *.gov and *.mil are offered in the Engagement view)");
      ctx.out(`never-test: ${r.entries.join(", ")}`);
      if (r.exceptions.length) ctx.out(`this engagement is exempted from: ${r.exceptions.join(", ")}`, "sys");
    },
  },
  {
    name: "sweep",
    group: "traffic",
    palette: { label: "Sweep a base URL with a check…", hint: "sweep <base-url> <check> [--yes]", needsArgs: true },
    complete: (args) => (args.length === 2 ? ["metafiles", "backup-files", "admin-interfaces"].filter((c) => c.startsWith(args[1]!)) : []),
    run: async (args, ctx) => {
      const yes = args.includes("--yes");
      const rest = args.filter((a) => a !== "--yes");
      if (rest.length !== 2 || !/^https?:\/\//i.test(rest[0]!)) return ctx.out("usage: sweep <base-url> <check> [--yes]  (checks lists them)", "warn");
      const [base, check] = rest as [string, string];
      if (yes) ctx.out(`sweep: running ${check} on ${base}… (every request goes through the gate)`, "sys");
      const r = await ctx.sweep(base, check, yes);
      if (typeof r === "string") return ctx.out(`sweep: ${r}`, "warn");
      if (r.kind === "estimate") return ctx.out(`sweep ${r.check} (${r.rule}) on ${base}: ${r.cost}. sweep ${base} ${check} --yes to run it`, "sys");
      sweepLines(r).forEach((l, i) => ctx.out(l, i === 0 ? "sys" : "out"));
    },
  },
  {
    name: "oob",
    group: "traffic",
    palette: { label: "Out-of-band callbacks", hint: "oob [new]" },
    run: async (args, ctx) => {
      if (args.length > 1 || (args[0] !== undefined && args[0] !== "new")) return ctx.out("usage: oob [new]", "warn");
      if (args[0] === "new") {
        const p = await ctx.oobMint();
        if (typeof p === "string") return ctx.out(`oob: ${p}`, "warn");
        ctx.out(`oob: ${p.host}  (${p.url}${p.labUrl ? `; in the lab: ${p.labUrl}` : ""}). Nothing was sent: put it in a request.`, "sys");
        return;
      }
      const s = await ctx.oob();
      if (typeof s === "string") return ctx.out(`oob: ${s}`, "warn");
      if (!s.configured) return ctx.out("oob: no OOB server is configured (SPALL_OAST_SERVER and SPALL_OAST_DOMAIN; the compose oob profile runs one)", "warn");
      ctx.out(`oob: ${s.payloads.length} payload(s) under ${s.domain}, ${s.hits.length} hit(s)`, "sys");
      for (const h of s.hits.slice(0, 20)) ctx.out(`  ${hitLine(h)}`);
    },
  },
  {
    name: "checks",
    group: "traffic",
    palette: { label: "Checks for sweep", hint: "checks" },
    run: async (_args, ctx) => {
      const r = await ctx.checks();
      if (typeof r === "string") return ctx.out(`checks: ${r}`, "warn");
      for (const c of r) ctx.out(`  ${c.name.padEnd(18)} ${c.rule.padEnd(13)} ${String(c.paths).padStart(3)} ${c.method} paths  ${c.source === "engagement" ? "(this engagement) " : ""}${c.title}`);
      ctx.out(`${r.length} checks · sweep <base-url> <check> to run one; import your own in the Engagement view`, "sys");
    },
  },
  {
    name: "quad",
    group: "app",
    palette: { label: "Quad: 4x agents for 2 minutes", hint: "quad [seconds]" },
    run: async (args, ctx) => {
      const usage = "usage: quad [seconds, 1-600] | quad stop";
      if (args.length > 1) return ctx.out(usage, "warn");
      const [a] = args;
      let what: { seconds?: number } | { stop: true } = {};
      if (a === "stop") what = { stop: true };
      else if (a !== undefined) {
        if (!/^\d{1,3}$/.test(a) || Number(a) < 1 || Number(a) > 600) return ctx.out(usage, "warn");
        what = { seconds: Number(a) };
      }
      const r = await ctx.quad(what);
      r.lines.forEach((l, i) => ctx.out(l, r.ok ? (i === 0 ? "sys" : "out") : "warn"));
    },
  },
  {
    name: "hosts",
    group: "traffic",
    palette: { label: "Hosts from traffic", hint: "hosts" },
    complete: () => ["--unknown"],
    run: async (args, ctx) => {
      const unknownOnly = args[0] === "--unknown";
      if (args.length > 1 || (args.length === 1 && !unknownOnly)) return ctx.out("usage: hosts [--unknown]", "warn");
      const l = await ctx.hosts();
      if (typeof l === "string") return ctx.out(`hosts: ${l}`, "warn");
      ctx.out(`hosts: ${l.counts.in} in scope · ${l.counts.out} excluded · ${l.counts.unknown} undecided`);
      const label = { in: "in", out: "out", unknown: "?" } as const;
      for (const h of l.hosts) {
        if (unknownOnly && h.state !== "unknown") continue;
        ctx.out(`  ${label[h.state].padEnd(4)}${String(h.count).padStart(8)}  ${h.host}${h.by ? `  (${h.by})` : ""}`, h.state === "unknown" ? "warn" : "out");
      }
      if (unknownOnly && l.counts.unknown) ctx.out("decide them in the Engagement view, or: scope add|exclude <host>", "sys");
      if (l.dropped) ctx.out(`${l.dropped} more hosts not tracked (table full)`, "sys");
    },
  },
  {
    name: "flag",
    group: "traffic",
    palette: { label: "Flag exchange as candidate…", hint: "flag", needsArgs: true },
    complete: (args) => (args.length >= 2 ? ["--rule", "--param", "--level", "--rank", "--message", "--replay", "--cwe", "--anyway"] : []),
    run: async (args, ctx) => {
      const i = parseFlagArgs(args);
      if (typeof i === "string") return ctx.out(i, "warn");
      const r = await ctx.flag(i);
      if (r.kind === "error") return ctx.out(`flag: ${r.message}`, "warn");
      const c = r.candidate;
      if (r.kind === "created") {
        ctx.out(`flagged ${c.id}: ${c.ruleId} · ${where(c)}`);
        ctx.out("waiting for triage in Findings (5)", "sys");
        return;
      }
      if (r.similar) {
        ctx.out(`flag: ${r.message}`, "warn");
        ctx.out(`flag it as its own with --anyway, or add this exchange to ${c.id}'s evidence from the inspector (f)`, "sys");
        return;
      }
      ctx.out(`flag: already flagged as ${c.id} (${c.ruleId}, ${stateOf(c)})${c.suppressed ? `: ${c.suppressed.justification}` : ""}`, "warn");
      ctx.out(c.suppressed ? "reopen it in Findings if that decision no longer holds" : "add this exchange to its evidence from the inspector (f)", "sys");
    },
  },
  {
    name: "promote",
    group: "app",
    run: async (args, ctx) => {
      const [id, flag] = args;
      if (!id || id.startsWith("--") || (flag && flag !== "--undo") || args.length > 2) return ctx.out("usage: promote <candidate-id> [--undo]", "warn");
      const c = await ctx.promote(id, flag === "--undo");
      if (typeof c === "string") return ctx.out(`promote: ${c}`, "warn");
      ctx.out(c.promoted ? `${c.id} promoted: in the next export` : `${c.id} is no longer promoted`);
    },
  },
  {
    name: "export",
    group: "app",
    palette: { label: "Export promoted findings (flux, redacted)", hint: "export" },
    complete: () => ["--unredacted"],
    run: async (args, ctx) => {
      if (args.length > 1 || (args[0] && args[0] !== "--unredacted")) return ctx.out("usage: export [--unredacted]", "warn");
      const r = await ctx.exportFindings(args[0] === "--unredacted");
      ctx.out(r.ok ? r.text : `export: ${r.text}`, r.ok ? "out" : "warn");
    },
  },
  {
    name: "rules",
    group: "app",
    palette: { label: "Rule catalog (WSTG)", hint: "rules" },
    run: async (args, ctx) => {
      const c = await ctx.rules();
      if (typeof c === "string") return ctx.out(`rules: ${c}`, "warn");
      const text = args.join(" ");
      const hits = searchRules(c.rules, text);
      if (!hits.length) return ctx.out(`rules: nothing matches "${text}"`, "warn");
      for (const r of hits) ctx.out(`  ${ruleLine(r)}`);
      ctx.out(`${hits.length} of ${c.rules.length} rules · ${c.attribution}`, "sys");
    },
  },
  {
    name: "task",
    group: "app",
    palette: { label: "Tasks", hint: "task [id]" },
    run: async (args, ctx) => {
      const [id, ...extra] = args;
      if (extra.length || (id !== undefined && !/^T-\d{1,9}$/.test(id))) return ctx.out("usage: task [T-n]", "warn");
      const r = await ctx.task(id);
      if (typeof r === "string") return ctx.out(`task: ${r}`, "warn");
      for (const line of r) ctx.out(line);
    },
  },
  {
    name: "halt",
    group: "app",
    palette: { label: "Halt a task…", hint: "halt <T-n>", needsArgs: true },
    run: async (args, ctx, rest) => {
      const [id] = args;
      if (!id || !/^T-\d{1,9}$/.test(id)) return ctx.out("usage: halt <T-n> [reason]", "warn");
      const r = await ctx.haltTask(id, rest.slice(id.length).trim());
      ctx.out(r.text, r.ok ? "stop" : "warn");
    },
  },
  {
    name: "ca",
    group: "traffic",
    palette: { label: "Set up a browser (CA certificate)", hint: "ca" },
    run: async (_args, ctx) => {
      const ca = await ctx.ca();
      if (typeof ca === "string") return ctx.out(`ca: ${ca}`, "warn");
      ctx.out(`ca: ${ca.subject ?? "taphttp CA"}${ca.notAfter ? `, valid until ${ca.notAfter}` : ""}`);
      ctx.out(`sha256: ${ca.sha256 ?? "unknown"}`);
      ctx.out("download: /api/ca.pem (or /api/ca.der); devices on the proxy: http://taphttp/");
      ctx.openSetup();
      ctx.out("browser setup steps are in the Engagement view", "sys");
    },
  },
  {
    name: "audit",
    group: "traffic",
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
    group: "traffic",
    palette: { label: "Jump to newest", hint: "newest" },
    run: (_a, ctx) => ctx.jumpToNewest(),
  },
  {
    name: "clear-table",
    group: "traffic",
    palette: { label: "Clear table", hint: "clear-table" },
    run: (_a, ctx) => ctx.clearTable(),
  },
  {
    name: "goto",
    group: "view",
    complete: () => VIEWS,
    run: ([view], ctx) => {
      if (!(VIEWS as readonly string[]).includes(view ?? "")) return ctx.out(`goto: ${VIEWS.join(" | ")}`, "warn");
      if (!ctx.goto(view!)) ctx.out(`goto: ${view} is not built yet`, "warn");
    },
  },
  {
    name: "theme",
    group: "app",
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
    group: "app",
    palette: { label: "Keyboard shortcuts", hint: "?" },
    run: (_a, ctx) => ctx.openKeys(),
  },
  {
    name: "console-key",
    group: "app",
    run: ([key], ctx) => {
      if (!key || !ctx.setConsoleKey(key)) return ctx.out("usage: console-key <one character>", "warn");
      ctx.out(`console key is now ${key}`, "sys");
    },
  },
  {
    name: "clear",
    group: "app",
    run: (_a, ctx) => ctx.clearConsole(),
  },
  { name: "god", group: "app", run: (_a, ctx) => ctx.out("no.") },
  ...VERBS.map(
    (v): Impl => ({
      name: v.name,
      group: "app",
      // An exchange verb runs on the selected one; a candidate's needs its id.
      palette: { label: `Agent: ${v.label}`, hint: v.hint(1), needsArgs: v.on === "candidate" || v.name === "test_for" },
      complete: v.name === "test_for" ? (args) => (args.length === 2 ? CATEGORIES.map(([c]) => c).filter((c) => c.startsWith(args[1]!.toUpperCase())) : []) : undefined,
      run: async (args, ctx) => {
        const want = v.on === "exchange" ? "exchange-id" : "candidate-id";
        const usage = `usage: ${v.name} <${want}>${v.name === "test_for" ? ` <${CATEGORIES.map(([c]) => c).join("|")}>` : ""}`;
        const isCategory = (a: string | undefined) => !!a && CATEGORIES.some(([c]) => c === a.toUpperCase());
        // `test_for INPV` runs on the selected exchange; `test_for e1 INPV` on e1.
        const [given, category] = v.name === "test_for" && args.length === 1 && isCategory(args[0]) ? [undefined, args[0]] : args;
        const id = given ?? (v.on === "exchange" ? ctx.selected()?.id : undefined);
        const max = v.name === "test_for" ? 2 : 1;
        if (!id || !ID.test(id) || args.length > max || (v.name === "test_for" && !isCategory(category))) return ctx.out(usage, "warn");
        const r = await ctx.verb(v.name, id, category);
        if (typeof r === "string") return ctx.out(`${v.name}: ${r}`, "warn");
        ctx.out(draftedLine(v.name, r), "sys");
      },
    }),
  ),
];

/** Names the UI implements and the server's registry doesn't (a test keeps this empty). */
export const UNREGISTERED: readonly string[] = IMPLS.filter((i) => !SPECS.some((s) => s.name === i.name && s.ui)).map((i) => i.name);

/** Every command the server's registry lists and the UI implements, in the registry's order. */
export const COMMANDS: readonly Command[] = SPECS.filter((s) => s.ui).flatMap((s) => {
  const impl = IMPLS.find((i) => i.name === s.name);
  return impl ? [{ ...impl, usage: s.usage, desc: s.desc, mcp: s.mcp, risk: s.risk, hidden: s.hidden, args: s.args }] : [];
});

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
