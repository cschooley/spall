// Rules of engagement (#36) as the UI sees them: the server's shape, the
// console's day/time syntax, and a short text summary. The server
// validates and enforces; this only parses what the tester types.

export const DAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
export type Day = (typeof DAYS)[number];

export interface Window {
  days: Day[];
  start: string;
  end: string;
}

export interface Blackout {
  from: string;
  to: string;
  note: string;
}

export interface Rules {
  methods: string[];
  safe_methods: string[];
  rate_limit: number | null;
  time_zone: string;
  windows: Window[];
  blackouts: Blackout[];
  agents_allowed: boolean;
  destructive_allowed: boolean;
  /** Refuse requests that look destructive by path or payload (#85); off: ask first. */
  refuse_destructive_looking: boolean;
}

export const DEFAULT_RULES: Rules = {
  methods: [],
  safe_methods: ["GET", "HEAD", "OPTIONS"],
  rate_limit: 10,
  time_zone: "UTC",
  windows: [],
  blackouts: [],
  agents_allowed: false,
  destructive_allowed: false,
  refuse_destructive_looking: false,
};

const isDay = (s: string): s is Day => (DAYS as readonly string[]).includes(s);

/** `mon-fri`, `sat,sun`, `weekdays`, `weekends`, `daily`; null if not days. */
export function parseDays(spec: string): Day[] | null {
  const named: Record<string, readonly Day[]> = { daily: DAYS, weekdays: DAYS.slice(0, 5), weekends: ["sat", "sun"] };
  const out: Day[] = [];
  for (const part of spec.toLowerCase().split(",")) {
    const p = part.trim();
    let days: readonly Day[];
    if (named[p]) days = named[p];
    else if (isDay(p)) days = [p];
    else {
      const m = /^([a-z]{3})-([a-z]{3})$/.exec(p);
      if (!m || !isDay(m[1]!) || !isDay(m[2]!)) return null;
      const a = DAYS.indexOf(m[1]);
      const b = DAYS.indexOf(m[2]);
      days = Array.from({ length: ((b - a + 7) % 7) + 1 }, (_, i) => DAYS[(a + i) % 7]!);
    }
    for (const d of days) if (!out.includes(d)) out.push(d);
  }
  return out.length ? out : null;
}

const minutes = (s: string, allow24: boolean) => {
  const m = /^(\d\d):(\d\d)$/.exec(s);
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h === 24 && min === 0 && allow24) return 1440;
  return h < 24 && min < 60 ? h * 60 + min : null;
};

/** `09:00-17:00` (an end before the start runs past midnight). */
export function parseSpan(spec: string): { start: string; end: string } | null {
  const [start, end, ...rest] = spec.split("-");
  if (!start || !end || rest.length) return null;
  const s = minutes(start, false);
  const e = minutes(end, true);
  return s === null || e === null || s === e ? null : { start, end };
}

/** `GET,POST` uppercased; `any` is the empty list. null if not methods. */
export function parseMethods(spec: string): string[] | null {
  if (spec.trim().toLowerCase() === "any") return [];
  const list = spec.split(",").map((m) => m.trim().toUpperCase()).filter(Boolean);
  return list.every((m) => /^[A-Z0-9_-]{1,32}$/.test(m)) ? list : null;
}

/** Days as a short phrase, the server's way: `mon-fri`, `sat,sun`, `daily`. */
export function daysText(days: readonly Day[]): string {
  const on = DAYS.map((d) => days.includes(d));
  if (on.every(Boolean)) return "daily";
  const parts: string[] = [];
  for (let i = 0; i < 7; i++) {
    if (!on[i]) continue;
    const start = i;
    while (i + 1 < 7 && on[i + 1]) i++;
    const len = i - start;
    parts.push(len === 0 ? DAYS[start]! : len === 1 ? `${DAYS[start]},${DAYS[i]}` : `${DAYS[start]}-${DAYS[i]}`);
  }
  return parts.join(",");
}

const local = (s: string) => s.replace("T", " ");

