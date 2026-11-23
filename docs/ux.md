# spall UX: information architecture

Status: design intent (Nov 2026). Visual reference: verdigris `mockups/spall-console.html`, `spall-tasks.html`, `spall-findings.html`.

## The core idea: objects, not tools

Burp is **tool-centric**. Proxy, Repeater, Intruder and Scanner are separate destinations, and a tester's day is spent shuttling requests between them ("Send to Repeater").

spall is **object-centric**. There are four kinds of things:

| Object | What it is |
|---|---|
| **Exchange** | One captured or replayed request/response (from taphttp) |
| **Endpoint** | A method + normalized path + parameters, discovered from exchanges |
| **Task** | A unit of agent work with a goal, constraints and a log |
| **Finding** | A flux result: candidate, then confirmed / rejected / duplicate |

They all link to each other, and every action (replay, flag, task the agent) is available wherever the object appears. Nothing is a separate tool you have to travel to.

## Five views

Left rail, one icon each, keys `1` to `5`, always in this order.

While spall is being built, views that don't exist yet still get their icon, **dimmed and inert** (no link; the tooltip says it isn't built yet). That fixes the order and the number keys from the start, and each icon lights up when its view ships. A finished release has no dimmed icons.

### 1. Engagement (home)

Not a stats dashboard. A **"what needs me now"** page.

- **Scope and guardrails:** in-scope hosts, rate limit, the agent's request budget and remaining balance
- **Waiting on you:** candidates to triage, tasks blocked on approval
- **Coverage at a glance:** endpoints seen vs. endpoints actually tested
- **Activity timeline:** yours and the agent's, interleaved

