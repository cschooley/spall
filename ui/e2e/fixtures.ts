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
  await page.getByPlaceholder("filter: host, path, method, status").fill(text);
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
        if (m.type() === "error" || m.type() === "warning") problems.push(`console ${m.type()}: ${m.text()}`);
      });
      page.on("pageerror", (e) => problems.push(`page error: ${e.message}`));
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
