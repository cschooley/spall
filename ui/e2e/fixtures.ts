import { test as base, expect, type Page } from "@playwright/test";
import { appendFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { LOG_FILE } from "../playwright.config.ts";
import type { TrafficEntry } from "../src/types.ts";

/** Unique per test, so tests sharing one spall can filter to their own rows. */
export function token(): string {
  return `t${randomUUID().slice(0, 8)}`;
}

export function entry(over: Partial<TrafficEntry> = {}): TrafficEntry {
  return {
    schema_version: 3,
    id: randomUUID(),
    ts: new Date().toISOString(),
    host: "example.test",
    method: "GET",
    url: "http://example.test/",
    req_headers: [["host", "example.test"]],
    req_body: null,
    req_body_encoding: "utf8",
    status: 200,
    res_headers: [["content-type", "text/plain"]],
    res_body: null,
    res_body_encoding: "utf8",
    duration_ms: 12,
    replay_of: null,
    ...over,
  };
}

/** Play taphttp: append entries to the log spall is tailing. */
export function append(...entries: TrafficEntry[]): void {
  appendFileSync(LOG_FILE, entries.map((e) => JSON.stringify(e) + "\n").join(""));
}

export async function filterTo(page: Page, text: string): Promise<void> {
  await page.getByLabel("Filter").fill(text);
  // The filter is debounced; wait until the table reflects it.
  await expect(page.locator(".tablewrap")).toHaveAttribute("data-filter", text);
}

/**
 * Every test fails on a console error, page error, dialog, or CSP
 * violation: each of those would mean traffic did something it shouldn't.
 */
export const test = base.extend<{ guard: void }>({
  guard: [
    async ({ page }, use) => {
      const problems: string[] = [];
      page.on("console", (m) => {
        // The action gate answers with these on purpose (deny, refused
        // token, needs confirmation, recorded failure); Chromium logs any
        // non-2xx fetch. Only for /api/replay and /api/fetch (#74), only
        // those statuses.
        const gateAnswer =
          /^Failed to load resource: the server responded with a status of (403|409|428|429|502)\b/.test(m.text()) &&
          /\/api\/(replay|fetch)$/.test(m.location().url);
        // A refused engagement change (bad input) is a 400 on purpose too,
        // and so is a too-wide scope entry without the override (#83).
        const refusedChange =
          /^Failed to load resource: the server responded with a status of 400\b/.test(m.text()) &&
          (m.location().url.endsWith("/api/engagement") || m.location().url.endsWith("/api/scope"));
        // Flagging a candidate that exists answers 409 with it (#43, #46);
        // a refused triage step (no justification) is a 400.
        const candidateAnswer =
          /^Failed to load resource: the server responded with a status of (400|403|409)\b/.test(m.text()) &&
          /\/api\/(candidates(\/[^/?]+)?|export)$/.test(m.location().url);
        // Evidence that taphttp no longer has is a 404 on purpose (#45).
        const goneEvidence =
          /^Failed to load resource: the server responded with a status of 404\b/.test(m.text()) && /\/api\/exchanges\/[^/?]+$/.test(m.location().url);
        // A refused approval (nothing to decide, past the RoE) is on purpose.
        const taskAnswer =
          /^Failed to load resource: the server responded with a status of (400|409)\b/.test(m.text()) && /\/api\/tasks(\/[^/?]+)?$/.test(m.location().url);
        if (gateAnswer || refusedChange || candidateAnswer || goneEvidence || taskAnswer) return;
        if (m.type() === "error" || m.type() === "warning") problems.push(`console ${m.type()}: ${m.text()}`);
      });
      page.on("pageerror", (e) => problems.push(`page error: ${e.message}`));
      // Tests that mock a lever-connected spall (replay: true) make the
      // inspector and the Engagement view ask for candidates; spall in log
      // mode has none. Tests that need candidates mock them (candidates-api.ts),
      // which takes precedence over this.
      await page.route(/\/api\/candidates(\?.*)?$/, (r) => (r.request().method() === "GET" ? r.fulfill({ json: { candidates: [] } }) : r.fallback()));
      // And for the Engagement view's activity (#62), from the audit log.
      await page.route(/\/api\/audit(\?.*)?$/, (r) => r.fulfill({ json: [] }));
      // The same for the tasks waiting on approval (#56).
      await page.route(/\/api\/tasks$/, (r) => (r.request().method() === "GET" ? r.fulfill({ json: { tasks: [] } }) : r.fallback()));
      // And the intake's draft plans (#65).
      await page.route(/\/api\/plans$/, (r) => (r.request().method() === "GET" ? r.fulfill({ json: { plans: [] } }) : r.fallback()));
      page.on("dialog", async (d) => {
        problems.push(`dialog: ${d.type()} ${d.message()}`);
        await d.dismiss();
      });
      await page.addInitScript(() => {
        document.addEventListener("securitypolicyviolation", (e) => {
          console.error(`CSP violation: ${e.violatedDirective} ${e.blockedURI}`);
        });
      });
      await use();
      expect(problems, "console errors, page errors, dialogs or CSP violations").toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
