# flux: a SARIF 2.1.0 profile for Kudzu

Status: draft v0.5 (Nov 2026). The `flux` crate v0.2.1 implements it (types, fingerprints, validation, redaction).

## What flux is (and isn't)

flux is **not a new schema**. It is a profile of [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html) (OASIS): a set of conventions on which fields are required, how fingerprints are computed, and which property-bag keys the Kudzu tools use. A flux document is a valid SARIF document. Any SARIF consumer can read it; flux-aware tools get extra guarantees.

Producers and consumers:

| Tool | Role |
|---|---|
| scurf (Rust SAST) | produces |
| spall (Rust DAST; SPA + MCP assessment workbench on top of taphttp) | produces and consumes |
| taphttp (Rust MITM proxy) | none directly; supplies traffic and replay IDs that spall cites as evidence |
| sarifview (VS Code) | consumes / merges |
| sec-actions, ghas-actions | produce / gate on |
| Kudzu platform | consumes (system of record), produces (retest requests, known findings) |

Kudzu may use any internal model it likes. flux is the interchange format, not a database schema.

## Rules

1. **Valid SARIF 2.1.0.** Validate against the official JSON schema. No top-level extensions.
2. **Custom data only in property bags**, under the `flux/` key prefix (e.g. `properties["flux/status"]`).
3. **Standard fields first.** If SARIF has a field for it, use the field, not a property.

## Field mapping

| Concept | SARIF field | flux convention |
|---|---|---|
| Vuln class | `result.ruleId`, `run.tool.driver.rules[]` | Rule IDs stable per tool |
| CWE / CAPEC | `run.taxonomies[]`, `result.taxa[]` | CWE taxonomy required where applicable: `taxa` entry `{"id": "89", "toolComponent": {"name": "CWE"}}`, and the run declares a taxonomy named `CWE`. A rule's CWE goes in its `relationships[]` the same way |
| Severity | `result.level` | `error` / `warning` / `note` |
| Confidence / priority | `result.rank` (0-100) | Tool-reported confidence |
| Dedup key | `result.fingerprints` | Required. See Fingerprints |
| Same issue across runs | `result.correlationGuid` | Assigned on first confirmation, preserved after |
| Run-over-run diff | `result.baselineState` | `new` / `unchanged` / `updated` / `absent` |
| Triage: rejected, accepted risk | `result.suppressions[]` | `kind: external`, `status` + `justification` required; `flux/disposition` says which |
| Duplicate | `result.suppressions[]` | `flux/disposition: duplicate`, `flux/duplicateOf` = the original's `correlationGuid`, which the `justification` also names |
| Tracked ticket | `result.workItemUris[]` | Kudzu / Jira / GitHub issue URIs |
| First/last seen | `result.provenance` | `firstDetectionTimeUtc`, `lastDetectionTimeUtc` |
| SAST code location | `result.locations[].physicalLocation` | File + region |
| DAST endpoint | `result.locations[]` | `physicalLocation.artifactLocation.uri` = URL (query and fragment stripped); method in `logicalLocations` as `{"name": "GET", "kind": "httpMethod"}` (uppercase) |
| SAST location on a DAST-confirmed finding | `result.relatedLocations[]` | Preserved when DAST confirms a SAST hypothesis |
| HTTP evidence | `result.webRequest`, `result.webResponse` | See Evidence |
| Data flow | `result.codeFlows[]` | SAST optional |

## flux property-bag keys

| Key | Type | Meaning |
|---|---|---|
| `flux/status` | `candidate` \| `suspected` \| `confirmed` \| `fixed` | Verification state (orthogonal to suppression) |
| `flux/confirmedBy` | string[] | Tools that confirmed, e.g. `["scurf", "spall"]` |
| `flux/replayIds` | string[] | taphttp exchange IDs (captured and replayed traffic) backing the evidence |
| `flux/redacted` | boolean | Evidence has been scrubbed (see Redaction) |
| `flux/hypothesisFor` | string | On spall output: `correlationGuid` of the ingested finding it tested |
| `flux/fingerprintVersion` | string | Fingerprint algorithm version |
| `flux/traceId` | string | OpenTelemetry trace ID linking a DAST request to runtime code locations |

On a `suppression`'s property bag:

