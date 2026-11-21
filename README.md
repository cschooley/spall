# spall

[![CI](https://github.com/cschooley/spall/actions/workflows/ci.yml/badge.svg)](https://github.com/cschooley/spall/actions/workflows/ci.yml)
[![Security](https://github.com/cschooley/spall/actions/workflows/security.yml/badge.svg)](https://github.com/cschooley/spall/actions/workflows/security.yml)

An assessment workbench for web application security testing: a web UI for interactive testing and an MCP server for agentic testing, built on top of [taphttp](https://github.com/cschooley/taphttp). Findings are exported as [flux](https://github.com/cschooley/flux), a SARIF 2.1.0 profile.

**Status:** pre-alpha, v0.2.0. M1 (traffic workbench) is done; M2 (replay) is next. See [Roadmap](#roadmap) and [docs/architecture.md](docs/architecture.md).

**For authorized testing and local development only.** spall displays raw captured traffic, including credentials. It binds to localhost by default; keep it that way unless you have added authentication.

## Screenshots

### Today: the Traffic view (M1)

Live traffic and history from taphttp, filter syntax, the shared inspector, command palette and the `` ` `` console. Screenshots use fixture traffic; regenerate them with `npm --prefix ui run screenshots`.

![spall Traffic view](docs/screenshots/traffic.png)

![spall console](docs/screenshots/traffic-console.png)

### Where it's going (design mockups)

Visual direction from [verdigris](https://github.com/cschooley/verdigris), Kudzu's design system. The interactive versions live in [`verdigris/mockups`](https://github.com/cschooley/verdigris/tree/main/mockups); the UX behind them is in [docs/ux.md](docs/ux.md).

**Traffic.** Still to come: attribution on every exchange (you or the agent), and replay and flag actions in the inspector.

![Mockup: Traffic view](docs/screenshots/mockup-traffic.png)

**Tasks.** The agent works from structured work orders: goal, constraints, request budget, plan and a linked request log. Out-of-scope actions wait for approval. STOP AGENT halts everything.

![Mockup: Tasks view](docs/screenshots/mockup-tasks.png)

**Findings.** A triage inbox. The agent proposes, a human confirms, rejects or marks duplicate, with evidence and a redacted export preview.

![Mockup: Findings view](docs/screenshots/mockup-findings.png)

## Quickstart

```sh
docker compose up --build
```

taphttp comes from its published image (`ghcr.io/cschooley/taphttp:0.3.0`, amd64 only for now; no login needed). The first `up` compiles spall, which takes a few minutes. Then, in another terminal:

```sh
curl -x http://127.0.0.1:8080 http://example.com
```

Open <http://127.0.0.1:3000> and the request appears in the Traffic view. Click a row (or press `j`) to open it in the inspector.

Ports:

| Port | Service | Bound to |
|---|---|---|
| 8080 | taphttp HTTP proxy | 127.0.0.1 |
| 3000 | spall web UI | 127.0.0.1 |

If either port is already in use, override the host side: `TAPHTTP_PORT=18080 SPALL_PORT=13000 docker compose up`. They stay bound to 127.0.0.1.

### HTTPS

taphttp terminates TLS using its own CA, so clients have to trust that CA. The CA lives in the `taphttp-ca` volume and survives restarts.

The proper way is to export the CA and trust it explicitly:

```sh
docker compose exec taphttp taphttp ca info              # where the cert is, install hints
docker compose exec taphttp taphttp ca print > taphttp-ca.pem
curl -x http://127.0.0.1:8080 --cacert taphttp-ca.pem https://example.com
```

For a quick demo only, you can skip verification:

```sh
curl -x http://127.0.0.1:8080 -k https://example.com
```

**`-k` is for this demo only.** It turns off certificate checking for the whole request. Never use it against anything you care about, and never put it in scripts.

## Using it

- **Filter** (`/`): `host:example.com`, `method:POST`, `status:404` or `status:4xx`, plus free text matched against URL and host. Filters search taphttp's full history, not just what's on screen; scroll to the top for older pages.
- **Inspector**: request and response tabs, headers in wire order (repeats numbered `n/N`), bodies (JSON pretty-printed; binary as type, size, sha256 and a hex view). Credential headers are highlighted and shortened until you click "show". Copy as curl, save a body to a file.
- **Keys**: `?` lists them all. `j` / `k` move, `Esc` closes the inspector, `t` toggles light/dark, `x` shows hex.
- **Command palette** (`Ctrl K`, `⌘K` on macOS): find a command you don't remember.
- **Console** (`` ` ``): type commands you do remember (`help` lists them: `filter`, `open <id>`, `curl`, `theme`, …), with history and tab completion. System messages (reconnects, taphttp going away) land here too. The palette and console run the same commands, and so will the MCP server.
- **Suite launcher** (top right): Kudzu's other tools. Ones not built yet are shown with their light off.

Everything shown comes from captured traffic, so it's treated as hostile: rendered as text only, never as HTML, under a strict Content-Security-Policy.

## Configuration

### Where traffic comes from

In compose, spall reads traffic from taphttp's **lever API**. It backfills recent history from `GET /exchanges`, then follows the `/feed` WebSocket, resuming from the last exchange it saw after any disconnect. The API port (8081) stays on the compose network, and spall reads the API token from a read-only `taphttp-token` volume. spall never mounts taphttp's traffic or CA volumes.

taphttp's API history starts when its sqlite store is turned on, so traffic captured before that (for example, by taphttp before v0.3.0) isn't in it.

### Running without Docker

With the lever API (recommended):

```sh
taphttp start --api-listen 127.0.0.1:8081 --api-token-file ./api-token &
cargo run -- --taphttp-api http://127.0.0.1:8081 --taphttp-token-file ./api-token
```

Or tail taphttp's log file (no history search):

```sh
taphttp start --log-file ./traffic.jsonl &
cargo run -- --log-file ./traffic.jsonl
```

| Flag | Env | Default |
|---|---|---|
| `--taphttp-api` | `SPALL_TAPHTTP_API` | unset (tail `--log-file` instead). `http://host:port` only |
| `--taphttp-token-file` | `SPALL_TAPHTTP_TOKEN_FILE` | `/token/api-token` |
| `--log-file` | `SPALL_LOG_FILE` | `/data/traffic.jsonl` (used only without `--taphttp-api`) |
| `--listen` | `SPALL_LISTEN` | `127.0.0.1:3000` |
| `--proxy-address` | `SPALL_PROXY_ADDRESS` | `127.0.0.1:8080` (shown in the status bar; display only) |
| `--allowed-host` | `SPALL_ALLOWED_HOSTS` | `localhost,127.0.0.1,[::1]` |
| `--history` | `SPALL_HISTORY` | `1000` (entries replayed to a newly opened page) |
| `--poll-ms` | `SPALL_POLL_MS` | `250` (log tailing only) |

`--allowed-host` is checked against the request's `Host` header to block DNS-rebinding attacks on the local console. Requests for any other host get a 403.

## Endpoints

- `GET /`: live console
- `GET /events`: Server-Sent Events, one event per taphttp `TrafficEntry` (JSON); recent history first, then live
- `GET /api/exchanges`: history, newest first, proxied to taphttp's `GET /exchanges`. Accepts `limit`, `before`, `host`, `method`, `status`, `q`, `since`, `until`; other parameters are dropped. 503 without `--taphttp-api`
- `GET /api/exchanges/{id}`: one exchange, or 404
- `GET /api/status`: `{"source": "lever" | "log", "connected": bool | null, "proxy": "127.0.0.1:8080"}` for the status bar
- `GET /healthz`: `ok`

The browser never talks to taphttp or sees its token: spall adds the token server-side.

## Development

The server is Rust; the UI is React + Vite + TypeScript in `ui/`, built to `ui/dist/` and embedded in the binary. `cargo build` and `cargo test` work without Node: `/` then explains how to build the UI.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

UI (Node 22.16, see `ui/.nvmrc`):

```sh
cd ui
npm ci               # lockfile only; install scripts are disabled (.npmrc)
npm run lint         # includes the "traffic is never HTML" bans
npm test             # node:test, no browser
npm run test:e2e     # Playwright: builds the UI, runs spall via cargo, headless Chromium
npm run screenshots  # regenerate docs/screenshots/traffic*.png (1440x820, fixture traffic)
npm run build        # then rebuild spall to embed ui/dist
npm run dev          # Vite on 127.0.0.1:5173, proxying /events, /api and /healthz to spall on :3000
```

Browser tests need a Chromium: either Playwright's (`npx playwright install chromium`, about 100 MB; `ignore-scripts` stops the automatic download), or any recent one via `SPALL_E2E_CHROMIUM=/usr/bin/chromium npm run test:e2e`. Every test fails on a console error, page error, dialog or CSP violation.

Adding an npm dependency: pick a version published at least two weeks ago, and resolve the whole tree as of a date at least two weeks back (`npm install --before=<date>`), so transitive dependencies follow the same rule.

### CI and security

- **CI** (`.github/workflows/ci.yml`, every push and PR): Rust fmt, clippy and tests; UI lint, unit tests and build with the CSP check; Playwright; the Docker image.
- **Security** (`.github/workflows/security.yml`, every push and PR plus weekly): gitleaks, semgrep, OSV-Scanner on both lockfiles, a syft SBOM with a license check, and grype on the SBOM and on the built image. These come from [sec-actions](https://github.com/cschooley/sec-actions) and run in audit mode for now. Findings appear in the repo's Security tab. Suppressions need a reason and an expiry date (`osv-scanner.toml`).
- **GitHub-native:** CodeQL (default setup), secret scanning with push protection, and Dependabot alerts and security updates.
- **Dependabot** (`.github/dependabot.yml`) keeps action pins and dependencies current, with a 14-day cooldown matching the two-week rule. `taphttp-types` and the taphttp image are bumped together by hand.

Every action is pinned to a commit SHA (the repository requires it: a workflow using `@v4` or `@main` won't run), and scanner versions are pinned too. Write a new step as `uses: owner/action@<40-char SHA> # vX.Y.Z`.

## Roadmap

| Milestone | Scope | Status |
|---|---|---|
| M0 hello | Docker Compose with taphttp; live traffic console over SSE | done, `v0.1.0` |
| M1 traffic workbench | Traffic view: history and search from taphttp's API, filters, shared inspector, command palette and console, verdigris styling | done, `v0.2.0` |
| M2 replay | Replay from the inspector and console via taphttp; results linked to the original; server-side action gate (scope, method policy, audit) | current |
| M3 engagement | Targets, scope and rules of engagement as data; build scope from traffic; Engagement home view | |
| M4 candidates | Flag candidates, triage, suppressions, on flux types | |
| M5 export | flux export with redaction and fingerprints | |
| M6 agent | Agent tasks over MCP: read traffic, replay, propose findings (never promote); e-stop, quad; execution sandbox and OOB listener | |
| M7 intercept | Pause and modify requests via taphttp | |
| M8 flux in | Suppressions and retests from the Kudzu platform; scurf hypotheses; route maps (Surface) | |

Details in [docs/architecture.md](docs/architecture.md) (milestones) and [docs/ux.md](docs/ux.md) (views).

## Provenance

Built with Claude Code, human-directed and reviewed.
