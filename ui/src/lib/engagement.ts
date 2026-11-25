// The engagement (#35) as the UI sees it: shape checks for spall's
// /api/engagement answers, and the one fetch helper the console uses.
// Engagement details are tester-entered, but still rendered as text only.

import type { EngagementInfo, EngagementSummary } from "../commands/registry.ts";
import { asRules } from "./roe.ts";

const str = (v: unknown) => (typeof v === "string" ? v : "");
const optStr = (v: unknown) => (typeof v === "string" && v !== "" ? v : null);
const strings = (v: unknown) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);

const notesOf = (v: unknown): Record<string, string> =>
  v && typeof v === "object" && !Array.isArray(v)
    ? Object.fromEntries(Object.entries(v as Record<string, unknown>).filter((kv): kv is [string, string] => typeof kv[1] === "string"))
    : {};

/** An engagement from the API, or null if it isn't one. */
export function asEngagement(v: unknown): EngagementInfo | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (typeof o.id !== "string" || typeof o.name !== "string") return null;
  return {
    id: o.id,
    name: o.name,
    client: str(o.client),
    starts: optStr(o.starts),
    ends: optStr(o.ends),
    expires: optStr(o.expires),
    scope: strings(o.scope),
    notes: str(o.notes),
    targetNotes: notesOf(o.target_notes),
    apiSpecs: asApiSpecs(o.api_specs),
    rules: asRules(o.rules),
    testingNow: testingOf(o.testing_now),
  };
}

/** `{allowed, reason}` from the API, or null. */
export function testingOf(v: unknown): { allowed: boolean; reason: string | null } | null {
  if (!v || typeof v !== "object") return null;
  const o = v as { allowed?: unknown; reason?: unknown };
  return typeof o.allowed === "boolean" ? { allowed: o.allowed, reason: typeof o.reason === "string" ? o.reason : null } : null;
}

export function asSummaries(v: unknown): EngagementSummary[] | null {
  if (!Array.isArray(v)) return null;
  return v.flatMap((x) => {
    const e = asEngagement(x);
    return e ? [{ id: e.id, name: e.name, client: e.client, active: (x as { active?: unknown }).active === true }] : [];
  });
}

/** Fetch JSON from spall; a string is an error message to show. */
export async function apiJson(path: string, init?: { method: string; body: unknown }): Promise<unknown> {
  try {
    const res = await fetch(
      path,
      init ? { method: init.method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(init.body) } : undefined,
    );
    const body: unknown = await res.json().catch(() => null);
    if (!res.ok) {
      const msg = (body as { error?: unknown } | null)?.error;
      return typeof msg === "string" ? msg : `HTTP ${res.status}`;
    }
    return body;
  } catch {
    return "spall unreachable";
  }
}

/** Today's date (`YYYY-MM-DD`) in a time zone; UTC if the zone is unknown. */
export function todayIn(timeZone: string, now: Date): string {
  const fmt = (tz: string) => new Intl.DateTimeFormat("en-CA", { timeZone: tz, year: "numeric", month: "2-digit", day: "2-digit" }).format(now);
  try {
    return fmt(timeZone);
  } catch {
    return fmt("UTC");
  }
}

const dayNumber = (d: string) => {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(d);
  return m ? Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])) / 86_400_000 : null;
};

/** The authorization countdown: "expires in 3 days", "expired 2 days ago". */
export function expiryText(expires: string | null, today: string): { text: string; tone: "ok" | "soon" | "past" } | null {
  const e = expires ? dayNumber(expires) : null;
  const t = dayNumber(today);
  if (e === null || t === null) return null;
  const n = e - t;
  const days = (k: number) => `${k} day${k === 1 ? "" : "s"}`;
  if (n < 0) return { text: `authorization expired ${days(-n)} ago`, tone: "past" };
  if (n === 0) return { text: "authorization expires today (through end of day)", tone: "soon" };
  return { text: `authorization expires in ${days(n)}`, tone: n <= 3 ? "soon" : "ok" };
}

export type ViewId = "engagement" | "traffic" | "surface" | "tasks" | "findings";

/** The view a location hash names; Engagement (home) by default (#71). */
export function viewFromHash(hash: string): ViewId {
  const v = hash.replace(/^#/, "").split("/")[0];
  return v === "traffic" || v === "surface" || v === "tasks" || v === "findings" ? v : "engagement";
}

/** One line of a scope-list preview (#38), as spall's scope engine read it. */
export interface PreviewItem {
  line: number;
  source: string;
  entry: string | null;
  exclude: boolean;
  error: string | null;
  notes: string[];
  present: boolean;
  /** Too wide to add without the override (#83); a note says why. */
  wide: boolean;
}

export interface Preview {
  items: PreviewItem[];
  add: string[];
  exclude: string[];
}

/** A scope-list preview from the API, or null if it isn't one. */
export function asPreview(v: unknown): Preview | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (!Array.isArray(o.items)) return null;
  const items = o.items.flatMap((x: unknown): PreviewItem[] => {
    const i = x as Record<string, unknown> | null;
    if (!i || typeof i.line !== "number" || typeof i.source !== "string") return [];
    return [
      {
        line: i.line,
        source: i.source,
        entry: typeof i.entry === "string" ? i.entry : null,
        exclude: i.exclude === true,
        error: typeof i.error === "string" ? i.error : null,
        notes: strings(i.notes),
        present: i.present === true,
        wide: i.wide === true,
      },
    ];
  });
  return { items, add: strings(o.add), exclude: strings(o.exclude) };
}

/** An imported API description kept on the engagement (#40). */
export interface ApiSpec {
  title: string;
  version: string;
  format: string;
  servers: string[];
  paths: string[];
  imported: string;
}

export function asApiSpecs(v: unknown): ApiSpec[] {
  if (!Array.isArray(v)) return [];
  return v.flatMap((x: unknown): ApiSpec[] => {
    const o = x as Record<string, unknown> | null;
    if (!o || typeof o.title !== "string") return [];
    const s = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : "");
    return [{ title: o.title, version: s("version"), format: s("format"), servers: strings(o.servers), paths: strings(o.paths), imported: s("imported") }];
  });
}

/** An OpenAPI import preview (#40): the spec, its notes, and the scope preview. */
export function asImport(v: unknown): { spec: ApiSpec; notes: string[]; preview: Preview } | null {
  const preview = asPreview(v);
  const o = v as Record<string, unknown> | null;
  const [spec] = asApiSpecs([o?.spec]);
  return preview && spec ? { spec, notes: strings(o?.notes), preview } : null;
}