Built thin first in M3 (#37): the engagement's details, scope (with exclusions, target notes and a check box), rules of engagement, a "testing now" chip and an expiry countdown, engagement switching, and dimmed placeholders for the panels above that later milestones fill. Engagement is the home view (decided 2026-11-23, #71): a bare URL, or one naming a view that isn't built, opens it. `1` to `5` switch, and the location hash (`#traffic`, `#engagement/...`) remembers the view. verdigris has no Engagement mockup yet; the view borrows the tasks and findings mockups' sections, `dl` grids and chips.

### 2. Traffic

The M0 console, grown up: live stream plus history, filters, inspector. Actions on an exchange (copy as curl and save body in M1; replay in M2; Flag candidate in M4; task the agent in M6a) live in the inspector, not on rows. Candidates go to the Findings view, never a panel beside Traffic.

### 3. Surface

Replaces the site map, organized around **coverage** instead of a URL tree.

- Every endpoint and parameter shows **who has touched it** (you, the agent, a scurf hypothesis) and **which checks ran**
- Untested surface is visually obvious
- This is where you point the agent: "task the agent on everything untested under `/v1/cart`"

The question it answers is the one that matters in an assessment: *what haven't we tested?*

### 4. Tasks (the agent)

**Not a chat tab.** A task is a work order:

| Part | Example |
|---|---|
| Goal | Test `POST /api/v2/session` for auth bypass |
| Constraints | Scope, allowed methods, request budget, rate, time limit, stop conditions |
| Plan | The agent's steps, checked off as it goes |
| Status | Running / waiting on approval / done / halted |
| Log | Every request the agent sent, each linked to its exchange in Traffic |
| Output | Proposed candidates, which land in Findings |

Steering chat lives *inside* a task, as a side panel. A chat window is a poor audit trail; a task with a goal, constraints and a linked request log is a good one, and that matters when a client asks what the AI did to their production app.

Approval gates: actions outside a task's constraints (a new method, more budget, a new host, a send it must ask for) pause the task as **waiting on approval** until a human answers, on a card in Waiting on you that shows exactly what would be sent. **Approve once** lets that exact request through one time (within 10 minutes; the agent never holds the approval). **Approve for the task** widens the task for what was asked, never past the rules of engagement (something the RoE wants confirmed can only be approved once). **Deny** takes a reason, and the task carries on with it so the agent can try another way. Outside the engagement's scope or RoE is a plain denial: no approval can widen the engagement.

#### Chat is the front door, tasks are the engine

The tester can still just say what they want. An **intake** panel, attached to an engagement and never free-floating, takes plain language. An orchestrator turns it into a **plan**: a tree of tasks, each with a role, scope, budget and stop conditions, which the tester edits and approves. Approved, the tree runs in the Tasks view and updates live; that live status is the bleeps and boops.

The intake's vocabulary is spall's objects (engagements, endpoints, exchanges, tasks, findings) and nothing else. Off-topic questions get the console's answer to `god`: "no."

Chat can never do anything a task and the action gate would not allow. It produces plans; only approved tasks act.

Worked example, the one the design has to pass:

> "My boss said to re-scan the findings from last week's evilcorp.com test."

1. **Resolve.** "last week's evilcorp.com test" is an engagement, matched by name and date; "the findings" are its findings (Kudzu) or candidates (spall); "re-scan" is a retest. Anything ambiguous is shown as a list to pick from, not guessed.
2. **Check before planning.**
   - *Still authorized?* If the engagement's test window has closed, say so and draft an extension or a retest engagement for a human to approve. "My boss said" is context, not authorization.
   - *Target unchanged?* If in-scope hosts now resolve somewhere else (new IPs, a CDN, a new owner), flag it before anything is sent.
3. **Plan.** One retest task per finding: replay the original evidence (plus the scurf route or hypothesis if there is one) and diff against the original (#20). Shown as "14 findings, 14 retest tasks, safe methods except 2 that need unsafe replays, ~300 requests." Nothing runs until approved.
4. **Run.** Tasks execute through the gate; unsafe steps stop at **needs your action**.
5. **Results.** No longer reproduces: `flux/status: fixed` with the retest exchange as evidence, and Kudzu closes the ticket on that evidence. Still reproduces: stays open with a comment. The audit record carries who asked, so "my boss said" is a note on the task, not a mystery.

#### Roles

The orchestrator hands tasks to **roles**. A role is a bundle of skills plus a permission set (least privilege, per agent):

| Role | Can do | Gate risk class |
|---|---|---|
| Scoper | Read traffic and scurf route maps; propose scope additions | read-only; proposals need a human |
| Mapper | Crawl, replay safe methods, passive checks | safe |
| Tester | Unsafe methods, payloads, OOB callbacks | unsafe; confirmation per #22 |
| Reporter | Draft candidates and evidence | `propose_finding` only |

The orchestrator can plan anything, but it cannot hand a role a task the engagement's rules of engagement don't allow. Role skills are the same WSTG/ASVS skills the academy uses (M9), loaded by the same content-pack loader.

### 5. Findings

A triage inbox you can clear from the keyboard.

- **Queue** of candidates. `c` confirm, `r` reject, `d` duplicate; reject and duplicate require a justification
- **Evidence pane:** linked exchanges, replay diff, the task that produced it, the scurf hypothesis it confirms (if any)
- **Redaction preview:** exactly what flux export will strip
- **Promoted** findings and flux export

The agent can only *propose*. Promotion is a human action. See `architecture.md`.

## Cross-cutting patterns

- **One inspector.** The right-hand detail pane behaves the same in every view.
- **Headers are shown in wire order, repeats kept.** Never merge repeated headers or comma-join them (`Set-Cookie` cannot be comma-joined). Repeats get a group rule and an `n/N` counter. Requires taphttp to store headers as an ordered list, not a map (taphttp#5).
- **Binary bodies get a placeholder, not garbage.** Show type, size and hash; offer hex view (8 bytes per row, paged), preview for images, and save. Never render raw binary as text.
- **Replay lives in the inspector.** Edit the request, send, see original and replay side by side. Repeater stops being a destination. A replay is a real request carrying the captured credentials, so unsafe methods (`POST`, `PUT`, `PATCH`, `DELETE`) take two presses: the first arms ("press again to send POST to <host>"), the second sends. Arming expires after a few seconds or on any other action. Safe methods send at once. The two presses are the UI's way of satisfying the server's rule (#22), not the rule itself: the first press gets a single-use token from the action gate, bound to that exact request, and the second sends it back. The console's `replay <id> --yes` does both in one step; an agent (M6a) gets the token only through an approval. Denials (out of scope) show their reason in the inspector and the console. **Edit & replay** (`e`, #19) turns the Request tab into a form: method, the ordered header list (edit, add, remove, reorder) and a text body, with changes marked against the original and the limits stated in place (the URL is fixed; binary bodies go as captured; repeats split by other headers are sent grouped; a `Host` unlike the URL's is flagged). Side-by-side comparison is #20. The inspector's **Replays N** tab (#21) lists an exchange's replays (history via `replay_of`, plus live), each with what it changed; a replay of a replay shows its chain back to the first original. **Compare** (#20) sets a replay against its original: a verdict line, then header and body diffs from `ui/src/lib/diff.ts`, the same comparison M4's evidence pane and M8's retests reuse.
- **Everything links.** Finding → evidence exchanges → producing task → endpoint in Surface, and back.
- **Command palette (`⌘K` / `Ctrl+K`)** instead of right-click menus: "replay this", "task agent on this endpoint", "flag". Keyboard first, discoverable.
- **Attribution everywhere.** Every exchange and candidate shows who created it: `you`, `agent · T-3`, or `scurf`. Agent-originated items carry a distinct glyph (◆), never color alone.
- **Agent e-stop.** Always visible in the top bar. One click (or `Shift+Esc`, which works even while typing and never clears it) halts every agent task, in every engagement, for good: a halted task is never resumed, and the work is given again as a new task. Confirm tokens are revoked, the stop survives a restart, and it holds until a human clears it with two presses; clearing only lets new tasks run. A hardware e-stop on purpose: a confident AI testing someone else's app needs a big red button, and its constant presence builds trust.

## Verbs on objects (buttons)

Chat is the front door; buttons are the other doors, and none of them get around the gate. A button is a registry command that pre-fills a task (role, scope, budget, risk class) from the object it sits on. One click shows the task card to approve; a safe-class task inside the engagement may run at once.

Rules:

- **No button-only powers.** Every button is also a palette entry, a console command and an MCP tool, from the same registry.
- **Name the outcome, not the effort.** "Escalate impact", not "Deepen". If you can't tell what a button will produce, you won't click it.
- **Show the cost first.** Every button states its estimate before it runs ("~40 requests, safe methods").
- **Hidden until built.** A button appears only when its milestone ships, like the dimmed rail views.

| On | Button | Produces | Role, risk class |
|---|---|---|---|
| Finding / candidate | **PoC** | A standalone proof for a dev: curl script, CSRF page, minimal repro, built in the sandbox | Tester; non-destructive by rule (XSS proves with `document.domain`, never a cookie stealer; IDOR reads, never writes) |
| | **Escalate impact** | Can the read become a write, reflected become stored, does it chain with another finding? | Tester; unsafe, always asks |
| | **Find variants** | The same bug elsewhere, from the scurf route map and sibling parameters | Mapper, then Tester |
| | **Retest** | The retest flow (see Tasks) for this one finding | Tester; class of the original evidence |
| | **Explain for devs** | Developer-facing writeup drafted from evidence and the writeup library | Reporter; no traffic |
| Endpoint (Surface) | **Test for…** | A WSTG category picker: "Test for IDOR", "Test for injection" | Tester; per category |
| | **Map auth** | Which roles reach this endpoint, with which methods | Mapper; safe |
| Exchange (Traffic) | **Replay**, **Diff** | As built in M2 | (gate per #22) |
| | **Why is this interesting?** | A read-only explanation; spends no request budget | none; no traffic |

Default output already includes evidence and a repro in every candidate; the buttons are for steering the agent past the default.

## The console and quad mode

### `` ` `` console

Quake-style drop-down console, the same in every suite app (verdigris component).

- **Palette vs. console.** `⌘K` is for finding things you don't remember (fuzzy search). The console is for doing things you do remember: typed commands with arguments, history, tab completion, scrollback.
- **One command registry.** Palette, console and MCP server expose the same commands with the same names and arguments. A command that exists in one exists in all three. It lives on the server, in `src/commands.rs` (#53): name, usage, argument schema (JSON Schema, reused as the MCP `inputSchema`), description, risk class (`read`, `record`, `send`, `unsafe`, `destructive`, `policy`, `export`) and whether it may be offered to the agent (`mcp`). It is served at `GET /api/commands` and generated into the UI (`ui/src/commands/specs.gen.ts`), where `registry.ts` adds only how each command runs in the browser; tests fail if the two disagree. The server refuses `policy` and `export` commands (promotion included) to an agent whatever its `mcp` flag says, and audits the refusal.
- **System output lands here:** agent messages, late OOB callbacks, warnings.
- **Text only.** Console output is never rendered as HTML (see the "Traffic is never HTML" rule).
- Toggle on backtick (configurable for non-US layouts). Never opens while typing in an input.

### Quad mode

A real feature with a joke name: a **time-boxed burst** for the agent.

- `quad [seconds]` (default 120, max 600): 4× agent workers at 4× rate, then it **wears off on its own**.
- **Never exceeds the engagement.** 4× applies *inside* the task constraints and rules of engagement. If 4× would exceed the engagement's rate ceiling, it's capped and the console says so.
- **Counts against request budgets** like any other agent work, and is logged in the task's request log with start, end and worker count.
- **The e-stop ends it instantly.** Halted agents can't go quad.
- **Visual state:** `--vg-boost` screen-edge glow, a `QUAD m:ss` countdown chip in the top bar, the request meter in boost color. Optional power-up sound, off by default (screen-shares).
- **Scorecard when it ends:** duration, requests sent, agent tokens used, candidates found. A **shareable quad card** can be copied. It's **redacted by construction**: numbers only, no hosts, paths, payloads or client names. Nobody leaks a client's attack surface on social media for a flex.
- **Burn is the user's own.** Quad burns their LLM tokens and their engagement's request budget. spall (and any hosted version) never charges per request or per scan. That's the pricing rule: more testing never costs more on our bill.
- `god` answers "no."

## Color semantics (from verdigris)

| Meaning | Token |
|---|---|
| Live / active / running | `accent-verdigris` |
| Needs your action (approval, triage) | `accent-rust` |
| Stop / halted | `control-stop` |
| Quad active | `boost` |
| HTTP status / method | `status-*` / `method-*` |
| Finding severity | Text only until verdigris defines severity colors (M4) |

## Open source vs. enterprise

Open (single tester, single agent): all five views, tasks, triage, export.

**Enterprise:**
- Shared engagements: multiple testers, task assignment, shared triage
- Org-wide agent policy set by an admin (what any agent may ever do)
- Audit trail of agent actions for compliance export

## Mapping to milestones

| View / pattern | Milestone |
|---|---|
| Traffic (history, filters, inspector) | M1 (done) |
| Replay in inspector | M2 (done) |
| Findings (manual flagging, triage, suppressions) | M4 (done) |
| Flux export with redaction preview | M5 (done) |
| Tasks + MCP, e-stop, approval gates | M6a |
| Intake chat, orchestrator plans, roles | M6b |
| Agent buttons (PoC, Escalate impact, Find variants, Retest, Explain for devs, Test for…, Map auth, Why is this interesting) | M6b (PoC needs the M6c sandbox); Find variants uses route maps fully from M8; Retest closes Kudzu tickets from M8 |
| Surface (coverage) | M6a–M8: needs task and check attribution (M6a); seeded from route maps (M8) |
| Engagement home, targets, scope, rules of engagement | M3 (done); home view since M6a (#71) |
| Command palette and `` ` `` console (one registry) | M1, grows with each milestone |
| Quad mode | M6b, with the agent host |
