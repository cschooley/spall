// An in-memory engagement API for browser tests (#37) and screenshots: the
// browser tests run spall in log mode, so /api/engagement* is mocked with
// state that survives a reload.
import { expect } from "./fixtures.ts";
import type { Page } from "@playwright/test";

export type Json = Record<string, unknown>;

export const days = (n: number) => new Date(Date.now() + n * 86_400_000).toISOString().slice(0, 10);

export function engagementApi(page: Page, over: Json = {}) {
  const state = {
    e: {
      id: "acme-1a2b",
      name: "ACME Q4",
      client: "ACME",
      starts: days(-3),
      ends: days(30),
      expires: days(2),
      contacts: [],
      notes: "",
      scope: ["*.acme.test", "!pay.acme.test"],
      target_notes: {} as Record<string, string>,
      rules: { methods: [], safe_methods: ["GET", "HEAD", "OPTIONS"], rate_limit: 10, time_zone: "UTC", windows: [] as Json[], blackouts: [] as Json[], agents_allowed: false, destructive_allowed: false },
      ...over,
    } as Json & { scope: string[]; rules: Json; target_notes: Record<string, string> },
    others: [{ id: "beta-9f00", name: "Beta", client: "", active: false }],
    never: [] as string[],
    testing: { allowed: true, reason: null as string | null },
    calls: [] as { method: string; path: string; body: unknown }[],
  };
  const answer = () => ({ ...state.e, testing_now: state.testing, scope_all: false });
  void page.route("**/api/never-test", async (r) => {
    if (r.request().method() === "POST") {
      const c = r.request().postDataJSON() as { op: string; entry: string };
      state.calls.push({ method: "POST", path: "/api/never-test", body: c });
      if (c.op === "add" && !state.never.includes(c.entry)) state.never.push(c.entry);
      if (c.op === "remove") state.never = state.never.filter((x) => x !== c.entry);
      return r.fulfill({ json: { entries: state.never, changed: true } });
    }
    return r.fulfill({ json: { entries: state.never, exceptions: (state.e.never_test_exceptions as string[] | undefined) ?? [] } });
  });
  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: { source: "log", connected: null, proxy: "127.0.0.1:8080", replay: true, scope: state.e.scope, scope_all: false, engagement: { id: state.e.id, name: state.e.name }, testing: state.testing },
    }),
  );
  void page.route("**/api/engagement", async (r) => {
    if (r.request().method() === "PATCH") {
      const body = r.request().postDataJSON() as Json;
      expect(r.request().headers()["content-type"]).toBe("application/json");
      state.calls.push({ method: "PATCH", path: "/api/engagement", body });
      if (body.ends === "1999-01-01") return r.fulfill({ status: 400, json: { error: "ends: before starts" } });
      const { rules, ...rest } = body;
      Object.assign(state.e, rest);
      if (rules) Object.assign(state.e.rules, rules as Json);
      return r.fulfill({ json: { engagement: answer(), changed: true } });
    }
    return r.fulfill({ json: answer() });
  });
  void page.route("**/api/engagements", (r) => r.fulfill({ json: [{ id: state.e.id, name: state.e.name, client: state.e.client, active: true }, ...state.others] }));
  void page.route("**/api/scope", async (r) => {
    const c = r.request().postDataJSON() as { op: string; entry: string; add?: string[]; exclude?: string[] };
    state.calls.push({ method: "POST", path: "/api/scope", body: c });
    if (c.op === "add") state.e.scope = [c.entry, ...state.e.scope];
    if (c.op === "exclude") state.e.scope = [...state.e.scope, `!${c.entry}`];
    if (c.op === "remove") state.e.scope = state.e.scope.filter((x) => x !== c.entry);
    if (c.op === "apply") state.e.scope = [...state.e.scope, ...(c.add ?? []), ...(c.exclude ?? []).map((x) => `!${x}`)];
    await r.fulfill({ json: { scope: state.e.scope, all: false, changed: true } });
  });
  void page.route("**/api/ca", (r) =>
    r.fulfill({ json: { sha256: "AA:BB", subject: "CN=taphttp CA", not_before: null, not_after: "2036-11-20T00:00:00Z", proxy: "127.0.0.1:8080" } }),
  );
  void page.route("**/api/scope/check?*", (r) => r.fulfill({ json: { host: "pay.acme.test", state: "out", by: "!pay.acme.test" } }));
  return state;
}
