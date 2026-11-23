// An in-memory candidates API for browser tests (#43-#47). The browser
// tests run spall in log mode, which keeps no candidates, so
// /api/candidates* and /api/status are mocked here with the server's
// rules: a matching fingerprint (endpoint without query, parameter, rule)
// answers 409 with the existing candidate; triage steps change flux
// fields the way src/candidates.rs does.
import { expect } from "./fixtures.ts";
import type { Page, Route } from "@playwright/test";
import type { TrafficEntry } from "../src/types.ts";

type Json = Record<string, unknown>;

export interface Stored {
  id: string;
  created: string;
  updated: string;
  flagged_by: string;
  promoted?: string | null;
  finding: Json & { properties: Json; suppressions?: Json[]; correlationGuid?: string };
}

const CWE: Record<string, number> = { "WSTG-INPV-05": 89, "WSTG-INPV-01": 79, "WSTG-ATHZ-04": 639 };

export function findingFor(rule: string, e: Pick<TrafficEntry, "id" | "method" | "url" | "status">, param?: string, extra: Json = {}): Stored["finding"] {
  const url = e.url.split(/[?#]/)[0]!;
  return {
    ruleId: rule,
    level: "warning",
    message: { text: `${rule}: ${e.method} ${url}${param ? `, parameter ${param}` : ""}` },
    fingerprints: { "flux/dast/v1": `${url}|${param ?? ""}|${rule}` },
    locations: [{ physicalLocation: { artifactLocation: { uri: url } }, logicalLocations: [{ name: e.method, kind: "httpMethod" }] }],
    taxa: CWE[rule] ? [{ id: String(CWE[rule]), toolComponent: { name: "CWE" } }] : [],
    webRequest: { method: e.method, target: e.url },
    webResponse: e.status === null ? { noResponseReceived: true } : { statusCode: e.status },
    properties: { "flux/status": "candidate", "flux/replayIds": [e.id], ...(param ? { "spall/parameter": param } : {}) },
    ...extra,
  };
}

export function candidatesApi(page: Page, opts: { entries?: TrafficEntry[]; seed?: Stored[]; engagement?: string; unredactedAllowed?: boolean } = {}) {
  let n = 0;
  const state = {
    list: [...(opts.seed ?? [])] as Stored[],
    calls: [] as { method: string; path: string; body: unknown }[],
    entries: new Map((opts.entries ?? []).map((e) => [e.id, e])),
  };
  const guid = () => `6f1c2e3a-0000-4000-8000-${String(++n).padStart(12, "0")}`;
  const inEffect = (c: Stored) => (c.finding.suppressions ?? []).some((s) => s.status === undefined || s.status === "accepted");
  const toTriage = () => state.list.filter((c) => c.finding.properties["flux/status"] === "candidate" && !inEffect(c)).length;

  void page.route("**/api/status", (r) =>
    r.fulfill({
      json: {
        source: "lever",
        connected: true,
        proxy: "127.0.0.1:8080",
        replay: true,
        scope: ["*"],
        scope_all: false,
        engagement: { id: opts.engagement ?? "acme-1a2b", name: "ACME Q4" },
        testing: { allowed: true, reason: null },
        to_triage: toTriage(),
        export: { promoted: state.list.filter((c) => c.promoted && !inEffect(c)).length, unredacted_allowed: opts.unredactedAllowed === true },
      },
    }),
  );
  // In lever mode the UI asks for history; serve what the log holds.
  void page.route("**/api/exchanges?*", (r) => {
    const q = new URL(r.request().url()).searchParams;
    const of = q.get("replay_of");
    const all = [...state.entries.values()].filter((e) => (of ? e.replay_of === of : true));
    return r.fulfill({ json: q.get("before") ? [] : all.sort((a, b) => b.ts.localeCompare(a.ts)) });
  });
  void page.route(/\/api\/exchanges\/[^/?]+$/, (r) => {
    const id = decodeURIComponent(new URL(r.request().url()).pathname.split("/").pop()!);
    const e = state.entries.get(id);
    return e ? r.fulfill({ json: e }) : r.fulfill({ status: 404, json: { error: `no exchange ${id}` } });
  });
  void page.route(/\/api\/candidates(\?.*)?$/, async (r: Route) => {
    const req = r.request();
    if (req.method() === "POST") {
      expect(req.headers()["content-type"]).toBe("application/json");
      const b = req.postDataJSON() as { exchange: string; rule: string; parameter?: string; replays?: string[]; level?: string; message?: string };
      state.calls.push({ method: "POST", path: "/api/candidates", body: b });
      const e = state.entries.get(b.exchange);
      if (!e) return r.fulfill({ status: 404, json: { error: `no exchange ${b.exchange}` } });
      const f = findingFor(b.rule, e, b.parameter);
      if (b.level) f.level = b.level;
      if (b.message) f.message = { text: b.message };
      f.properties["flux/replayIds"] = [e.id, ...(b.replays ?? [])];
      const fp = (f.fingerprints as Json)["flux/dast/v1"];
      const existing = state.list.find((c) => (c.finding.fingerprints as Json)["flux/dast/v1"] === fp);
      if (existing) return r.fulfill({ status: 409, json: { error: `already flagged as ${existing.id}`, existing } });
      const c: Stored = { id: `c-${String(state.list.length + 1).padStart(8, "0")}`, created: new Date().toISOString(), updated: new Date().toISOString(), flagged_by: "human", finding: f };
      state.list.push(c);
      return r.fulfill({ status: 201, json: c });
    }
    const x = new URL(req.url()).searchParams.get("exchange");
    const list = [...state.list].reverse().filter((c) => !x || (c.finding.properties["flux/replayIds"] as string[]).includes(x));
    return r.fulfill({ json: { candidates: list } });
  });
  // What redaction changes: cookie and authorization headers, as flux does.
  void page.route(/\/api\/candidates\/[^/?]+\/redaction$/, (r) => {
    const id = decodeURIComponent(new URL(r.request().url()).pathname.split("/").at(-2)!);
    const c = state.list.find((x) => x.id === id);
    if (!c) return r.fulfill({ status: 404, json: { error: `no candidate ${id}` } });
    const e = state.entries.get((c.finding.properties["flux/replayIds"] as string[])[0]!);
    const changes = (e?.req_headers ?? [])
      .filter(([k]) => ["cookie", "authorization"].includes(k.toLowerCase()))
      .map(([k]) => ({ at: `webRequest.headers["${k.toLowerCase()}"]`, kind: k.toLowerCase() }));
    return r.fulfill({ json: { id, finding: { ...c.finding, webRequest: { target: e?.url } }, changes } });
  });
  void page.route("**/api/export", (r) => {
    const b = r.request().postDataJSON() as { unredacted?: boolean };
    state.calls.push({ method: "POST", path: "/api/export", body: b });
    if (b.unredacted && !opts.unredactedAllowed) return r.fulfill({ status: 403, json: { error: "an unredacted export needs spall started with --allow-unredacted-export" } });
    const results = state.list.filter((c) => c.promoted && !inEffect(c)).map((c) => ({ ...c.finding, properties: { ...c.finding.properties, "flux/redacted": !b.unredacted } }));
    if (!results.length) return r.fulfill({ status: 409, json: { error: "nothing promoted to export (promote confirmed candidates first)" } });
    return r.fulfill({
      status: 200,
      headers: { "content-type": "application/sarif+json", "content-disposition": `attachment; filename="spall-acme-1a2b-20261123${b.unredacted ? "-UNREDACTED" : ""}.sarif"` },
      body: JSON.stringify({ version: "2.1.0", runs: [{ tool: { driver: { name: "spall" } }, results }] }),
    });
  });
  void page.route(/\/api\/candidates\/[^/?]+$/, async (r: Route) => {
    const req = r.request();
    const id = decodeURIComponent(new URL(req.url()).pathname.split("/").pop()!);
    const b = req.postDataJSON() as Json & { op: string };
    state.calls.push({ method: req.method(), path: `/api/candidates/${id}`, body: b });
    const c = state.list.find((x) => x.id === id);
    if (!c) return r.fulfill({ status: 404, json: { error: `no candidate ${id}` } });
    const f = c.finding;
    const need = () => typeof b.justification === "string" && b.justification.trim() !== "";
    const decide = (disposition: string, extra: Json = {}) => {
      f.suppressions = [{ kind: "external", status: "accepted", justification: b.justification, properties: { "flux/disposition": disposition, ...extra } }];
    };
    if (b.op === "confirm") {
      f.properties["flux/status"] = "confirmed";
      f.correlationGuid ??= guid();
    } else if (b.op === "reject" || b.op === "accept") {
      if (!need()) return r.fulfill({ status: 400, json: { error: "justification: required" } });
      decide(b.op === "reject" ? "falsePositive" : "acceptedRisk");
    } else if (b.op === "duplicate") {
      if (!need()) return r.fulfill({ status: 400, json: { error: "justification: required" } });
      const orig = state.list.find((x) => x.id === b.of);
      if (!orig || orig.id === id) return r.fulfill({ status: 400, json: { error: `no candidate ${String(b.of)} to duplicate` } });
      orig.finding.correlationGuid ??= guid();
      b.justification = `duplicate of ${orig.finding.correlationGuid}: ${String(b.justification)}`;
      decide("duplicate", { "flux/duplicateOf": orig.finding.correlationGuid });
    } else if (b.op === "reopen") {
      if (!f.suppressions?.length) return r.fulfill({ status: 400, json: { error: "not rejected, accepted or a duplicate" } });
      f.suppressions = [];
    } else if (b.op === "evidence") {
      const ids = f.properties["flux/replayIds"] as string[];
      for (const x of b.exchanges as string[]) if (!ids.includes(x)) ids.push(x);
    } else if (b.op === "promote") {
      if (f.properties["flux/status"] !== "confirmed" || inEffect(c)) return r.fulfill({ status: 400, json: { error: "only a confirmed candidate that isn't rejected, accepted or a duplicate can be promoted" } });
      c.promoted = new Date().toISOString();
    } else if (b.op === "unpromote") {
      if (!c.promoted) return r.fulfill({ status: 400, json: { error: "not promoted" } });
      c.promoted = null;
    } else if (b.op === "edit") {
      if (b.level) f.level = b.level;
      if (typeof b.rank === "number") f.rank = b.rank;
      if (typeof b.message === "string") f.message = { text: b.message };
    } else return r.fulfill({ status: 400, json: { error: "not a candidate change" } });
    c.updated = new Date().toISOString();
    return r.fulfill({ json: c });
  });
  return state;
}
