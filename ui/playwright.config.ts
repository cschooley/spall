import { defineConfig, devices } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

// Browser tests (#7): headless Chromium against a real spall, tailing a
// fixture log that the tests append to. No Docker. Debug spall reads
// ui/dist from disk, so `npm run build` first (test:e2e does).
//
// Browser: Playwright's own Chromium (`npx playwright install chromium`;
// ignore-scripts blocks the automatic download), or any recent Chromium via
// SPALL_E2E_CHROMIUM=/path/to/chromium.

const port = Number(process.env.SPALL_E2E_PORT ?? 3917);
export const LOG_FILE = resolve(import.meta.dirname, ".e2e/traffic.jsonl");

// Start every run from an empty log. Only the main process: workers load
// this file too and must not truncate it mid-run.
if (!process.env.TEST_WORKER_INDEX) {
  mkdirSync(resolve(import.meta.dirname, ".e2e"), { recursive: true });
  writeFileSync(LOG_FILE, "");
}

const executablePath = process.env.SPALL_E2E_CHROMIUM;

export default defineConfig({
  testDir: "e2e",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? "list" : [["list"]],
  timeout: 30_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    ...devices["Desktop Chrome"],
    viewport: { width: 1440, height: 900 },
    launchOptions: executablePath ? { executablePath } : {},
    trace: "retain-on-failure",
  },
  webServer: {
    command: [
      "cargo run --quiet --manifest-path ../Cargo.toml --",
      `--log-file ${LOG_FILE}`,
      `--listen 127.0.0.1:${port}`,
      "--poll-ms 50",
      "--history 5000",
    ].join(" "),
    url: `http://127.0.0.1:${port}/healthz`,
    reuseExistingServer: false,
    timeout: 300_000,
    stdout: "ignore",
    stderr: "pipe",
  },
});