export function describeRules(r: Rules): string[] {
  const methods = r.methods.length ? r.methods.join(", ") : "any";
  const safe = r.safe_methods.length ? `confirm all but ${r.safe_methods.join(", ")}` : "confirm every method";
  const rate = r.rate_limit === null ? "no rate limit" : `rate ${r.rate_limit}/s`;
  const windows = r.windows.length ? r.windows.map((w, i) => `${i + 1}) ${daysText(w.days)} ${w.start}-${w.end}`).join("  ") : "any time";
  const blackouts = r.blackouts.length
    ? r.blackouts.map((b, i) => `${i + 1}) ${local(b.from)} → ${local(b.to)}${b.note ? ` (${b.note})` : ""}`).join("  ")
    : "none";
  const yes = (b: boolean) => (b ? "allowed" : "not allowed");
  return [
    `methods: ${methods} · ${safe} · ${rate} · time zone ${r.time_zone}`,
    `testing windows: ${windows}`,
    `no-test windows: ${blackouts}`,
    `agents: ${yes(r.agents_allowed)} · destructive actions: ${yes(r.destructive_allowed)} · requests that look destructive: ${r.refuse_destructive_looking ? "refused" : "asked for"}`,
  ];
}

const strs = (v: unknown) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);

/** Rules from the API; anything missing or malformed takes the default. */
export function asRules(v: unknown): Rules {
  if (!v || typeof v !== "object") return DEFAULT_RULES;
  const o = v as Record<string, unknown>;
  const windows = (Array.isArray(o.windows) ? o.windows : []).flatMap((w: unknown) => {
    const x = w as Record<string, unknown> | null;
    if (!x || typeof x.start !== "string" || typeof x.end !== "string") return [];
    return [{ days: strs(x.days).filter(isDay), start: x.start, end: x.end }];
  });
  const blackouts = (Array.isArray(o.blackouts) ? o.blackouts : []).flatMap((b: unknown) => {
    const x = b as Record<string, unknown> | null;
    if (!x || typeof x.from !== "string" || typeof x.to !== "string") return [];
    return [{ from: x.from, to: x.to, note: typeof x.note === "string" ? x.note : "" }];
  });
  return {
    methods: strs(o.methods),
    safe_methods: Array.isArray(o.safe_methods) ? strs(o.safe_methods) : DEFAULT_RULES.safe_methods,
    rate_limit: typeof o.rate_limit === "number" ? o.rate_limit : null,
    time_zone: typeof o.time_zone === "string" ? o.time_zone : "UTC",
    windows,
    blackouts,
    agents_allowed: o.agents_allowed === true,
    destructive_allowed: o.destructive_allowed === true,
    refuse_destructive_looking: o.refuse_destructive_looking === true,
  };
}

/** The rules' simple fields as form text. */
export interface RulesForm {
  methods: string;
  safe: string;
  rate: string;
  tz: string;
  agents: boolean;
  destructive: boolean;
  refuseLooking: boolean;
}

export function rulesFormOf(r: Rules): RulesForm {
  return {
    methods: r.methods.length ? r.methods.join(", ") : "any",
    safe: r.safe_methods.join(", "),
    rate: r.rate_limit === null ? "off" : String(r.rate_limit),
    tz: r.time_zone,
    agents: r.agents_allowed,
    destructive: r.destructive_allowed,
    refuseLooking: r.refuse_destructive_looking,
  };
}

/** The changes a form makes to the rules (only what changed), or a message. */
export function rulesPatchOf(f: RulesForm, r: Rules): Partial<Rules> | string {
  const patch: Partial<Rules> = {};
  const methods = parseMethods(f.methods);
  if (methods === null) return "methods: comma-separated names like GET, POST (or any)";
  const safe = f.safe.trim() === "" ? [] : parseMethods(f.safe);
  if (safe === null || (f.safe.trim().toLowerCase() === "any")) return "methods sent without confirmation: names like GET, HEAD (empty: confirm every method)";
  const same = (a: string[], b: string[]) => a.join(",") === b.join(",");
  if (!same(methods, r.methods)) patch.methods = methods;
  if (!same(safe, r.safe_methods)) patch.safe_methods = safe;
  const rateText = f.rate.trim().toLowerCase();
  const rate = rateText === "off" || rateText === "" || rateText === "0" ? null : Number(rateText);
  if (rate !== null && !(Number.isFinite(rate) && rate > 0)) return "rate: sends per second, like 5 or 0.5 (or off)";
  if (rate !== r.rate_limit) patch.rate_limit = rate ?? 0;
  if (f.tz.trim() !== r.time_zone) patch.time_zone = f.tz.trim();
  if (f.agents !== r.agents_allowed) patch.agents_allowed = f.agents;
  if (f.destructive !== r.destructive_allowed) patch.destructive_allowed = f.destructive;
  if (f.refuseLooking !== r.refuse_destructive_looking) patch.refuse_destructive_looking = f.refuseLooking;
  return patch;
}