| Key | Type | Meaning |
|---|---|---|
| `flux/disposition` | `falsePositive` \| `acceptedRisk` \| `duplicate` | Why a human suppressed it (triage: reject, accept the risk, duplicate) |
| `flux/duplicateOf` | string | For `duplicate`: the original's `correlationGuid` |

A suppression is in effect when its `status` is `accepted` or absent (SARIF's default); `underReview` is a proposal, `rejected` a refused one.

`flux/status` is about whether the issue is real. `suppressions` is about what a human decided to do with it. A finding can be `confirmed` and suppressed (accepted risk).

## Fingerprints

Every result carries `fingerprints` with at least one flux key:

- **DAST:** `flux/dast/v1` = SHA-256 of `host | method | normalized path | parameter name | ruleId`
  - Host: lowercased, as in the URL (no userinfo; omit a default port). Method: uppercased. Parameter: its name, or empty when there is none.
  - Normalized path: query string and fragment removed; segments of only digits, and UUIDs (8-4-4-4-12 hex), replaced with `{id}`; trailing slashes removed; a leading `/` added if missing (the root is `/`). Case and percent-encoding are kept.
  - The parts are joined with `|` and hashed as UTF-8; the value is lowercase hex.
- **SAST:** `flux/sast/v1` = SHA-256 of `repo | file path | function/symbol | ruleId` (avoid line numbers; they drift). Same joining and encoding.

The crate's tests pin exact outputs for known inputs.

Changing the algorithm means a new version suffix. Consumers match on any shared key.

## Evidence (DAST)

- Request/response pairs use SARIF `webRequest` / `webResponse`.
- Large or multiple exchanges: include the decisive pair inline, reference the rest via `flux/replayIds`.
- Evidence is appended, never replaced, as findings move from `suspected` to `confirmed`.

## Redaction

Required before any flux document leaves the producing tool's local store (export, promotion, upload):

- Scrub `Authorization`, `Cookie`, `Set-Cookie`, `Proxy-Authorization`, and token-shaped values (JWTs, bearer tokens, API keys by pattern) in headers, bodies, and URLs.
- Replace with `[REDACTED:<kind>]`.
- Set `flux/redacted: true`.
- Unredacted export requires an explicit opt-in flag and leaves `flux/redacted: false` so consumers can refuse it.

Exactly what the crate's `redact` module does (the definition; v0.5):

| What | Where | Kind |
|---|---|---|
| The whole value of `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, `Api-Key`, `X-Auth-Token`, `X-Access-Token` | request and response headers | `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `api-key`, `token` |
| JWTs (`eyJ…` with three dot-separated base64url parts) | everywhere below | `jwt` |
| `Bearer <token>` (8+ token characters; the scheme is kept) | everywhere below | `bearer` |
| API keys by format: AWS (`AKIA`, `ASIA`), GitHub (`ghp_` …, `github_pat_`), GitLab (`glpat-`), Stripe (`sk_live_`, `sk_test_`, `rk_live_`), Slack (`xox?-`), Google (`AIza`) | everywhere below | `api-key` |
| The password in `scheme://user:password@host` | everywhere below | `password` |
| The value of a sensitive name in `name=value` and JSON `"name": value`: password, passwd, pwd, pass, passphrase (`password`); secret, client_secret, token, access_token, refresh_token, id_token, auth_token, session, sessionid, sid, private_key (`token`); api_key, api_secret (`api-key`). Names compare case-insensitively, ignoring `-` and `_`. | everywhere below | as listed |
| A binary (base64) body, which can't be inspected: dropped | `webRequest.body`, `webResponse.body` | `binary` |

"Everywhere": other header values, text bodies, `webRequest.target`, `message.text`, and location URIs and messages. Fields a tool keeps outside this profile (unknown properties) are not inspected, so producers must not put secrets there. Redaction is idempotent; a document marked `flux/redacted: true` that still has something to redact fails validation.

## Flows

**Promotion (spall to Kudzu):** results with `flux/status` in `suspected` / `confirmed`, unsuppressed, redacted.

**Hypotheses (scurf to spall):** SAST results with `flux/status: suspected`. spall tests them and emits a result with the same `correlationGuid`, `flux/hypothesisFor` set, the SAST location moved to `relatedLocations`, and status `confirmed` or left `suspected` with a note in `result.message`.

**Retest (Kudzu to spall):** open findings out, results back with `baselineState` (`unchanged` = still vulnerable, `absent` = not reproduced) and `flux/status: fixed` where appropriate.

**Known findings / suppressions (Kudzu to tools):** consumed so producers can badge results as tracked (`workItemUris`) or previously rejected (`suppressions`) instead of re-raising them.

## Route maps (source maps for the backend)

A **route map** links HTTP endpoints to the code that serves them, the way a browser source map links minified JavaScript to its source. scurf produces it from source; spall consumes it to put source locations on DAST findings and to find untested routes.

**Format: OpenAPI 3.1** (so any OpenAPI tool can read it), with flux extensions:

```yaml
openapi: 3.1.0
info: { title: shop-api route map, version: "a91f03c" }
x-flux-source:
  repo: github.com/example/shop-api
  commit: a91f03c2e4...          # full SHA the map was built from
  generated-by: scurf 0.1.0
  generated-at: 2026-11-21T09:00:00Z
paths:
  /api/v2/session:
    post:
      operationId: create_session
      x-flux-source:
        file: src/auth/session.py
        line: 88
        function: create_session
        framework: fastapi
        auth: none                # what the route declares, if detectable
```

Rules:
- **Commit-pinned.** Every map records the exact commit. Consumers compare it to what's deployed when they can and warn on mismatch ("map is from a91f03c; staging runs 7d2e19b").
- **Path templates, not examples:** `/v1/cart/{id}`, matching the framework's own route definition.
- **Locations, not code.** The map carries `file`, `line`, `function`. Code snippets are fetched locally by the consumer when needed and are never part of the map or of flux export by default.

**How DAST results use it:** when a DAST result's endpoint matches a route, the source location goes in `result.relatedLocations[]` with a `message` of `"handler"`, the same slot used when DAST confirms a SAST hypothesis. A finding confirmed at runtime via tracing (see below) may also add `"reached"` locations.

**Optional runtime evidence:** where the target runs OpenTelemetry and the tester has access to its traces, a request carrying a `traceparent` header can be matched to spans whose `code.filepath` / `code.lineno` / `code.function` attributes show exactly which code the payload reached. Those locations are added as `relatedLocations` with message `"reached"`, plus `properties["flux/traceId"]`.

## The flux crate

flux is also a Rust library, so every Rust tool in Kudzu shares one set of finding types instead of each defining its own.

- **One `Finding` type for Kudzu.** spall and scurf (and the Kudzu platform, if it is Rust) depend on `flux`; none define their own `Finding`, `Severity`, or `Evidence`.
- **Types model this profile, not all of SARIF.** Fields map to the table above; `flux/` properties are typed fields, not a loose map.
- **SARIF is the wire format.** `Finding` <-> SARIF `result`, `Report` <-> SARIF `log`, via serde. The types are hand-written for this profile's subset; SARIF fields they don't model are kept as they were, so reading and writing a document loses nothing. (`serde-sarif` was evaluated, Nov 2026: it generates all of SARIF at build time, with `schemafy`, `syn` and `typed-builder`, too heavy for a small core, and had no release since May 2025.)
- **Fingerprint functions live here** (`flux/dast/v1`, `flux/sast/v1`), so every producer computes them identically.
- **Redaction lives here**, so no producer can export without it.
- **Validation lives here**: SARIF schema + flux rules, exposed as a function and a small CLI.
- **Adapter traits** for converting to and from other formats. Core stays dependency-light; format-specific adapters (OCSF export, third-party SARIF from Semgrep/Trivy/etc., later STIX / VEX / CycloneDX) are separate crates or feature flags, never core dependencies.

## Optional exporters

- **OCSF** (Vulnerability Finding class) for SIEM / security data lake consumers. A one-way export, not interchange.

## Repo contents

- `SPEC.md`: this document
- `src/`: the crate (`Finding`, `Report`, `fingerprint`, `validate`)
- `schema/sarif-schema-2.1.0.json`: the official SARIF 2.1.0 schema (OASIS, errata 01), used by the tests
- `examples/`: golden files, one per flow: DAST candidate, SAST hypothesis, SAST confirmed by DAST, suppressed duplicate, retest result

Planned: redaction; a validator CLI; `schema/flux-properties.schema.json` for the `flux/` property bag.
