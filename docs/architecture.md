# spall architecture

Status: design intent (Nov 2026). Milestones below M0 are direction, not spec.

## Where spall sits

```
browser / agent traffic -> taphttp (capture, log, replay, intercept hook)
                                |  JSON lines / sqlite, replay, unix socket
                                v
                     spall (web UI, MCP server, triage)
                                |  flux (SARIF 2.1.0 profile)
                                v
                 Kudzu / sarifview / any SARIF consumer

scurf (SAST) --flux hypotheses--> spall
Kudzu --flux retest requests--> spall
```

- **taphttp** is the proxy. Unix philosophy: capture, log, replay, nothing else. spall never reimplements it.
- **flux** is the shared findings layer: a SARIF 2.1.0 profile (spec) and a Rust crate (types, fingerprints, redaction, validation).
- **Kudzu** is the system of record for findings. spall never talks to Kudzu directly.

A Burp-style thick client was considered and rejected. spall is a web UI plus an MCP server.

## Two tiers: candidates vs findings

| | spall (workbench) | Kudzu (system of record) |
|---|---|---|
| Holds | Candidates | Findings |
| Lifetime | Per engagement | Durable |
| Quality | Noisy, unverified | Triaged |
| Lifecycle | Triage only | Full (ticketing, SLA, retest, closure) |

Candidates live in their own Findings view (a triage inbox), not in a side panel beside Traffic. Traffic links to it through a "Flag candidate" action in the shared inspector (the right-hand pane in every view). See [ux.md](ux.md). Only triaged candidates are exported.

Triage in flux terms:

- New: `flux/status: candidate`
- Promoted: `flux/status: suspected` or `confirmed`, exported
- Rejected: `suppressions[]` with `status: rejected` + `justification`
- Duplicate: `suppressions[]`, `justification` references the original `correlationGuid`

Rejections and duplicates are kept as suppressions so the same false positive is not raised again.

## Agent autonomy boundary (MCP)

- `propose_finding`: available to the agent. Creates a candidate.
- `promote_finding`: not exposed to the agent by default. Requires a human in the web UI.
- Later, optional: explicit opt-in auto-promotion rules (e.g. replay-verified + allowlisted rule IDs). Never the default.

Promotion flows toward Jira. An agent must not file tickets against dev teams unsupervised.

## Evidence

- Every candidate cites the taphttp exchange IDs it came from (`flux/replayIds`).
- Fingerprint computed at candidate creation (`flux/dast/v1`, via the flux crate).
- Raw evidence stays local. Redaction is applied on export.

Depends on taphttp recording replayed exchanges as their own entries, linked to the original. See taphttp `docs/findings-architecture.md`.

## Flux in

1. **SAST hypotheses** from scurf: target the referenced endpoint. On confirmation keep the `correlationGuid`, move the SAST location to `relatedLocations`, append DAST evidence, set `flux/status: confirmed`.
2. **Retest** from Kudzu: replay via taphttp, emit `baselineState` (`unchanged` = still vulnerable, `absent` = not reproduced).
3. **Known findings and suppressions**: badge candidates as tracked (`workItemUris`) or previously rejected.

## Security posture of spall itself

spall holds raw captured traffic: session tokens, credentials, PII. Treat it as sensitive.

- Localhost binding by default, in the binary and in Docker port publishing.
- Auth before any non-local binding.
- MCP server: same locality rule; the agent gets read access to traffic and `propose_finding`, nothing that exports.
- No telemetry.

## Frontend (decided Nov 2026, #3)

**React + Vite + TypeScript, built as a static single-page app and embedded in the Rust binary.** No Next.js, no SSR, no Node at runtime. spall stays one binary and one container.

### Why

- **An AI writes most of it.** React has the deepest training data, so Claude Code is most reliable in it. That outweighs elegance.
- **The ecosystem matches the hard parts:** TanStack Virtual for the dense live table, cmdk for the command palette.
- **One frontend stack across the suite.** The THA casting app is already React; Kudzu's dashboard can reuse spall's components.
- Considered: Svelte 5 (lighter and nicer, but Svelte 5 is new and models mix in Svelte 4 syntax), SolidJS (small ecosystem), Leptos/Dioxus (immature for virtualized tables, doubles the Rust learning curve), htmx (fights the client-side state this UI needs).

