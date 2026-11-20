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

Candidates live in their own Findings view (a triage inbox), not in a side panel beside Traffic. Traffic links to it through a "flag as candidate" row action, and the right-hand pane in every view is the shared inspector. See [ux.md](ux.md). Only triaged candidates are exported.

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

## Milestones

| | Scope |
|---|---|
| **M0 hello** | Docker Compose: taphttp + spall; spall tails the JSONL log and streams it to a single-page live console over SSE. See CLAUDE.md. |
| M1 traffic workbench | Filtering, search, request/response detail view, sqlite-backed history instead of tailing. |
| M2 replay | Send-to-replay from the UI via taphttp; replay results shown linked to the original. |
| M3 candidates | Candidate model on flux crate types, manual "flag as candidate", triage states, suppressions. |
| M4 export | Flux export with redaction and fingerprints. |
| M5 MCP | MCP server: read traffic, replay, `propose_finding`. No promotion. |
| M6 intercept | Pause/modify via taphttp `--intercept-socket`. |
| M7 flux in | Suppressions and retest from Kudzu; then scurf hypothesis correlation. |

SPA framework choice is deferred to M1. M0 is one static HTML file on purpose.

The UI's information architecture (five views, shared inspector, command palette, agent e-stop) is in [ux.md](ux.md), with a view-to-milestone mapping.

## History

spall was first scoped (Aug 2026) as a CLI Cargo workspace (`spall-core`, `spall-ssrf`, a `clap` binary) starting with SSRF probing, with its own `Finding` type. That shape is superseded: finding types come from flux, and the primary interface is the web UI + MCP. SSRF probing returns later as an active-check module that produces candidates.
