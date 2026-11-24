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

### How an agent attaches

An LLM reached through an API doesn't speak MCP; an **agent host** (the loop that calls the model and executes its tool calls) does. Two hosts, both supported:

1. **External host.** Claude Code, Claude Desktop or any MCP client runs the loop and connects to spall's MCP server. spall never holds a model key.
2. **spall as host.** The intake and Tasks view (ux.md) run the loop inside spall: an orchestrator decomposes a request into a plan of role tasks, and spall calls the model API with the tester's key (server-side only, never in the browser). Tool definitions come straight from the command registry; no MCP hop internally. Providers (decided 2026-11-24): Anthropic's Messages API, and any OpenAI-compatible endpoint, so a local model can keep captured traffic on the tester's machine. The loop (`src/host.rs`, #63) runs tools through the same `agent::Tools::call` as MCP sessions, counts each model call on the task (tokens against a per-task cap) and stops mid-call on a halt or the e-stop.

Not supported: a model provider's API calling spall's MCP server remotely. That needs spall reachable from the internet, breaks localhost-by-default, and routes captured traffic through someone else's infrastructure.

Rules for both:

- **One registry, one gate.** Palette, console, MCP tools and built-in agent tools are generated from the same command registry (`src/commands.rs`, #53), and every state-changing call goes through the action gate (#22). No side doors. Each command's risk class maps onto the gate's (`send` and `unsafe` are sends, `destructive` is destructive); `policy` and `export` are refused to agents in the server, not only left unoffered.
- **Chat plans, tasks act.** The intake produces plans; only human-approved tasks send traffic. Plans start from an engagement (#30); with no authorized engagement, the only thing the orchestrator can draft is one.
- **Roles are least privilege.** Each role (Scoper, Mapper, Tester, Reporter; ux.md) maps to gate risk classes. A role cannot be handed work its class or the RoE forbids.
- **MCP over HTTP gets the #17 treatment.** Validate `Origin`, require a token, bind 127.0.0.1 (DNS rebinding and cross-site requests apply to it exactly as to `/api/replay`). stdio is offered too.
- **Tool output is attacker-controlled.** Exchanges returned to a model are captured traffic and a prompt-injection channel: wrapped and labeled as untrusted data, truncated, and never able to widen scope (the gate enforces that regardless).
- **Retests are a flow, not a feature.** "Re-scan last week's findings" resolves to the engagement, re-checks authorization and target ownership, then plans one retest task per finding (replay evidence, diff per #20). Results feed the flux retest flow; Kudzu closes on evidence.

Open vs. enterprise: bring-your-own-key, both hosts, roles and the gate are open. Org-wide model policy, shared agent budgets and central audit of agent runs across testers are enterprise.

## Building blocks: what's code, what's text (decided Nov 2026)

Six kinds of thing make up agent work in spall. The line between them is one question: **does the gate have to see it?** If yes, it is code, reviewed like the gate. If no, it is text, and text never grants anything. The names follow Anthropic's agent architecture (harness, tools, skills, subagents, MCP) so the same content works in Claude Code and in spall's own host (M6b).

| Block | What it is | Form | Lives in | Anthropic's name |
|---|---|---|---|---|
| **Primitive** | The only things that touch a target or the outside world: `replay`, later `fetch`, `sweep`, ... Every one goes through the gate. | Rust | `src/` (e.g. `Replayer::agent_replay`) | the side effect behind a dedicated tool |
| **Tool** | A command offered to an agent: typed arguments, a risk class. Reads (`filter`, `open`, `hosts`, `rules`, `task`), records (`propose_finding`) and the tools that drive primitives (`replay`). | Rust, one registry | `src/commands.rs` (`mcp: true`), run by `src/agent.rs` | tool (MCP tool for external hosts) |
| **Check** | What to send and how to judge the answer: a wordlist plus matchers, naming a WSTG rule. Run by a primitive (`sweep`); never sends by itself. | Data file | a vetted set in the repo; engagement-local ones imported by a human (`policy`) | none (closest: a scanner template) |
| **Rule** | What a finding is about: WSTG test to CWE. A taxonomy, not something that runs. | Rust table | `src/rules.rs` | none |
| **Skill** | How to do a kind of work: method, what to look for, which tools and checks to use, how to judge and report. | `SKILL.md` folder (frontmatter `name`, `description`; body loaded on demand; may bundle reference files) | spall-shipped and content packs (#64, shared with M9) | Agent Skill |
| **Role** | A kind of agent (Scoper, Mapper, Tester, Reporter): a prompt, the skills it loads, the tools and risk classes it may use, its model. | Markdown with frontmatter | spall-shipped; the host loads it (#64) | subagent |

Around them: the **agent host** runs the loop (Claude Code over MCP, or spall itself since #63; Anthropic's *harness*), the **task** is the agent's credentials and limits (`src/tasks.rs`), and the **gate** is the policy layer every primitive passes (Anthropic's harness hooks and permission checks play this part in Claude Code).

How they compose: a **role** takes a **task**, reads **skills** to decide how to work, and calls **tools**; tools that send run **primitives** through the gate, sometimes driven by **checks**; what it finds becomes a candidate under a **rule**, and a human promotes it.

### Invariants

- **Only primitives send.** No tool, skill, role or check reaches a target except through a primitive, and every primitive goes through the gate. No shell tool: the agent gets dedicated tools the gate can read, not an opaque command string (the sandbox, #13, contains tools; it does not hand out a shell on the host).
- **Text never grants.** A skill can say "sweep with the backup-files check"; whether that runs is the role's tools, the task's constraints, the RoE and scope. A skill or role file asking for more than that gets the same refusal as anyone.
- **Permissions come from the role, the task and the RoE, never from a skill.** A role lists risk classes; `policy` and `export` are never among them (`commands::admit`).
- **Data is bounded.** A check holds paths relative to an in-scope base, a method (GET/HEAD unless the RoE says otherwise) and simple matchers (status, size, substring, differs from baseline). No hosts, no code, no heavy regex. Nuclei-style templates that send anywhere or run multi-step logic are out; at most a safe subset, later.
- **Where content comes from matters.** spall ships vetted skills, roles and checks. Engagement-local checks are a `policy` import. Content packs show their source and license (M9); a skill is instructions to a model, so a pack is treated like code you'd run.
- **One format, two hosts.** Skills and roles use Claude Code's file shapes so the same files work in an external host and in spall's (a spall plugin for Claude Code bundling the MCP server config, roles and skills is the natural package).

### Is it a primitive?

When something new comes up, it is a **primitive** if any of these is yes:

1. Do bytes leave for a target (or anywhere) that the gate would not otherwise see?
2. Does input arrive from outside (a listener, callbacks, mail)?
3. Does it hold secrets the agent must never see?
4. Does it change volume, timing or concurrency beyond what budget and rate express?
5. Does it run code other than spall's own?

If it only decides *what* to send or *how to judge*, it is a **check**. If it is *how to work*, it is a **skill**. If it only reads what is recorded, it is a plain `read` **tool**. Every primitive needs a stop path (e-stop and halt end it mid-run), a budget charge and attribution in taphttp `meta`; if one of those doesn't fit, it is probably two primitives.

| Primitive | Why it is one | Where |
|---|---|---|
| `replay` | resend a captured request, edited | done (#55) |
| `fetch` | a new GET/HEAD to an in-scope URL; a crawl is `fetch` in a loop | #74 (done; taphttp `POST /send`) |
| `sweep` | many requests from a list (with a check), bounded, soft-404 baseline | #75 |
| `session` | log in as a test user and keep the session; holds credentials the agent never sees | #76 |
| `burst` | N requests released together (race conditions); breaks the rate limit by design, so its own RoE approval | #77 |
| intercept | hold and change a live request in flight | M7 (taphttp hook) |
| OOB listener | receive callbacks (blind SSRF, XXE) | #1 |
| inbox | receive mail for sign-up and reset flows | #2 |
| sandbox exec | run nmap, sqlmap, ...; HTTP egress forced through taphttp | #13 |
| browser | a headless browser driving the app (SPAs, DOM XSS); its traffic through the proxy, attributed | not filed |
| raw send | exact bytes (smuggling, HTTP/2 quirks, WebSockets); needs taphttp | not filed, later |

Not primitives: payload lists, wordlists, encoders and matchers (checks); compare/diff and extracting links (read tools); proposing a finding (a record tool); a crawl or fuzzing strategy (a skill). Every primitive refuses to target the stack itself (#73).

## Sensors: where the work happens

spall is the controller. The work happens in **sensors**, small deployable components that run wherever the target's traffic is: on the tester's laptop, on a compose network, on a VPS, or inside a client network.

```
            ┌── taphttp   observe + replay HTTP
spall  <────┼── listener  catch out-of-band callbacks (name TBD, #1)
            └── sandbox   run tools for the agent (name TBD, #13)
```

**Shared deployment kit** (designed once, in taphttp #8, reused by all three):
- Three modes per sensor: same box, compose network, remote. Unsafe combinations are refused at startup.
- Pairing: spall and each sensor exchange keys once; nothing else can connect.
- `--expires`: a forgotten sensor stops working.
- Startup record of mode, bound addresses and access controls; teardown notes.
- Remote deployment inside a client network only with explicit approval in the rules of engagement.

### The sandbox: tools for the agent, without a host shell

Agents need real tools (`curl`, `nmap`, `ffuf`, `nuclei`, `jq`, a Python interpreter to decode a JWT). The danger is a shell on the host guarded by a command blocklist: blocklists can always be bypassed. spall's answer is **containment, not filtering.**

- **Disposable and isolated.** A fresh sandbox per task, destroyed when the task ends. Strong isolation (gVisor or a Firecracker microVM, not a plain container where avoidable). No host mounts, no credentials, no access to spall's own data. Read-only image with a curated toolset; a scratch directory that dies with the sandbox. Files leave only as explicit task artifacts.
- **The network is the real control.**
  - All HTTP(S) egress is forced through taphttp: captured, scoped, budgeted, attributed to the task, visible in Traffic.
  - Non-HTTP egress (`nmap`, raw TCP) passes an egress firewall allowlisted to in-scope hosts, with packet capture so it's on the record too.
  - Out-of-scope traffic is not filtered by pattern; it has no route.
- **Everything is logged.** Each command, its exit code, output (truncated, with the full output as an artifact) and duration land in the task's request log alongside the HTTP requests.
- **Approval by category, not pattern.** Running an exploitation-class tool, exceeding N hosts, or exceeding the task's time or budget pauses the task for a human.
- **Resource limits:** CPU, memory, disk, process count, wall-clock per command.
- **The e-stop kills the sandbox,** not just the agent loop. Quad mode raises worker count inside the same limits.
- **Agent access is through spall's tools** (`run_in_sandbox`, `replay`, `propose_finding`, `mint_oob_payload`), never a direct socket to the sandbox.

Enterprise line: a local sandbox is open. Sandbox pools/fleets and hosted sandboxes are paid. Hosted capacity is priced by plan tier with generous included capacity, never per command.

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
- **The ecosystem matches the hard parts:** TanStack Virtual for the dense live table. (cmdk was the palette pick; see the budget below for why spall has its own.)
- **One frontend stack across Kudzu.** The THA casting app is already React; the Kudzu platform's dashboard can reuse spall's components.
- Considered: Svelte 5 (lighter and nicer, but Svelte 5 is new and models mix in Svelte 4 syntax), SolidJS (small ecosystem), Leptos/Dioxus (immature for virtualized tables, doubles the Rust learning curve), htmx (fights the client-side state this UI needs).

### Rules that come with it

**Untrusted content.** Captured traffic is attacker-controlled. If it ever renders as HTML, the target app gets XSS against the tester, in the tool holding their session tokens.
- Traffic data is rendered as text only. `dangerouslySetInnerHTML` is banned (the `react/no-danger` rule, enforced with ESLint's built-in `no-restricted-syntax` because `eslint-plugin-react` doesn't support ESLint 10), as are `innerHTML` / `outerHTML` / `insertAdjacentHTML` (`no-restricted-properties`). Any exception needs a comment and review.
- Previews of rendered HTML or images, if ever, only in a sandboxed iframe from a separate origin. So M1 has no image preview (#8 had proposed an inline one behind an `img-src blob:` CSP change); binary bodies get type, size, sha256, hex and save.

**CSP from day one:** `default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`. Vite production builds need no `eval` and no inline script. No CDN: fonts and assets are bundled.

**Supply chain.** A security tool's dependency tree is part of its attack surface.
- Runtime dependency budget: `react`, `react-dom`, `@tanstack/react-virtual`. Adding a runtime dependency needs a stated reason in the PR. `cmdk` was dropped in #8: it pulls about 25 runtime packages (Radix dialog machinery), including `react-style-singleton`, which injects `<style>` at runtime and conflicts with `style-src 'self'`. The palette is a small in-house component instead.
- Credential headers are highlighted using flux's redaction list (`Authorization`, `Cookie`, `Set-Cookie`, `Proxy-Authorization`; docs/flux/SPEC.md), so the console marks what an export will scrub. One list, taken from the flux crate (`/api/redaction`, flux v0.2.1).
- Lockfile committed, `npm ci` only, install scripts disabled (`ignore-scripts=true` in `.npmrc`).
- Versions at least two weeks old, transitive ones included: resolve with `npm install --before=<date>`. A test asserts the HTML bans fire, and a post-build check rejects inline script, inline style and external URLs in `dist/index.html`.
- OSV-Scanner on the lockfile in CI (via sec-actions).

**Styling:** verdigris tokens (`--vg-*`) and plain CSS. No CSS framework on top, no CSS-in-JS runtime.

**Build and embed:** source in `ui/`, `vite build` outputs `ui/dist/`, embedded with `rust-embed`. Docker becomes multi-stage (Node builds the UI, Rust builds the binary, final image has neither toolchain). Local dev: Vite dev server proxies `/events` and the API to the running spall.

**Tests:** Playwright for browser tests (#7), headless Chromium, no Docker. Untrusted-content rendering is the first test. Pure logic (formatting, header numbering, hex dump, the entry store) is unit-tested with Node's built-in `node:test` running TypeScript directly (`--experimental-strip-types`; `erasableSyntaxOnly` keeps the code compatible). Vitest was dropped: its optional-peer tree crashes npm 10.9's resolver, and the built-in runner needs no dependencies.

## Milestones

GitHub milestones are the tracker; this table is the narrative. Each milestone **lights up a rail icon** (unbuilt views stay dimmed placeholders until then). Issues also carry a view label (`view:engagement`, `view:traffic`, `view:surface`, `view:tasks`, `view:findings`) or `cross-cutting` for work that belongs to no single view (the action gate, sensors, CI). Issue numbers are spall issues unless noted.

Renumbered 2026-11-21: M3 engagement was inserted, so older issue text saying M3/M4/M5/M6/M7 now means M4/M5/M6/M7/M8. Split 2026-11-23: M6 became M6a, M6b and M6c (letters, so M7 onward keep their numbers); older text saying M6 means one of them.

| | Scope | Lights up | Issues |
|---|---|---|---|
| **M0 hello** (done, `v0.1.0`) | Docker Compose: taphttp + spall; spall tails the JSONL log and streams it to a single-page live console over SSE. | Traffic (first light) | |
| **M1 traffic workbench** (done, `v0.2.0`) | Filtering, search, request/response detail view, command palette and console. History comes from taphttp's lever API (its sqlite store), not a spall copy; see below. | Traffic | #3-#12 |
| **M2 replay** (done, `v0.3.0`) | Replay from the inspector via taphttp; edit, compare, Replays tab. Every action that sends traffic goes through the server-side action gate (scope, method policy, audit). CI and security gates; hardened runtime image. | Traffic | #17-#22, #10, #28, #29, taphttp#10 |
| **M3 engagement** (done, `v0.4.0`) | The engagement as the root object: targets, scope (incl. explicit out-of-scope), rules of engagement as data that the gate reads; build scope from traffic; paste a scope list; OpenAPI import; CA setup in the UI; Engagement home view. | **Engagement** | #30 (#32, #34-#40) |
| **M4 candidates** (done, `v0.5.0`) | Candidate model on flux crate types (cschooley/flux v0.1.0), WSTG v4.2 rule ids mapped to CWE, manual "flag as candidate", keyboard triage, evidence, suppressions. | **Findings** | #48 (#41-#47), #16 |
| **M5 export** (done, `v0.6.0`) | Promotion, flux export with redaction (flux v0.2.1) and fingerprints; redaction preview. | Findings | #52 (#49-#51) |
| **M6a agent over MCP** (done, `v0.7.0`) | An outside MCP client drives spall: server-side command registry, tasks, the gate for agents, approval gates, e-stop, MCP server (read traffic, replay, fetch, `propose_finding`; no promotion), untrusted tool output, Tasks view, attribution, duplicate detection. Engagement becomes the home view. | **Tasks** | #68 (#53-#62, #71, #73, #74, #78) |
| M6b agent host | spall runs the loop: bring-your-own key, roles, intake and orchestrator plans (chat plans, tasks act), quad, agent buttons. | Tasks | #69 (#63-#67) |
| M6c sensors | Agent tools outside spall on the shared sensor kit: execution sandbox, OOB listener, disposable inboxes. | Tasks | #70 (#13, #1, #2) |
| M7 intercept | Pause/modify via taphttp `--intercept-socket`. | Traffic | |
| M8 flux in | Suppressions and retests from the Kudzu platform; scurf hypothesis correlation; route maps ("source maps for the backend"). | **Surface** | #14 |
| M9 academy (capstone) | A separate Kudzu tool: graduated exercises on vulnerable targets in the sandbox, graded by spall + flux; WSTG/ASVS skills shared by humans and agents; content packs loaded from repos at runtime with their licenses shown; `--profile academy` ready-to-go lab. The content-pack loader is shared with M6b agent skills (#64). | (its own launcher tile) | #31 |
| Later | Wanted, not scheduled (label `later`). | | #15 |

Traffic history (decided Nov 2026, spall#4): spall reads history from taphttp's lever API rather than keeping its own copy of the traffic. One store, and M2 replay goes through the same API. spall has its own store only for what it owns: engagements (M3) and candidates (M4). Built on taphttp v0.3.0 (taphttp#7: token file outside the CA volume, sqlite-backed paging and search, `/feed?since=`): spall backfills from `/exchanges`, follows `/feed`, and proxies history to the browser as `/api/exchanges`. Tailing `--log-file` remains as a fallback when no API is configured. The API port stays on the compose network and is never published to the host.

SPA framework: decided, see Frontend above. M0 stayed one static HTML file on purpose.

The UI's information architecture (five views, shared inspector, command palette, agent e-stop) is in [ux.md](ux.md), with a view-to-milestone mapping.

## History

spall was first scoped (Aug 2026) as a CLI Cargo workspace (`spall-core`, `spall-ssrf`, a `clap` binary) starting with SSRF probing, with its own `Finding` type. That shape is superseded: finding types come from flux, and the primary interface is the web UI + MCP. SSRF probing returns later as an active-check module that produces candidates.
