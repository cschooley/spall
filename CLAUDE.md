# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Project

spall is the DAST tool in the cschooley security toolkit: an
assessment workbench for interactive and agentic web app testing.

- **Web UI (SPA)** for a human tester: live traffic, candidate
  findings, triage.
- **MCP server** so an agent can drive assessments.
- **Findings** are flux (a SARIF 2.1.0 profile), via the `flux` crate.

spall does **not** proxy traffic itself. taphttp does that. spall
consumes taphttp's lever API (history, live feed, replay), its traffic
log as a fallback, and later its intercept hook. If you are about to
write proxy, TLS, or CA code in this repo, stop: it belongs in taphttp.
When taphttp needs a change, file it there (and note it in
`docs/taphttp-requests.md`) instead of working around it here.

Read `docs/architecture.md` before designing anything beyond the
current milestone. Read `docs/ux.md` before building any UI: spall is
object-centric (exchanges, endpoints, tasks, findings), not a set of
Burp-style tool tabs. Read `docs/flux/SPEC.md` (vendored from the flux
repo) before touching findings.

UI styling comes from verdigris (`cschooley/verdigris`): use its
`tokens.css` variables, never hard-coded colors, and treat its
`mockups/spall-*.html` as the visual target. Tokens and icons are
vendored (copied, pinned to a verdigris tag, never edited here) in
`ui/src/vendor/verdigris/`; see its `SOURCE.md` to re-vendor.

## Hard rules

- **No findings format of our own.** Findings are flux. Once the
  `flux` crate exists, use its types; do not define a local
  `Finding`, `Severity`, or `Evidence`.
- **No Kudzu-specific code.** spall reads and writes flux. Kudzu
  ingests flux.
- **The agent proposes, a human promotes.** The MCP server may expose
  `propose_finding`. It must not expose promotion/export to the agent
  by default.
- **Redaction on export.** Never export traffic or findings without
  flux redaction unless an explicit opt-in flag is set.
- **Localhost by default.** The web UI shows raw traffic, including
  credentials. Bind to 127.0.0.1 (and publish Docker ports to
  127.0.0.1 only) unless explicitly configured otherwise. Real auth
  comes before any non-local binding.
- **Traffic is never HTML.** Captured traffic is attacker-controlled and
  is rendered as text only. No `dangerouslySetInnerHTML`, `innerHTML` or
  equivalent on traffic data. Frontend stack, CSP and dependency budget
  are in `docs/architecture.md` (Frontend); follow them.
- **Authorized testing only.** README states plainly that spall is
  for authorized testing and local development.

## Milestones

**M0 "hello": done** (tag `v0.1.0`). `docker compose up`, point a
client at the proxy, watch traffic in a browser console: compose with
taphttp + spall, the log tailer, SSE, a static console page, README
quickstart, tests. Superseded since by M1's stack.

## Current milestone: M1 "traffic workbench"

GitHub milestone "M1 traffic workbench". Goal (docs/architecture.md):
filtering, search, request/response detail view, history from
taphttp's lever API.

```
client --(proxy 127.0.0.1:8080)--> taphttp --> internet
                                      | lever API :8081 (compose network only)
                                      |   GET /exchanges, /feed?since=, token on its own volume
                                    spall (Rust, axum) --> browser http://127.0.0.1:3000
                                      | /events (SSE), /api/exchanges, /api/status
                                      | serves ui/ (React + Vite + TS, embedded with rust-embed)
```

Built (#3-#9): the frontend stack and its rules (#3), history from the
lever API (#4), inspector rendering rules (#5), verdigris styling (#6),
Playwright browser tests (#7), the Traffic view (#8: rail, shared
inspector, history search, command palette), scaffolding (#9), plus
mockup chrome (brand mark, five-view rail, suite launcher).

Also done: the `` ` `` console and one command registry shared with
the palette (#11), and the mockup-parity review (#12). No M1 issues
remain open; closing the milestone and tagging `v0.2.0` is pending.
Next is M2 (replay).

Explicitly not in M1: replay (M2), candidates, triage and flux (M3),
export (M4), MCP server, agent tasks, e-stop, attribution (M5),
intercept (M6), image preview (#15, later). Unbuilt views and tools
appear as dimmed, inert placeholders (docs/ux.md) until they ship.

## Testing

TDD, same as the rest of the suite:

- Write the test first. No behavior ships without a test that would
  catch its regression.
- Unit-test non-trivial logic in its module (`#[cfg(test)] mod tests`).
- Integration-test user-visible workflows.
- `cargo test` green and `cargo clippy --all-targets -- -D warnings`
  clean before committing.
- UI (`ui/`, Node from `ui/.nvmrc`): `npm run lint`, `npm test`
  (node:test) and `npm run test:e2e` (Playwright, headless Chromium;
  `SPALL_E2E_CHROMIUM=/usr/bin/chromium` to use a system browser) all
  green. Untrusted-content rendering is always covered.
- New npm or Rust dependencies: versions at least two weeks old,
  transitive ones included (`npm install --before=<date>`;
  `cargo update --precise`).
- No `.unwrap()` / `.expect()` in non-test code.

## Provenance

Built with Claude Code, human-directed and reviewed. Stated openly in
the README, not hidden.
