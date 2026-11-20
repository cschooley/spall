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
consumes taphttp's traffic log, replay, and intercept hook. If you
are about to write proxy, TLS, or CA code in this repo, stop: it
belongs in taphttp.

Read `docs/architecture.md` before designing anything beyond the
current milestone. Read `docs/ux.md` before building any UI: spall is
object-centric (exchanges, endpoints, tasks, findings), not a set of
Burp-style tool tabs. Read `docs/flux/SPEC.md` (vendored from the flux
repo) before touching findings.

UI styling comes from verdigris (`cschooley/verdigris`): use its
`tokens.css` variables, never hard-coded colors, and treat its
`mockups/spall-*.html` as the visual target.

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
- **Authorized testing only.** README states plainly that spall is
  for authorized testing and local development.

## Current milestone: M0 "hello"

Goal: `docker compose up`, point a client at the proxy, watch traffic
scroll by in a browser console. Nothing else.

```
client --(HTTP proxy :8080)--> taphttp --> internet
                                  |
                       /data/traffic.jsonl (shared volume)
                                  |
                               spall (tails file)
                                  |  SSE
                                  v
                       browser: http://127.0.0.1:3000
```

### Deliverables

1. **`docker-compose.yml`** with two services sharing a volume:
   - `taphttp`: built from `https://github.com/cschooley/taphttp.git`
     (compose `build.context` with a git URL, or a Dockerfile under
     `docker/taphttp/` that clones and builds; pick one and say why).
     Runs `taphttp start --listen 0.0.0.0:8080 --log-file /data/traffic.jsonl`
     with its data dir (CA) on a named volume so the CA survives
     restarts. Port published as `127.0.0.1:8080:8080`.
   - `spall`: this repo. Port `127.0.0.1:3000:3000`. Mounts the same
     volume read-only.
2. **spall binary** (Rust, tokio + axum):
   - Tails `/data/traffic.jsonl` (handles file not existing yet,
     truncation, and partial lines).
   - Parses each line as a taphttp `TrafficEntry`. Malformed lines
     are logged and skipped, never crash the tailer.
   - `GET /events`: Server-Sent Events, one event per entry.
   - `GET /`: a single static HTML page (no framework, no build step
     for M0) showing a live, scrolling table: time, method, host,
     path, status, duration. Click a row to expand headers and bodies.
   - `GET /healthz`.
   - Config via env/flags: log path, listen address.
3. **README quickstart**: `docker compose up`, then
   `curl -x http://127.0.0.1:8080 http://example.com`, then open
   `http://127.0.0.1:3000`. For HTTPS, show how to get the taphttp CA
   out of the volume (`taphttp ca info`) or use `curl --proxy-insecure -k`
   for the demo, and say clearly that `-k` is demo-only.
4. **Tests** (TDD, see below): tailer behavior (append, partial
   line, truncation, missing file), entry parsing, SSE endpoint emits
   an event for an appended line. One end-to-end test that runs
   against a temp file, no Docker required.

### Explicitly not in M0

- Findings, triage, flux export
- MCP server
- Intercept (pause/modify) via taphttp's `--intercept-socket`
- Replay
- An SPA framework or JS build pipeline
- Auth (M0 is localhost-only; see hard rules)

If you notice taphttp needs a change to make M0 work (e.g. it has no
Dockerfile, or the log format lacks something), note it in
`docs/taphttp-requests.md` instead of working around it here.

## Testing

TDD, same as the rest of the suite:

- Write the test first. No behavior ships without a test that would
  catch its regression.
- Unit-test non-trivial logic in its module (`#[cfg(test)] mod tests`).
- Integration-test user-visible workflows.
- `cargo test` green and `cargo clippy -- -D warnings` clean before
  committing.
- No `.unwrap()` / `.expect()` in non-test code.

## Provenance

Built with Claude Code, human-directed and reviewed. Stated openly in
the README, not hidden.
