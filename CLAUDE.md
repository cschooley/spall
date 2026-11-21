# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Project

spall is the DAST tool in Kudzu: an
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

**M1 "traffic workbench": done** (tag `v0.2.0`, GitHub milestone
closed, #3-#12). The Traffic view: history and search from taphttp's
lever API, filter syntax, virtualized table, shared inspector, command
palette and `` ` `` console on one command registry
(`ui/src/commands/registry.ts`), verdigris styling and chrome, and
Playwright browser tests.

```
client --(proxy 127.0.0.1:8080)--> taphttp --> internet
                                      | lever API :8081 (compose network only)
                                      |   GET /exchanges, /feed?since=, POST /replay; token on its own volume
                                    spall (Rust, axum) --> browser http://127.0.0.1:3000
                                      | /events (SSE), /api/exchanges, /api/status
                                      | serves ui/ (React + Vite + TS, embedded with rust-embed)
```

## Current milestone: M2 "replay"

Goal (docs/architecture.md): send-to-replay from the UI via taphttp;
replay results shown linked to the original. docs/ux.md: replay lives
in the inspector (edit, send, see original and replay side by side);
the mockup shows a Replay button (`⏎`) and a Replays tab.

Issues (GitHub milestone "M2 replay"), roughly in order:

- **#17**: `POST /api/replay` plus a same-origin (CSRF) guard on every
  state-changing route. Everything else builds on it.
- **#18**: replay as captured from the inspector (`⏎`) and console;
  includes the decision on confirming unsafe methods.
- **#19**: edit before replay (method, headers, body).
- **#20**: compare a replay with its original; a reusable diff (M4
  evidence, M8 retests).
- **#21**: the Replays tab and links between originals and replays.
- **#22**: the action gate: server-side scope (URL host and effective
  `Host` header), unsafe-method policy with a single-use confirmation
  token (#18's second `⏎` sends it), and an audit record for every
  attempt. #17 routes through it; M6 agents reuse it.
- taphttp side: cschooley/taphttp#10 (ordered header edits, binary
  bodies, `replay_of` filter). Not blocking; #19 and #21 work around it.

What exists: taphttp v0.5.0 (`taphttp-types` 0.4.0, schema v4) `POST /replay` takes `{id, method?,
headers?, body?}`, records the result as a new exchange with
`replay_of` set, and returns it (it also appears on `/feed`). The
registry reserves `replay` (mockup: `replay <id> [--header H:V]`).

Known constraints and requirements:

- **CSRF.** Replay is spall's first state-changing endpoint, on an
  unauthenticated localhost app. The Host allowlist doesn't stop it: a
  cross-site POST from any web page carries `Host: 127.0.0.1:3000`. A
  forged replay would resend the tester's captured requests, with their
  credentials, to targets the attacker picks. Require same-origin
  (check `Origin` / `Sec-Fetch-Site`), accept only `application/json`
  (forces a CORS preflight that spall never approves), and test it from
  a foreign origin.
- spall proxies replay server-side (`POST /api/replay`); the token never
  reaches the browser.
- taphttp v0.5.0 gaps (still open as taphttp#10; file on taphttp when needed): header overrides
  are a name->value map (no repeated headers); the body override is a
  string (binary bodies can only be replayed as captured); `GET
  /exchanges` can't filter by `replay_of`, which a Replays tab wants.
- Agent-driven replay is M6 (MCP), not M2.

Explicitly not in M2: the engagement model and scope UI (M3; #22's
scope comes from config until then), candidates, triage and flux (M4), export (M5),
MCP server, agent tasks, e-stop, attribution (M6), intercept (M7),
image preview (#15, later). Unbuilt views and tools stay dimmed, inert
placeholders (docs/ux.md) until they ship.

## Testing

TDD, same as the rest of Kudzu:

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
- CI (`.github/workflows/ci.yml`) runs all of the above; the security
  workflow runs sec-actions scanners (audit mode, SARIF to the Security
  tab). Pin every action to a commit SHA (`@<sha> # vX.Y.Z`; the repo's
  Actions settings require it, so an unpinned `uses:` fails) and every
  scanner version; a
  suppression in `osv-scanner.toml` needs a reason and `ignoreUntil`.
- New npm or Rust dependencies: versions at least two weeks old,
  transitive ones included (`npm install --before=<date>`;
  `cargo update --precise`).
- No `.unwrap()` / `.expect()` in non-test code.

## Provenance

Built with Claude Code, human-directed and reviewed. Stated openly in
the README, not hidden.
