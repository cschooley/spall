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
current milestone. Before adding any agent capability, read its *Building
blocks* section: only primitives send, text (skills, roles, checks) never
grants, and its five questions decide whether something is a primitive. Read `docs/ux.md` before building any UI: spall is
object-centric (exchanges, endpoints, tasks, findings), not a set of
Burp-style tool tabs. Read `docs/flux/SPEC.md` (vendored from the flux
repo) before touching findings.

UI styling comes from verdigris (`cschooley/verdigris`): use its
`tokens.css` variables, never hard-coded colors, and treat its
`mockups/spall-*.html` as the visual target. Tokens and icons are
vendored (copied, pinned to a verdigris tag, never edited here) in
`ui/src/vendor/verdigris/`; see its `SOURCE.md` to re-vendor.

## Hard rules

- **No findings format of our own.** Findings are flux: use the
  `flux` crate's types (cschooley/flux); do not define a local
  `Finding`, `Severity`, or `Evidence` type, nor a lookalike under
  another name.
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

**M2 "replay": done** (tag `v0.3.0`, #17-#22). Replay from the
inspector (`⏎`) and console, edit before replay, compare with the
original (`ui/src/lib/diff.ts`, reused by M4 evidence and M8 retests),
the Replays tab. Every send goes through the action gate (`src/gate.rs`:
scope, rules, confirmation; `src/confirm.rs` single-use tokens;
`src/audit.rs` JSONL; `src/replay.rs` the chain), behind the same-origin
guard (`src/csrf.rs`). Scope and policy changes are `policy` risk:
never MCP.

**M3 "engagement": done** (tag `v0.4.0`, #30: #32, #34-#40). The
engagement is the root object (`src/engagement.rs`, one JSON file each
in `--engagements`); scope with exclusions (`src/gate.rs` `Scope`);
rules of engagement the gate enforces (`src/roe.rs`: windows, blackouts,
rate, methods, agents, destructive; layers only narrow); paste a scope
list (`src/scope_list.rs`), hosts from traffic (`src/hosts.rs`), OpenAPI
import (`src/openapi.rs`), CA setup from taphttp's `/info` and
`/ca.pem`; the Engagement view (`#engagement`, key `1`). #33
(scope-limited CA) moved to `later`.

taphttp: v0.7.0 (`taphttp-types` 0.6.0, schema v6: every entry has
`origin`, `proxy|replay|send`). `POST /replay` takes
`{id, method?, req_headers?, headers?, body?, body_encoding?, meta?}`;
`req_headers` is the whole ordered list; `meta` is attribution stored on
the recorded replay, never sent upstream (spall sends `{"actor":
"human"}`; M6a agents add a task id). A failed attempt is recorded with
`error` and returned as a 502; a 400 means nothing was sent. `GET
/exchanges?replay_of=<id>` lists replays. `GET /info`, `/ca.pem`,
`/ca.der` give the public CA. `POST /send` is a new request
(`{method?, url, req_headers?, body?, body_encoding?, meta?}`, the same
answers); `/send` and `/replay` refuse taphttp's own listeners. Recorded
`req_headers` are what went on the wire: hyper groups a repeated header
name at its first position.

**M4 "candidates": done** (tag `v0.5.0`, #48: #41-#47, #16). Rule
catalog (`src/rules.rs`: 74 WSTG v4.2 tests mapped to CWE, plus
`spall/other`); candidates as `flux::Finding`s per engagement
(`src/candidates.rs`, `<engagements>/candidates/<id>.json`; flag, 409 on
a matching fingerprint, triage ops, audited as `CandidateChanged`);
flag from the inspector (`f`, `FlagForm.tsx`) and console (`flag`,
risk `record`, never MCP); the Findings view (`FindingsView.tsx`,
`lib/findings.ts`: keyboard triage `c`/`r`/`a`/`d`/`o`); evidence
(`Evidence.tsx`, compare via `lib/diff.ts`); "waiting on you"; the
launcher's taphttp and flux details. Browser tests mock candidates with
`ui/e2e/candidates-api.ts`.

**M5 "export": done** (tag `v0.6.0`, #52: #49-#51). Redaction lives in
the flux crate (v0.2.1, `flux::redact`; spec draft v0.5 has the exact
table); spall promotes (`{"op": "promote"}`, a human's call, risk
`export`, never MCP), previews redaction per candidate
(`/api/candidates/{id}/redaction`), and exports the promoted ones as a
redacted, validated flux report (`POST /api/export`,
`candidates::export`). Unredacted only with `--allow-unredacted-export`
and an explicit ask, marked `flux/redacted: false`. The inspector's
credential highlighting comes from flux (`/api/redaction`).

## Current milestone: M6b "agent host"

**M6a "agent over MCP": done** (tag `v0.7.0`, #68: #53-#62, #73,
#74, #78). Its notes stay below: M6b builds on all of it.

M6 is split in three (2026-11-23), lettered so M7 onward keep their
numbers. Read docs/architecture.md (agent sections) and docs/ux.md
(Tasks, intake, roles) first.

- **M6a agent over MCP** (#68, done): #53 server-side registry
  (first: the UI registry is TS-only), #54 tasks, #55 gate for agents,
  #56 approvals, #57 e-stop, #58 MCP server, #59 untrusted tool output,
  #60 `propose_finding`, #61 Tasks view, #62 attribution (taphttp
  `meta`). Never promote or export over MCP.
- **M6b agent host** (#69, current): #63 host (done), #81 task report (done), #82 candidate notes (done), #64 roles (done), #67 buttons (done), #65 intake, #66 quad,
  #67 buttons.
- **M6c sensors** (#70): #13 sandbox, #1 OOB listener, #2 inboxes.

The command registry is server-side (#53, done): `src/commands.rs`
holds name, usage, argument schema, risk class and `mcp` for every
command, served at `GET /api/commands`. The UI's `registry.ts` adds
only `run`/`complete`/palette and takes the rest from the generated
`ui/src/commands/specs.gen.ts` (never edit it; regenerate with
`SPALL_BLESS=1 cargo test --lib commands::tests::ui_specs_are_current`).
A new command goes in both, or a test fails. Agents run commands only
through `commands::admit` / `admit_audited`, which refuses `policy` and
`export` whatever the `mcp` flag says.

Tasks (#54, done): `src/tasks.rs`, stored at
`<engagements>/tasks/<engagement>.json`. Constraints are resolved
against the RoE at create/edit and anything wider is refused, not
trimmed; the gate (#55) must still check `Constraints::layer` with
`roe::check_layers` and `allows_host`, since the RoE can narrow later.
Humans get create/approve/edit-draft/halt over `/api/tasks`; the machine
moves (`transition`), `log` and `charge` (budget, one lock) are
server-internal for #55/#58.

**Halted is forever** (decided 2026-11-23): an e-stop, a human halt or a
stop condition ends the task for good; it is never resumed or retried,
and the work is given again as a new task. Don't add a way out of
`halted`.

Agents through the gate (#55, done): `Replayer::agent_replay(task, req)`
and `agent_fetch` (#74, a new GET/HEAD via taphttp `/send`; both share
`gated_send`) are the only agent send paths and have no HTTP route (tests reach them via
`spall::build` + `App::run`). Outside the RoE or scope denies; outside
the task (host, method, unsafe send, time limit, budget) pauses the task
to `waiting_on_approval` with the exact request in `Task::pending`.
Agents never get or use a confirm token. Before any of that, `src/stack.rs`
refuses an agent target that is the stack itself (#73: spall, taphttp's
proxy and lever API, by name on any port or as this machine on stack ports);
not approvable, whatever the scope. The budget is charged under the
task store's lock just before sending; taphttp `meta` is
`{"actor":"agent","task":"T-n"}`; audit records carry `task`.

E-stop (#57, done): `tasks::Store::engage` raises a flag first (checked
by `charge` under the book lock, so nothing uncharged is sent after it),
saves `<engagements>/tasks/estop` (unreadable = still engaged), then
halts every unfinished task in every engagement. While engaged, nothing
is approved or started; `clear_estop` only lets new tasks run. UI: the
top-bar button (two presses to clear), Shift+Esc (engage only), the halt
banner, console `estop [reason] | estop clear`. `/api/estop`,
`/api/estop/clear`, `estop` in `/api/status`. #63 (model loops), #13
(sandboxes) and #66 (quad) must stop when `Store::estop()` is set.

Approvals (#56, done): a paused task holds `Pending { ask, fingerprint }`
(SHA-256 of taphttp's replay body). `PATCH /api/tasks/{id}` takes
`approve_once` (a `Grant` for that fingerprint, 10 min, used up by
`charge_granted` under the lock; never given to the agent),
`approve_for_task` (`Constraints::widened` then `resolve`: never past the
RoE; an RoE confirmation is once only) and `deny {reason}` (resumes the
task with the reason). Grants die with a finished task. UI: approval
cards in Engagement's Waiting on you, `N waiting on approval` in the
status bar (`waiting_approval` in `/api/status`).

MCP (#58, #59, #60, done): hand-rolled JSON-RPC, no SDK. `src/mcp.rs`
serves streamable HTTP at `/mcp` (bearer token from `--mcp-token-file`,
created 0600; `csrf::check_mcp` allows a headless client but refuses a
browser's cross-site request); `src/mcp_bridge.rs` is `spall mcp`, a stdio
bridge that forwards to the running spall (one process owns the stores).
Sessions bind to `?task=T-n` or make a draft. Tools are
`commands::offered()` run in `src/agent.rs` (shared with #63's host): every
call goes through `admit_audited` and is logged to the task. Anything
carrying traffic goes through `src/untrusted.rs`: nonce-wrapped, labeled,
4 KiB bodies by default, credentials masked with flux's redaction (always,
for agents; replay by id doesn't need them). `propose_finding` needs
evidence the task sent or read (`Event::Request`/`Event::Read`), is
`flagged_by: agent:T-n`, capped at `agent::MAX_PROPOSALS` per task.
Dedupe (#78): besides the exact `flux/dast/v1` 409, `candidates::dedupe_key`
matches the rule at its granularity (`rules::Granularity`: host, endpoint,
parameter) in any state. An agent's match becomes a `Sighting` on the
original (`seen_again`; evidence, `lastDetectionTimeUtc`,
`occurrenceCount`), never a candidate. A human's warns, and `force` flags
it with `possible_duplicate_of`. An agent's `parameter` must be a name. The
registry's `ui` flag marks agent-only commands (`propose_finding`).
`tests/fixtures/injection-corpus.txt` is the injection corpus.

Tasks view and attribution (#61, #62, done): `ui/src/components/TasksView.tsx`
(key 4) polls `/api/tasks` every 2 s while open (the SSE stream carries
traffic only). Humans add steering with `PATCH {"op":"note"}`; an
approved task starts on its agent's first non-`task` tool call. `halt
<T-n>` and `task [T-n]` are console commands. Attribution comes from the
record: taphttp `meta` on exchanges (`ui/src/lib/attribution.ts`
`ofExchange`), `flagged_by: agent:T-n` on candidates; ◆ plus a label,
never color alone. Filters `actor:agent|you` (or `by:`) and `task:T-n`
are client-side. The Engagement view's Activity reads `/api/audit`.
Browser tests mock `/api/candidates`, `/api/tasks` and `/api/audit` by
default (log mode has none).

Engagement is the home view (#71, done in M6a): a bare URL opens it.
Browser tests that exercise Traffic open `/#traffic`.

Agent host (#63, done): `src/model.rs` holds the key (`Key`: 0600 file or
`SPALL_MODEL_KEY`, never printed, scrubbed from errors) and one client per
provider: Anthropic Messages API (raw HTTP, `anthropic-version`
2026-10-21, top-level `cache_control`) and OpenAI-compatible chat
completions (decided 2026-11-24: both, so a local model keeps traffic on
the machine). HTTPS is hyper-rustls on ring with webpki roots (the image
is FROM scratch). A `Conversation` stays in the provider's wire format and
is append-only: thinking blocks go back verbatim, and a task's system
prompt and tools never change mid-conversation. `src/host.rs` drives tasks
with `runner: spall` through `agent::Tools::call` (the same path as MCP; no
side door): it parks while waiting on approval and passes the decision and
notes as news, ends `done` when the model stops calling tools, and drops a
model call in flight on halt or e-stop. Each call is charged with
`tasks::Store::charge_model` (`usage`; past `constraints.tokens` the task
halts). Turns go in the log (4,000 characters) and the whole conversation
in `<engagements>/tasks/transcripts/<eng>/<T-n>.json`. Tests drive it with
a scripted `model::Model` via `App::with_model` (tests/lever.rs).

Roles (#64, done): `src/roles.rs` is the permission set (`Role::risks`,
`may_run`, `may_send`, `may_destroy`); `src/content/roles/*.md` and
`src/content/skills/*/SKILL.md` are compiled-in text (prompt, skills),
whose frontmatter a test holds to the code. `agent::Tools::call` checks
the role after `admit` (refusals audited); `gated_send` refuses a send the
role can't make before any pause or grant; `tasks::role_fits` refuses
destructive constraints for any role but tester. Agents see
`commands::offered_to(role)`; `skill` reads a skill. M6 ships built-ins;
the pack loader is #31.

Buttons (#67, done): `src/verbs.rs` (`VERBS`, `draft`) turns a verb on an
exchange or candidate into a `tasks::New` with its cost; each verb is a
`record` registry command (palette, console, MCP) and
`Replayer::verb(asked_by, …)` drafts it (drafts only; an agent's capped at
`MAX_AGENT_DRAFTS`, `asked_by: agent:T-n`), served at
`POST /api/verbs/{name}`. `retest_result` (agent, needs a replay the task
sent) is `candidates::Store::retest`: flux's retest flow (`baselineState`,
`fixed`). UI: `lib/verbs.ts` (its list is held to the registry by test),
`AgentVerbs.tsx` in the inspector and the Findings pane. PoC waits on #13.

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
  scanner version. The security scanners block (#29). Suppress in the
  repo, never only in the Security tab: OSV in `osv-scanner.toml` with a
  reason and `ignoreUntil`; semgrep with an inline `nosemgrep: <rule-id>`
  and a reason beside it; code-scanning dismissals get a comment.
- New npm or Rust dependencies: versions at least two weeks old,
  transitive ones included (`npm install --before=<date>`;
  `cargo update --precise`).
- No `.unwrap()` / `.expect()` in non-test code.

## Provenance

Built with Claude Code, human-directed and reviewed. Stated openly in
the README, not hidden.
