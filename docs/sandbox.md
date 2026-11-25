# The execution sandbox (#13) — design

Status: **slices 1 and 2 built** (`src/sandbox.rs`,
`Replayer::run_in_sandbox`, the `run_in_sandbox` tool, `spall
sandbox-shell`; per-command taphttp credentials and the sandbox network).
Slice 2's network switches on once taphttp has its credential-only
listener (taphttp#27). Slices 3-7 are the plan.
This note expands `architecture.md` → *Sensors* → *The sandbox*. Read that first, and
*Building blocks* (the five questions) before changing what the sandbox may
do.

## What it is

The third sensor, next to taphttp and the OOB listener (#1): a disposable,
isolated place where an agent runs real tools (`curl`, `nmap`, `ffuf`,
`nuclei`, `jq`, a Python interpreter), with **containment doing the work a
command blocklist can't**. The agent never gets a host shell. It gets a
sandbox that can only reach a target through taphttp.

It is a **primitive** (all five of the architecture's questions are yes: it
runs code, bytes leave for a target, input arrives, it changes
volume/timing, and it holds a per-task proxy credential the agent never
sees). So, like every primitive, it needs a **stop path** (e-stop and halt
end it mid-run), a **budget charge**, and **attribution** in taphttp `meta`.
If one of those ever doesn't fit a piece of this, that piece is a second
primitive.

The invariant it exists to keep: **containment, not filtering.** We never
ship a regex blocklist of "dangerous commands". The host is untouched
because the sandbox cannot touch it, and out-of-scope traffic is refused
because it has no route — not because a pattern matched.

## Why gVisor, and the hard floor

Decided 2026-11-24: **gVisor (`runsc`) on Docker, Linux only.** A plain
container shares the host kernel; `runsc` interposes a user-space kernel, so
a container escape is a `runsc` escape first. Firecracker (microVM) stays an
option for later.

**No silent weaker mode.** Where `runsc` is absent (macOS, a dev box without
it, this repo's own CI by default), the sandbox **refuses to start and says
why**. It never falls back to plain `runc`: a weaker sandbox that looks like
the strong one is worse than none, because the tester trusts it. The
preflight check (`runsc` present, Docker reachable, the toolset image
pulled) is the one part of this testable on any box, because here `runsc` is
absent and the refusal is the behaviour under test.

## Where it sits in the code

Mirror the OOB sensor (`src/oob.rs`), which is the closest precedent: an
`Option<Arc<…>>` on the `Replayer`, `None` until configured, injected in
`lib::build`, with a trait so tests use a fake.

```
src/sandbox.rs
  trait Sandbox                     // start, exec, collect artifact, kill
    fn preflight() -> Result<(), Unavailable>
  struct Runsc : Sandbox            // the real one: docker run --runtime=runsc
  struct Fake  : Sandbox            // scripted, for tests (see Testing)
  struct Unavailable(String)        // why the sandbox won't start

Replayer {
  pub sandbox: Option<Arc<dyn Sandbox>>,   // like `oob`
}

App::with_sandbox(Arc<dyn Sandbox>) -> App // test seam, like with_model / with_resolver
```

The agent reaches it only through spall's tool `run_in_sandbox`; there is
**no HTTP route** to the sandbox and no socket from the agent to it (same as
`agent_replay`/`agent_fetch`: primitives have no web route, tests drive them
through `spall::build` + `App::run`).

## The primitive: `Replayer::run_in_sandbox`

One call, shaped like `Replayer::sweep` (`src/replay.rs`), which is the
model to copy: bounded work, under a task, gated, charged, stop-aware,
logged.

```
async fn run_in_sandbox(&self, task: &Task, cmd: SandboxCmd)
    -> Result<SandboxRun, String>
```

In order:

1. **Preflight / live sandbox.** `self.sandbox` is `Some`, else a clear
   "no sandbox configured" (like `oob` when unconfigured).
2. **Stack guard + role.** `src/stack.rs` already refuses the stack as a
   target; the role (`src/roles.rs`) must permit sandbox execution — Mapper
   and Tester only, never Scoper/Reporter — checked the same place
   `gated_send` checks a send the role can't make (refused, not paused: no
   approval widens a role).
3. **Approval by category, not pattern.** An exploitation-class tool, more
   than N hosts, or a time/budget overrun pauses the task to
   `waiting_on_approval` with the exact command in `Task::pending` — the
   same gate path as an out-of-task send. Category comes from the tool, not
   a regex over the argv.
4. **Budget charge under the book lock,** just before launch, via
   `tasks::Store::charge` (so nothing uncharged runs after an e-stop: the
   e-stop raises its flag under that same lock). A command's cost is its own
   unit; HTTP it makes is charged separately at taphttp (below).
5. **Run,** watching the stop flag: `Store::estop()` or the task leaving
   `running` kills the container within N seconds. E-stop kills the
   sandbox, not just the loop.
6. **Log** a new `Event::Sandbox { argv, exit, ms, artifact? }` on the task
   (truncated output inline, full output as an artifact), interleaved with
   `Event::Request` like the architecture says.
7. **Attribution:** every HTTP request the sandbox makes carries
   `meta: {actor: agent, task: T-n}` because it goes out on a per-task
   proxy credential (below), so it lands in Traffic attributed, exactly
   like `agent_replay`.

`SandboxCmd` is typed (argv as a list, never a shell string — "no opaque
command string the gate can't read"), a working dir under scratch, a
wall-clock limit.

## The network is the real control

This is the half that makes "enforced, not only captured" true, and it
leans on **taphttp's per-task proxy credentials (taphttp#25**, already in
taphttp 0.9.0; spall has no client for `/clients` yet — that's a slice).

- **All HTTP(S) egress forced through taphttp:** proxy env vars **and** an
  egress firewall, so a tool that ignores `HTTP_PROXY` has no other route.
- **Per-task credential:** spall mints one from taphttp `POST /clients`
  (allowlist = the task's in-scope hosts, plus the task's rate and budget),
  hands it to the sandbox, and deletes it at task end. taphttp enforces
  scope, rate and budget **there**, because spall's gate never sees the
  sandbox's HTTP one request at a time (a browser makes dozens per click).
- **Out-of-scope hosts have no route** — refused at taphttp for that
  credential, and unreachable through the firewall. Not pattern-filtered.
- **Non-HTTP egress** (`nmap`, raw TCP) passes the firewall allowlisted to
  in-scope hosts only, with packet capture stored as a task artifact.

## Preflight, limits, teardown

- **Limits:** CPU, memory, disk, pid count, per-command wall clock (Docker
  `--cpus --memory --pids-limit`, a read-only image, `--tmpfs` scratch).
- **Disposable:** fresh sandbox per task, destroyed at task end; scratch
  dies with it; files leave only as explicit task artifacts.
- **Deployment kit (taphttp#8):** same-box / compose / remote, pairing,
  `--expires`, a startup record and teardown notes — shared with the other
  two sensors, not reinvented here.

## Testing strategy (with `runsc` absent here)

Same pattern as `model::Model` (scripted in `tests/lever.rs`), the fake
resolver (`addresses::Resolve`), and the fake interactsh: a `Fake` sandbox
that honours the **contract** without a real kernel.

Testable on any box, against the fake and the real preflight:

- Preflight refuses when `runsc` is absent, with the reason. (Real, here.)
- A command is logged with argv, exit code and duration on the task.
- E-stop / halt kills a running sandbox within N seconds (the fake blocks
  until killed; assert it was).
- Budget is charged before launch; an e-stop between charge and launch runs
  nothing.
- The sandbox is gone after task end (the fake records teardown).
- A write outside scratch fails; an out-of-scope host has no route (the
  fake refuses both per the contract).

Needs a real `runsc` host (manual, and a CI runner that has it — `#[ignore]`
like the real-interactsh round-trip, gated on an env var):

- Containment for real: fork bomb and disk fill are contained and reported.
- A tool that ignores proxy env still can't reach the internet.
- An in-scope request appears in taphttp with the task's attribution; an
  out-of-scope one has no route.

## Slice sequence (proposed sub-issues under #70 / #13)

Each slice is shippable and TDD'd against the fake; the real runtime is
exercised on a `runsc` box as it lands.

1. **Sandbox primitive + preflight.** `src/sandbox.rs` (trait, `Runsc`,
   `Fake`), `Replayer::run_in_sandbox` through the gate (role, charge,
   stop, log), registry tool `run_in_sandbox` (send, MCP, Mapper/Tester),
   agent tool wrapped untrusted, `with_sandbox` test seam, config flags.
   No real network yet; the fake proves the contract. **This is the
   foundation every later slice hangs on.**
2. **Network enforcement.** spall client for taphttp `/clients`
   (taphttp#25): per-task credential, scope/rate/budget at taphttp,
   out-of-scope = no route. Egress firewall + forced proxy.
3. **Artifacts + logging polish.** Full output and pcap as task artifacts;
   `Event::Sandbox` in the Tasks view.
4. **Non-HTTP egress + packet capture.** Firewall allowlist, pcap artifact.
4b. **Toolpacks and the tool registry** (above): packs per tool family,
   category-based approval, wrappers that pass the proxy.
5. **Discovery tools for the Scoper.** Subdomain/port results into the
   asset inventory (#95); scope changes stay proposals (#91) through the
   guardrails (#83).
6. **Headless browser.** A browser the agent drives (SPAs, real logins, DOM
   XSS proof); its traffic through taphttp under the same enforcement.
7. **PoC button (#67's last verb).** Registry `poc` on a candidate
   (palette, console, MCP, Findings-pane button like the other verbs in
   `src/verbs.rs`): drafts a Tester task to build a standalone,
   non-destructive proof in the sandbox (XSS proves with `document.domain`,
   never a cookie stealer; IDOR reads, never writes). States cost first;
   the proof returns as a task artifact.

## What slice 1 taught us

- **Where it runs.** spall starts sandboxes with the `docker` CLI, so the
  sandbox works where spall runs on the host (`cargo run`, a binary).
  Inside the compose image (FROM scratch, no Docker socket, and mounting
  one would hand spall root on the host) it reports *unavailable* and says
  why. Running it under compose is the sensor deployment kit's job
  (taphttp#8): a separate sandbox sensor that holds the Docker access.
- **A fork bomb takes the sandbox down, not the host.** Under gVisor with
  the pids and memory caps, the container dies. spall notices ("the
  sandbox died"), forgets it, and the next command starts a fresh one with
  empty scratch. Backgrounded processes outlive their command's `timeout`
  but not the task: the sandbox goes when the task does.
- **A halted task's log is closed** (halted is forever), so a command the
  e-stop kills is recorded in the audit log, next to every other sandbox
  command, rather than in the task log.
- **`spall sandbox-shell`** gives the tester a shell with exactly the
  agent's flags (`sandbox::run_flags`), to see what a tool can and can't
  do. It checks without clearing anything, so it's safe while spall runs.
  It's the host's tool, never an agent's.

## What slice 2 does

- **Topology.** Sandboxes join `spall-sandbox`, a Docker network created
  `internal: true` (compose defines it), whose only other member is
  taphttp. Nothing on it routes anywhere; the only way out is taphttp.
- **Two doors in taphttp** (taphttp#27): the open `--listen` for the
  tester's browser, and a credential-only `--auth-listen` for sandboxes.
  One door with `--proxy-auth-api` would have put a password on the
  browser too.
- **The check before anything routes** (`sandbox::check_network`, every
  minute until it passes): taphttp's `/info` reports `auth_proxy.auth ==
  "issued"`; the network is `--internal`; a probe sandbox on it gets no
  answer from the open listener and a 407 from the credential-only one.
  Until all hold, sandboxes start with `--network none` and the status
  says why.
- **A credential per command** (`Replayer::sandbox_grant`): `allow` from
  `Scope::sandbox_allow` (the task's hosts or the scope's entries; never
  `*`; a name only through an explicit entry; a wildcard or network only
  if no exclusion or never-test entry falls inside it, since a credential
  has no deny list; a network only if it holds none of the stack's
  addresses), `budget` the task's remaining one after the command's own
  unit, `rate` the task's, `meta` the task. The proxy URL and taphttp's CA
  reach the command through `docker exec -e NAME` from docker's own
  environment, so the password isn't on the host's process list; it's
  never logged or audited.
- **After the command** the credential is revoked (closing anything a
  backgrounded process still had open), and what taphttp counted on it
  (`sent`, `refused`) is charged to the task (`tasks::Store::spend`),
  logged on `Event::Sandbox` and audited. The e-stop or a halt revokes it
  at once, before the sandbox is removed.
- **Raw TCP** (`nmap`, anything not HTTP) has no route: slice 4.

## The sandbox sensor (`spall sandboxd`)

spall's own image has no Docker and never gets the socket: the app that
shows raw traffic, credentials included, shouldn't be root on the host.
Under compose (`--profile sandbox`), `sandboxd` is the one container that
holds `/var/run/docker.sock`, and spall reaches it with
`SPALL_SANDBOX_URL=http://sandboxd:8070`.

- **Narrow by construction.** Five calls: `GET /check`, `POST /exec`
  (engagement and task ids, an argv list, a timeout, optionally a proxy URL
  and CA), `POST /teardown`, `GET /live`, `POST /network`. No image, mount,
  flag, network or container name comes from the caller: the image,
  network and taphttp's listeners are sandboxd's own flags, and every
  container gets `run_flags`. A compromised spall can run tools in
  sandboxes, which it could anyway; it can't ask for a privileged
  container or a host mount.
- **Who may call.** A token sandboxd writes (0640, group 10001, on the
  `sandbox-token` volume spall mounts read-only), compared in constant
  time; any request carrying an `Origin` is refused. Nothing is published;
  sandboxes aren't on its network.
- **Its own container:** root for the socket (its owner) with no
  capabilities, read-only, `no-new-privileges`.
- **`Remote`** is spall's side: the same `Sandbox` trait over HTTP, so the
  gate, the stop path (a dropped request drops `docker exec`; spall then
  tears down) and the tests don't change.
- spall on the host still works without it (`--sandbox-image`), starting
  containers itself.

## Tools: toolpacks, not a distro (decided 2026-11-25)

Containment is what makes a tool safe here, not knowing what it does:
whatever sqlmap or nuclei sends goes through taphttp on the command's
credential, so it can't leave scope or outrun the task's rate and budget.
So the questions are supply chain and who says yes.

- **A small base** (`sandbox/Dockerfile`): `curl`, `jq`, `python3`,
  `openssl`, `dig`, `nmap`. Enough for first-principles scripts on a
  precise hypothesis (does this parameter reflect? decode this JWT), which
  are short, readable and kept for review (slice 3), and feed the PoC
  button.
- **Toolpacks:** one image per tool family (`spall-pack-sqli`: sqlmap;
  `-fuzz`: ffuf; `-scan`: nuclei with vetted templates), each pinned by
  digest and scanned, pulled only when a task needs one. Wordlists
  (SecLists) are a read-only data mount, not code. Not a multi-terabyte
  distro: unpinnable, and a supply chain nobody reviewed.
- **A tool registry** (data): tool → pack, and a *category* (recon,
  scanner, exploitation) by program name, never by matching arguments.
  An exploitation-class tool (sqlmap, hydra) pauses the task for the
  tester, who sees the exact argv; a destructive one needs the RoE too.
- **Tools that ignore proxy env fail closed** (no route); a pack ships a
  thin wrapper that passes the proxy (`sqlmap --proxy`).
- **Bring your own pack:** a tester imports an image (a `policy` action,
  pinned by digest), as content packs are (#31). A Kali-sized image is an
  opt-in the tester makes, still under gVisor and taphttp.

## Open questions

- **Toolset image contents and update cadence.** It is supply chain: pin by
  digest, scan it (osv/trivy), document what's in it and why. Start minimal
  (`curl`, `jq`, Python, `nmap`, `ffuf`); add on evidence.
- **Approval categories.** The exact list of "exploitation-class" tools and
  the host/time/budget thresholds that pause a task.
- **N seconds for the e-stop kill.** The deadline before a hard `docker
  kill`, and what we log if a container won't die.