### Rules that come with it

**Untrusted content.** Captured traffic is attacker-controlled. If it ever renders as HTML, the target app gets XSS against the tester, in the tool holding their session tokens.
- Traffic data is rendered as text only. `dangerouslySetInnerHTML` is banned (the `react/no-danger` rule, enforced with ESLint's built-in `no-restricted-syntax` because `eslint-plugin-react` doesn't support ESLint 10), as are `innerHTML` / `outerHTML` / `insertAdjacentHTML` (`no-restricted-properties`). Any exception needs a comment and review.
- Previews of rendered HTML or images, if ever, only in a sandboxed iframe from a separate origin.

**CSP from day one:** `default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`. Vite production builds need no `eval` and no inline script. No CDN: fonts and assets are bundled.

**Supply chain.** A security tool's dependency tree is part of its attack surface.
- Runtime dependency budget: `react`, `react-dom`, `@tanstack/react-virtual`, `cmdk`. Adding a runtime dependency needs a stated reason in the PR.
- Lockfile committed, `npm ci` only, install scripts disabled (`ignore-scripts=true` in `.npmrc`).
- Versions at least two weeks old, transitive ones included: resolve with `npm install --before=<date>`. A test asserts the HTML bans fire, and a post-build check rejects inline script, inline style and external URLs in `dist/index.html`.
- OSV-Scanner on the lockfile in CI (via sec-actions).

**Styling:** verdigris tokens (`--vg-*`) and plain CSS. No CSS framework on top, no CSS-in-JS runtime.

**Build and embed:** source in `ui/`, `vite build` outputs `ui/dist/`, embedded with `rust-embed`. Docker becomes multi-stage (Node builds the UI, Rust builds the binary, final image has neither toolchain). Local dev: Vite dev server proxies `/events` and the API to the running spall.

**Tests:** Playwright for browser tests (#7), headless Chromium, no Docker. Untrusted-content rendering is the first test. Pure logic (formatting, header numbering, hex dump, the entry store) is unit-tested with Node's built-in `node:test` running TypeScript directly (`--experimental-strip-types`; `erasableSyntaxOnly` keeps the code compatible). Vitest was dropped: its optional-peer tree crashes npm 10.9's resolver, and the built-in runner needs no dependencies.

## Milestones

| | Scope |
|---|---|
| **M0 hello** | Docker Compose: taphttp + spall; spall tails the JSONL log and streams it to a single-page live console over SSE. See CLAUDE.md. |
| M1 traffic workbench | Filtering, search, request/response detail view. History comes from taphttp's lever API (its sqlite store), not a spall copy; see below. |
| M2 replay | Send-to-replay from the UI via taphttp; replay results shown linked to the original. |
| M3 candidates | Candidate model on flux crate types, manual "flag as candidate", triage states, suppressions. |
| M4 export | Flux export with redaction and fingerprints. |
| M5 MCP | MCP server: read traffic, replay, `propose_finding`. No promotion. |
| M6 intercept | Pause/modify via taphttp `--intercept-socket`. |
| M7 flux in | Suppressions and retest from Kudzu; then scurf hypothesis correlation. |

Traffic history (decided Dec 2026, spall#4): spall reads history from taphttp's lever API rather than keeping its own copy of the traffic. One store, and M2 replay goes through the same API. spall has its own store only for what it owns (candidates, from M3). Built on taphttp v0.3.0 (taphttp#7: token file outside the CA volume, sqlite-backed paging and search, `/feed?since=`): spall backfills from `/exchanges`, follows `/feed`, and proxies history to the browser as `/api/exchanges`. Tailing `--log-file` remains as a fallback when no API is configured. The API port stays on the compose network and is never published to the host.

SPA framework: decided, see Frontend above. M0 stayed one static HTML file on purpose.

The UI's information architecture (five views, shared inspector, command palette, agent e-stop) is in [ux.md](ux.md), with a view-to-milestone mapping.

## History

spall was first scoped (Aug 2026) as a CLI Cargo workspace (`spall-core`, `spall-ssrf`, a `clap` binary) starting with SSRF probing, with its own `Finding` type. That shape is superseded: finding types come from flux, and the primary interface is the web UI + MCP. SSRF probing returns later as an active-check module that produces candidates.
