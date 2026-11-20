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

Left rail, one icon each, keys `1` to `5`.

### 1. Engagement (home)

Not a stats dashboard. A **"what needs me now"** page.

- **Scope and guardrails:** in-scope hosts, rate limit, the agent's request budget and remaining balance
- **Waiting on you:** candidates to triage, tasks blocked on approval
- **Coverage at a glance:** endpoints seen vs. endpoints actually tested
- **Activity timeline:** yours and the agent's, interleaved

### 2. Traffic

The M0 console, grown up: live stream plus history, filters, inspector. Row actions: replay, flag as candidate, task the agent on this.

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

Approval gates: actions outside a task's constraints (a new method, more budget, a new host) pause the task as **waiting on approval** until a human says yes.

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
- **Replay lives in the inspector.** Edit the request, send, see original and replay side by side. Repeater stops being a destination.
- **Everything links.** Finding → evidence exchanges → producing task → endpoint in Surface, and back.
- **Command palette (`⌘K` / `Ctrl+K`)** instead of right-click menus: "replay this", "task agent on this endpoint", "flag". Keyboard first, discoverable.
- **Attribution everywhere.** Every exchange and candidate shows who created it: `you`, `agent · T-3`, or `scurf`. Agent-originated items carry a distinct glyph (◆), never color alone.
- **Agent e-stop.** Always visible in the top bar. One click (or `Shift+Esc`) halts every running task. A hardware e-stop on purpose: a confident AI testing someone else's app needs a big red button, and its constant presence builds trust.

## Color semantics (from verdigris)

| Meaning | Token |
|---|---|
| Live / active / running | `accent-verdigris` |
| Needs your action (approval, triage) | `accent-rust` |
| Stop / halted | `control-stop` |
| HTTP status / method | `status-*` / `method-*` |
| Finding severity | Text only until verdigris defines severity colors (M3) |

## Open source vs. enterprise

Open (single tester, single agent): all five views, tasks, triage, export.

**Enterprise:**
- Shared engagements: multiple testers, task assignment, shared triage
- Org-wide agent policy set by an admin (what any agent may ever do)
- Audit trail of agent actions for compliance export

## Mapping to milestones

| View / pattern | Milestone |
|---|---|
| Traffic (history, filters, inspector) | M1 |
| Replay in inspector | M2 |
| Findings (manual flagging, triage, suppressions) | M3 |
| Flux export with redaction preview | M4 |
| Tasks + MCP, e-stop, approval gates | M5 |
| Surface (coverage) | M5/M6, needs task and check attribution |
| Engagement home | After Tasks and Findings exist to summarize |
| Command palette | M1, grows with each milestone |
