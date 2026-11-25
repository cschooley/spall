//! Tasks (#54): the work orders an agent runs under (docs/ux.md, *Tasks*),
//! spall's third owned object after engagements and candidates.
//!
//! A task belongs to one engagement and carries its goal, role, plan,
//! status, request budget and spend, who asked, and a log of everything
//! done under it. Its **constraints only narrow** the engagement's rules of
//! engagement: they default to the RoE, and anything wider is refused when
//! the task is created or edited. The gate (#55) also checks them as a
//! layer on top of the RoE (`roe::check_layers`), so an RoE that narrows
//! later still wins over a task made before it.
//!
//! Stored like candidates: `<engagements>/tasks/<engagement>.json`, written
//! atomically, 0600 in a 0700 directory, with a `schema_version`; a file
//! that is corrupt or from a newer spall stops spall from starting.

use crate::engagement::{create_dir_private, write_atomic};
use crate::gate::{HostState, Scope};
use crate::roe::{Period, Rules};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_PER_ENGAGEMENT: usize = 1000;
/// Log entries per task. A task that fills its log can't go on: #55 halts it.
pub const MAX_LOG: usize = 20_000;
pub const MAX_GOAL: usize = 2000;
pub const MAX_NOTE: usize = 2000;
/// An agent's report on its task (#81), in characters.
pub const MAX_REPORT: usize = 16_000;
pub const MAX_STEPS: usize = 50;
pub const MAX_STEP: usize = 300;
pub const MAX_HOSTS: usize = 100;
/// Requests a task may be given, at most.
pub const MAX_BUDGET: u32 = 100_000;
pub const DEFAULT_BUDGET: u32 = 100;
/// Minutes a task may run, at most (a day) and by default.
pub const MAX_MINUTES: u32 = 24 * 60;
pub const DEFAULT_MINUTES: u32 = 60;
/// Model tokens (in and out) a task run by spall's host may use (#63).
pub const DEFAULT_TOKENS: u64 = 2_000_000;
pub const MAX_TOKENS: u64 = 100_000_000;

/// Where a task stands. `draft` → `approved` → `running` ⇄
/// `waiting_on_approval` → `done` / `halted` / `failed`; anything not yet
/// finished can be halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Approved,
    Running,
    WaitingOnApproval,
    Done,
    Halted,
    Failed,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Draft,
        Status::Approved,
        Status::Running,
        Status::WaitingOnApproval,
        Status::Done,
        Status::Halted,
        Status::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Approved => "approved",
            Status::Running => "running",
            Status::WaitingOnApproval => "waiting_on_approval",
            Status::Done => "done",
            Status::Halted => "halted",
            Status::Failed => "failed",
        }
    }

    /// Finished: nothing more happens under it.
    pub fn finished(self) -> bool {
        matches!(self, Status::Done | Status::Halted | Status::Failed)
    }

    /// Whether `self → to` is a legal move.
    pub fn can_become(self, to: Status) -> bool {
        use Status::*;
        match (self, to) {
            (Draft, Approved)
            | (Approved, Running)
            | (Running, WaitingOnApproval)
            | (WaitingOnApproval, Running)
            | (Running, Done)
            | (Running, Failed)
            | (WaitingOnApproval, Failed) => true,
            (from, Halted) => !from.finished(),
            _ => false,
        }
    }
}

/// Who the task is for (#64 gives each its permission set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Scoper,
    Mapper,
    Tester,
    Reporter,
}

/// Who runs a task's agent loop (#63).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runner {
    /// An outside MCP client (Claude Code, Claude Desktop, ...).
    #[default]
    External,
    /// spall's own host, with the tester's model key.
    Spall,
}

/// What the agent says it did and found (#81): plain text, written by a
/// model that read attacker-controlled traffic, so shown as text only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub text: String,
    pub at: String,
}

/// What a task's model has used (#63): only spall's host counts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelUsage {
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Time spent waiting on the model.
    pub model_ms: u64,
}

impl ModelUsage {
    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

fn default_tokens() -> u64 {
    DEFAULT_TOKENS
}

/// Stop the task when one of these happens (#55 enforces them).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    /// After this many 5xx responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_errors: Option<u32>,
    /// After this many 429 responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttled: Option<u32>,
}

/// A task's limits, resolved against the engagement's rules: never wider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    /// Hosts the task may send to, each in the engagement's scope; empty
    /// means the engagement's whole scope.
    pub hosts: Vec<String>,
    /// Methods it may send; empty means any the RoE allows.
    pub methods: Vec<String>,
    /// Methods it sends without asking; others pause for approval (#55).
    pub safe_methods: Vec<String>,
    /// Sends per second; never above the RoE's.
    pub rate_limit: Option<f64>,
    /// May run destructive-class actions; only if the RoE allows them.
    pub destructive: bool,
    /// Requests it may send.
    pub budget: u32,
    /// Minutes it may run.
    pub time_limit_minutes: u32,
    #[serde(default)]
    pub stop: Stop,
    /// Model tokens (in and out) spall's host may use on it; past this it
    /// halts. An outside client's model isn't spall's to count.
    #[serde(default = "default_tokens")]
    pub tokens: u64,
}

/// Asked-for constraints: anything left out comes from the RoE (or the
/// defaults, for what the RoE doesn't have).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requested {
    pub hosts: Option<Vec<String>>,
    pub methods: Option<Vec<String>>,
    pub safe_methods: Option<Vec<String>>,
    pub rate_limit: Option<f64>,
    pub destructive: Option<bool>,
    pub budget: Option<u32>,
    pub time_limit_minutes: Option<u32>,
    pub stop: Option<Stop>,
    pub tokens: Option<u64>,
}

fn method_token(m: &str) -> bool {
    (1..=32).contains(&m.len())
        && m.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn in_list(list: &[String], m: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(m))
}

/// `req` resolved against the engagement's `rules` and `scope`. Leaving a
/// field out takes the RoE's; asking for more than the RoE allows is an
/// error that says what, never silently trimmed.
pub fn resolve(req: &Requested, rules: &Rules, scope: &Scope) -> Result<Constraints, String> {
    let mut hosts = Vec::new();
    for h in req.hosts.as_deref().unwrap_or_default() {
        let v = scope.explain(h);
        if v.host.is_empty() {
            return Err(format!("hosts: {h:?} isn't a host name or IP address"));
        }
        if v.host.contains('*') || v.host.contains('/') {
            return Err(format!("hosts: {h:?}: name hosts one by one, not patterns"));
        }
        // Every host allowed (`scope all on`) is this process only; a task
        // stays inside the engagement's own entries.
        if v.state != HostState::In || v.by.as_deref() == Some("*") {
            return Err(format!("hosts: {} isn't in the engagement's scope", v.host));
        }
        if !hosts.contains(&v.host) {
            hosts.push(v.host);
        }
    }
    if hosts.len() > MAX_HOSTS {
        return Err(format!("hosts: at most {MAX_HOSTS}"));
    }

    let methods = match &req.methods {
        None => rules.methods.clone(),
        Some(list) if list.is_empty() => {
            return Err("methods: name at least one, or leave it out for the RoE's".into())
        }
        Some(list) => {
            let mut out: Vec<String> = Vec::new();
            for m in list {
                if !method_token(m) {
                    return Err(format!("methods: {m:?} isn't an uppercase method name"));
                }
                if !rules.methods.is_empty() && !in_list(&rules.methods, m) {
                    return Err(format!(
                        "methods: {m} isn't allowed by the rules of engagement (allowed: {})",
                        rules.methods.join(", ")
                    ));
                }
                if !out.contains(m) {
                    out.push(m.clone());
                }
            }
            out
        }
    };

    let safe_methods = match &req.safe_methods {
        None => rules
            .safe_methods
            .iter()
            .filter(|m| methods.is_empty() || in_list(&methods, m))
            .cloned()
            .collect(),
        Some(list) => {
            let mut out: Vec<String> = Vec::new();
            for m in list {
                if !method_token(m) {
                    return Err(format!(
                        "safe_methods: {m:?} isn't an uppercase method name"
                    ));
                }
                if !in_list(&rules.safe_methods, m) {
                    return Err(format!(
                        "safe_methods: the rules of engagement want {m} confirmed"
                    ));
                }
                if !methods.is_empty() && !in_list(&methods, m) {
                    return Err(format!("safe_methods: {m} isn't one of the task's methods"));
                }
                if !out.contains(m) {
                    out.push(m.clone());
                }
            }
            out
        }
    };

    let rate_limit = match req.rate_limit {
        None => rules.rate_limit,
        Some(r) => {
            let ceiling = rules.rate_limit.unwrap_or(f64::INFINITY);
            if !(r > 0.0 && r <= ceiling && r <= 1000.0) {
                return Err(match rules.rate_limit {
                    Some(c) => {
                        format!("rate_limit: more than 0 and at most the RoE's {c} per second")
                    }
                    None => "rate_limit: more than 0 and at most 1000 per second".into(),
                });
            }
            Some(r)
        }
    };

    let destructive = req.destructive.unwrap_or(false);
    if destructive && !rules.destructive_allowed {
        return Err("destructive: the rules of engagement don't allow destructive actions".into());
    }

    let budget = req.budget.unwrap_or(DEFAULT_BUDGET);
    if !(1..=MAX_BUDGET).contains(&budget) {
        return Err(format!("budget: 1 to {MAX_BUDGET} requests"));
    }
    let time_limit_minutes = req.time_limit_minutes.unwrap_or(DEFAULT_MINUTES);
    if !(1..=MAX_MINUTES).contains(&time_limit_minutes) {
        return Err(format!("time_limit_minutes: 1 to {MAX_MINUTES}"));
    }
    let stop = req.stop.clone().unwrap_or_default();
    if stop.server_errors == Some(0) || stop.throttled == Some(0) {
        return Err("stop: a count of at least 1".into());
    }

    let tokens = req.tokens.unwrap_or(DEFAULT_TOKENS);
    if !(1..=MAX_TOKENS).contains(&tokens) {
        return Err(format!("tokens: 1 to {MAX_TOKENS}"));
    }

    Ok(Constraints {
        hosts,
        methods,
        safe_methods,
        rate_limit,
        destructive,
        budget,
        time_limit_minutes,
        stop,
        tokens,
    })
}

impl Constraints {
    /// The task's layer for `roe::check_layers`, on top of the engagement's
    /// `rules`: its methods, confirmation and rate; agents allowed (the RoE
    /// layer decides that); destructive only if the task may.
    pub fn layer(&self, rules: &Rules) -> Rules {
        Rules {
            methods: self.methods.clone(),
            safe_methods: self.safe_methods.clone(),
            rate_limit: self.rate_limit,
            time_zone: rules.time_zone.clone(),
            windows: Vec::new(),
            blackouts: Vec::new(),
            agents_allowed: true,
            destructive_allowed: self.destructive,
            refuse_destructive_looking: rules.refuse_destructive_looking,
            sweep_max: rules.sweep_max,
        }
    }

    /// Whether the task may send to `host` (the scope is checked too).
    pub fn allows_host(&self, scope: &Scope, host: &str) -> bool {
        let v = scope.explain(host);
        v.state == HostState::In && (self.hosts.is_empty() || self.hosts.contains(&v.host))
    }
}

/// Whether the engagement lets anyone make a task now: agents allowed, a
/// scope, and inside its dates and authorization. Windows don't matter
/// for planning: a task waits for one.
pub fn authorized(
    rules: &Rules,
    period: &Period,
    scope: &Scope,
    now: jiff::Timestamp,
) -> Result<(), String> {
    if !rules.agents_allowed {
        return Err(
            "the rules of engagement don't allow agents (roe set agents yes, if they should)"
                .into(),
        );
    }
    if scope.with_all(false).is_empty() {
        return Err("the engagement has no scope: a task would have nowhere to go".into());
    }
    Rules {
        windows: Vec::new(),
        blackouts: Vec::new(),
        ..rules.clone()
    }
    .check_time(period, now)
}

/// Whether `role` can be given these constraints (#64): refused, not
/// trimmed, like asking for more than the RoE. Only a Tester gets
/// destructive actions; what a role may send is checked at the gate.
pub fn role_fits(role: Role, c: &Constraints) -> Result<(), String> {
    if c.destructive && !role.may_destroy() {
        return Err(format!(
            "destructive: a {} doesn't get destructive actions; give the work to a tester",
            role.as_str()
        ));
    }
    Ok(())
}

/// One step of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// What happened under a task, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// A request sent (or refused) under the task: which check against
    /// which endpoint, for coverage (Surface, later).
    Request {
        exchange: Option<String>,
        method: String,
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        check: Option<String>,
        outcome: String,
        /// The target's status code, when it answered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
    },
    /// A registry command the task ran.
    Command { name: String, outcome: String },
    /// An exchange the task read (evidence it may cite, #60).
    Read { exchange: String },
    /// An approval asked for or given (#56).
    Approval { what: String, outcome: String },
    /// A model turn (#63), as text.
    Turn { text: String },
    /// The status changed.
    Status {
        from: Status,
        to: Status,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Something a human said (steering, #61).
    Note { by: String, text: String },
    /// An out-of-band callback hit one of the task's payloads (#1): the
    /// hit's id, its protocol, and the exchange that carried the payload.
    Interaction {
        id: String,
        protocol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exchange: Option<String>,
    },
    /// Quad mode started or ended while the task was live (#66).
    Quad {
        phase: String,
        workers: u32,
        seconds: u64,
    },
    /// A command run in the task's sandbox (#13): the argv, its exit code
    /// (none when killed), how long it took, and what became of it.
    Sandbox {
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i32>,
        ms: u64,
        outcome: String,
        /// Requests it sent through taphttp, and ones taphttp refused
        /// (slice 2); absent without a network.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sent: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<u32>,
    },
}

// No deny_unknown_fields: serde doesn't support it with flatten (the
// event's own fields are still checked by its tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub ts: String,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// `T-<n>`, counting up per engagement.
    pub id: String,
    pub engagement: String,
    pub goal: String,
    pub role: Role,
    /// Who runs it: an outside MCP client, or spall's own host.
    #[serde(default)]
    pub runner: Runner,
    pub constraints: Constraints,
    pub plan: Vec<Step>,
    pub status: Status,
    /// Why it's waiting, halted or failed.
    pub reason: Option<String>,
    /// Requests charged against the budget.
    pub spent: u32,
    /// Who asked for it (`human`, later an orchestrator plan).
    pub asked_by: String,
    /// The asker's note ("my boss said …").
    pub note: String,
    pub created: String,
    pub updated: String,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub log: Vec<Entry>,
    /// While waiting on approval: the exact request that paused it (#55),
    /// for the approval to show and bind its token to (#56).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// An "approve once" not yet used (#56).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
    /// What its model has used, when spall's host runs it (#63).
    #[serde(default)]
    pub usage: ModelUsage,
    /// The agent's report on its work (#81); filing again replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
}

/// A request a task paused on, exactly as it would be sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub method: String,
    pub target: String,
    pub host_header: Option<String>,
    /// taphttp's `POST /replay` body, attribution included.
    pub request: serde_json::Value,
    /// SHA-256 of that body's canonical bytes (hex): what an approval binds to.
    pub fingerprint: String,
    /// What it needs, which decides what "approve for this task" widens.
    pub ask: Ask,
    /// Why it needs a human.
    pub reason: String,
    pub asked: String,
}

/// What a paused request asks for (#56).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ask {
    /// A host the task wasn't given.
    Host { host: String },
    /// A method the task wasn't given.
    Method { method: String },
    /// A method the task sends only with approval.
    Unsafe { method: String },
    /// The rules of engagement want this confirmed: only "once" can.
    Confirm,
    /// The task's time limit is used up.
    Time,
    /// The task's budget is spent.
    Budget,
}

/// "Approve once" (#56): the one request with this fingerprint may be sent
/// once, before `expires`, past the task's own constraints (never the
/// engagement's). Held by spall, never given to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub fingerprint: String,
    pub expires: String,
}

/// How long an "approve once" waits for the agent's retry.
pub const GRANT_MINUTES: i64 = 10;

/// A human's answer to a paused request.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    /// This exact request, once.
    ApproveOnce,
    /// Widen the task for what was asked (within the RoE).
    ApproveForTask,
    /// No; the task goes on, told why, and can try another way.
    Deny { reason: String },
}

impl Decision {
    pub fn name(&self) -> &'static str {
        match self {
            Decision::ApproveOnce => "approve_once",
            Decision::ApproveForTask => "approve_for_task",
            Decision::Deny { .. } => "deny",
        }
    }
}

impl Constraints {
    /// These constraints, as a request to resolve again.
    fn requested(&self) -> Requested {
        Requested {
            hosts: Some(self.hosts.clone()),
            methods: if self.methods.is_empty() {
                None
            } else {
                Some(self.methods.clone())
            },
            safe_methods: Some(self.safe_methods.clone()),
            rate_limit: self.rate_limit,
            destructive: Some(self.destructive),
            budget: Some(self.budget),
            time_limit_minutes: Some(self.time_limit_minutes),
            stop: Some(self.stop.clone()),
            tokens: Some(self.tokens),
        }
    }

    /// What "approve for this task" asks to change, before the RoE has its
    /// say (`resolve` refuses anything past it). Budget and time double.
    /// A method added to the task is sent without asking only if the RoE
    /// already treats it as safe.
    pub fn widened(&self, ask: &Ask, rules: &Rules) -> Result<Requested, String> {
        let mut r = self.requested();
        fn add(list: &mut Option<Vec<String>>, v: &str) {
            let l = list.get_or_insert_with(Vec::new);
            if !l.iter().any(|x| x.eq_ignore_ascii_case(v)) {
                l.push(v.to_string());
            }
        }
        match ask {
            Ask::Host { host } => add(&mut r.hosts, host),
            Ask::Method { method } => {
                if r.methods.is_some() {
                    add(&mut r.methods, method);
                }
                if rules.is_safe(method) {
                    add(&mut r.safe_methods, method);
                }
            }
            Ask::Unsafe { method } => add(&mut r.safe_methods, method),
            Ask::Confirm => {
                return Err("only the rules of engagement can lift that: approve it once".into())
            }
            Ask::Time => {
                r.time_limit_minutes =
                    Some(self.time_limit_minutes.saturating_mul(2).min(MAX_MINUTES));
            }
            Ask::Budget => r.budget = Some(self.budget.saturating_mul(2).min(MAX_BUDGET)),
        }
        Ok(r)
    }
}

impl Task {
    /// Without the log, for lists.
    pub fn summary(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("log");
            o.insert("log_len".into(), self.log.len().into());
        }
        v
    }
}

/// A new task, as asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    pub goal: String,
    pub role: Role,
    /// Who runs it; an outside MCP client when left out.
    #[serde(default)]
    pub runner: Option<Runner>,
    #[serde(default)]
    pub constraints: Requested,
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default)]
    pub note: String,
}

fn one_line(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

fn check_text(what: &str, s: &str, max: usize, multiline: bool) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("{what}: at most {max} characters"));
    }
    let ok = if multiline {
        !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
    } else {
        one_line(s)
    };
    if !ok {
        return Err(format!("{what}: no control characters"));
    }
    Ok(())
}

fn check_plan(plan: &[String]) -> Result<Vec<Step>, String> {
    if plan.len() > MAX_STEPS {
        return Err(format!("plan: at most {MAX_STEPS} steps"));
    }
    plan.iter()
        .map(|s| {
            let t = s.trim();
            if t.is_empty() {
                return Err("plan: an empty step".to_string());
            }
            check_text("plan step", t, MAX_STEP, false)?;
            Ok(Step {
                text: t.into(),
                done: false,
            })
        })
        .collect()
}

fn check_goal(goal: &str) -> Result<String, String> {
    let g = goal.trim();
    if g.is_empty() {
        return Err("goal: say what the task is for".into());
    }
    check_text("goal", g, MAX_GOAL, true)?;
    Ok(g.into())
}

/// What a human may do to a task over the API. The machine moves (start,
/// wait, resume, finish, fail) belong to the agent side (#55) and aren't
/// here.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Draft → approved: a human says it may run.
    Approve,
    /// Stop it, whatever it's doing.
    Halt {
        #[serde(default)]
        reason: String,
    },
    /// Steer it: a human's message, kept in its log for the agent (#61).
    Note { text: String },
    /// Change a draft (nothing else: an approved task is what was approved).
    Edit {
        goal: Option<String>,
        role: Option<Role>,
        runner: Option<Runner>,
        constraints: Option<Requested>,
        plan: Option<Vec<String>>,
        note: Option<String>,
    },
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::Approve => "approve",
            Op::Halt { .. } => "halt",
            Op::Note { .. } => "note",
            Op::Edit { .. } => "edit",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    /// Not allowed from where the task stands.
    Conflict(String),
    Full,
    /// The task has spent its request budget.
    Spent {
        id: String,
        budget: u32,
    },
    Save(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no task {id}"),
            Error::Invalid(m) | Error::Conflict(m) => f.write_str(m),
            Error::Full => write!(f, "this engagement has {MAX_PER_ENGAGEMENT} tasks already"),
            Error::Spent { id, budget } => {
                write!(f, "{id} has spent its budget of {budget} requests")
            }
            Error::Save(m) => write!(f, "saving tasks: {m}"),
        }
    }
}

/// What the engagement looks like when a task is made or changed.
pub struct Context<'a> {
    pub rules: &'a Rules,
    pub scope: &'a Scope,
    pub now: &'a str,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBody {
    schema_version: u32,
    engagement: String,
    /// The next task's number.
    next: u64,
    tasks: Vec<Task>,
}

#[derive(Default, Clone)]
struct Book {
    next: u64,
    tasks: Vec<Task>,
}

/// The agent e-stop (#57), while engaged. Saved beside the task files,
/// so spall comes back up stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EStop {
    pub since: String,
    pub by: String,
    pub reason: String,
}

/// Not `.json`, so `Store::open` never takes it for an engagement's tasks.
const ESTOP_FILE: &str = "estop";

/// What engaging the e-stop did.
#[derive(Debug, Serialize)]
pub struct Stopped {
    pub estop: EStop,
    /// `engagement/T-n` for every task it halted.
    pub halted: Vec<String>,
    /// What couldn't be saved. The stop holds in memory regardless.
    pub errors: Vec<String>,
}

/// Every engagement's tasks, loaded at start.
pub struct Store {
    dir: PathBuf,
    by_engagement: Mutex<HashMap<String, Book>>,
    /// Up the moment the e-stop is engaged, before anything else happens;
    /// `charge` checks it under the book lock, so nothing not yet charged
    /// is sent after it.
    stopped: AtomicBool,
    estop: Mutex<Option<EStop>>,
    /// Content-free alerts for the tester (#86): raised here when a task
    /// starts waiting on approval or finishes, and by OOB hits and quad.
    pub alerts: crate::alerts::Alerts,
}

impl Store {
    /// Load `dir` (created if missing, mode 0700). A file that doesn't
    /// parse, or comes from a newer spall, stops spall: tasks never go
    /// silently missing.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        create_dir_private(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut by_engagement = HashMap::new();
        for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let (eng, book) = load(&path)?;
            by_engagement.insert(eng, book);
        }
        let estop = load_estop(&dir.join(ESTOP_FILE));
        if let Some(e) = &estop {
            tracing::warn!(
                "the agent e-stop is engaged (since {}): {}",
                e.since,
                e.reason
            );
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            by_engagement: Mutex::new(by_engagement),
            stopped: AtomicBool::new(estop.is_some()),
            estop: Mutex::new(estop),
            alerts: crate::alerts::Alerts::default(),
        })
    }

    /// The e-stop, if engaged.
    pub fn estop(&self) -> Option<EStop> {
        self.estop.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Engage the e-stop (#57): from this instant no task is charged, so
    /// no agent request not yet sent is sent; then it's saved, and every
    /// unfinished task in every engagement is halted, for good. Engaging
    /// it again halts anything new and keeps the first `since`.
    pub fn engage(&self, by: &str, reason: &str, now: &str) -> Stopped {
        self.stopped.store(true, Ordering::SeqCst);
        let estop = {
            let mut e = self.estop.lock().unwrap_or_else(|p| p.into_inner());
            e.get_or_insert_with(|| EStop {
                since: now.into(),
                by: by.into(),
                reason: reason.into(),
            })
            .clone()
        };
        let mut errors = Vec::new();
        match serde_json::to_vec_pretty(&estop) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.dir.join(ESTOP_FILE), &bytes) {
                    errors.push(format!("saving the e-stop: {e:#}"));
                }
            }
            Err(e) => errors.push(format!("saving the e-stop: {e}")),
        }
        let why = format!("e-stop: {}", estop.reason);
        let mut halted = Vec::new();
        let mut all = self.lock();
        for (eng, book) in all.iter_mut() {
            let mut changed = false;
            for t in book.tasks.iter_mut().filter(|t| !t.status.finished()) {
                if move_to(t, Status::Halted, Some(why.clone()), now).is_ok() {
                    halted.push(format!("{eng}/{}", t.id));
                    changed = true;
                }
            }
            if changed {
                if let Err(e) = save(&self.dir, eng, book) {
                    errors.push(format!("saving {eng}'s tasks: {e:#}"));
                }
            }
        }
        Stopped {
            estop,
            halted,
            errors,
        }
    }

    /// Clear the e-stop: new tasks may be approved and run again. Halted
    /// tasks stay halted. Returns the stop that was cleared, if any.
    pub fn clear_estop(&self) -> Result<Option<EStop>, Error> {
        let mut e = self.estop.lock().unwrap_or_else(|p| p.into_inner());
        match fs::remove_file(self.dir.join(ESTOP_FILE)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(Error::Save(format!("removing the e-stop: {err}"))),
        }
        self.stopped.store(false, Ordering::SeqCst);
        Ok(e.take())
    }

    fn stopped(&self) -> Result<(), Error> {
        if self.stopped.load(Ordering::SeqCst) {
            Err(Error::Conflict(
                "the agent e-stop is engaged: no task runs until a human clears it".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Book>> {
        self.by_engagement.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The engagement's tasks, newest first.
    pub fn list(&self, engagement: &str) -> Vec<Task> {
        let mut v = self
            .lock()
            .get(engagement)
            .map(|b| b.tasks.clone())
            .unwrap_or_default();
        v.reverse();
        v
    }

    pub fn get(&self, engagement: &str, id: &str) -> Option<Task> {
        self.lock()
            .get(engagement)?
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    /// Change the engagement's book with `f` and save; on any error,
    /// including a failed save, nothing changes.
    fn change<T>(
        &self,
        engagement: &str,
        f: impl FnOnce(&mut Book) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut all = self.lock();
        let mut book = all.get(engagement).cloned().unwrap_or_default();
        let before: HashMap<String, Status> = book
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.status))
            .collect();
        let out = f(&mut book)?;
        save(&self.dir, engagement, &book).map_err(|e| Error::Save(format!("{e:#}")))?;
        // Alerts (#86): a task that now waits on you, or has finished.
        for t in &book.tasks {
            if before.get(&t.id) == Some(&t.status) {
                continue;
            }
            use crate::alerts::Kind;
            let kind = match t.status {
                Status::WaitingOnApproval => Kind::Waiting,
                Status::Done => Kind::Done,
                Status::Failed => Kind::Failed,
                Status::Halted => Kind::Halted,
                _ => continue,
            };
            self.alerts.raise(kind, engagement, Some(&t.id), None);
        }
        all.insert(engagement.into(), book);
        Ok(out)
    }

    /// Make a draft task. The caller has checked [`authorized`].
    pub fn create(
        &self,
        engagement: &str,
        new: &New,
        asked_by: &str,
        cx: &Context,
    ) -> Result<Task, Error> {
        let goal = check_goal(&new.goal).map_err(Error::Invalid)?;
        let plan = check_plan(&new.plan).map_err(Error::Invalid)?;
        check_text("note", &new.note, MAX_NOTE, true).map_err(Error::Invalid)?;
        let constraints = resolve(&new.constraints, cx.rules, cx.scope).map_err(Error::Invalid)?;
        role_fits(new.role, &constraints).map_err(Error::Invalid)?;
        self.change(engagement, |book| {
            if book.tasks.len() >= MAX_PER_ENGAGEMENT {
                return Err(Error::Full);
            }
            book.next = book.next.max(1);
            let t = Task {
                id: format!("T-{}", book.next),
                engagement: engagement.into(),
                goal,
                role: new.role,
                runner: new.runner.unwrap_or_default(),
                constraints,
                plan,
                status: Status::Draft,
                reason: None,
                spent: 0,
                asked_by: asked_by.into(),
                note: new.note.trim().into(),
                created: cx.now.into(),
                updated: cx.now.into(),
                started: None,
                ended: None,
                log: Vec::new(),
                pending: None,
                grant: None,
                usage: ModelUsage::default(),
                report: None,
            };
            book.next += 1;
            book.tasks.push(t.clone());
            Ok(t)
        })
    }

    /// Apply a human's `op` to task `id`. Returns it before and after.
    pub fn apply(
        &self,
        engagement: &str,
        id: &str,
        op: &Op,
        cx: &Context,
    ) -> Result<(Task, Task), Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            let before = t.clone();
            match op {
                Op::Approve => {
                    self.stopped()?;
                    authorize_rules(cx)?;
                    move_to(t, Status::Approved, None, cx.now)?;
                }
                Op::Note { text } => {
                    let t2 = text.trim();
                    if t2.is_empty() {
                        return Err(Error::Invalid("note: say something".into()));
                    }
                    check_text("note", t2, MAX_NOTE, true).map_err(Error::Invalid)?;
                    if t.status.finished() {
                        return Err(Error::Conflict(format!(
                            "{} is {}: nobody is listening",
                            t.id,
                            t.status.as_str()
                        )));
                    }
                    push(
                        t,
                        Event::Note {
                            by: "human".into(),
                            text: t2.into(),
                        },
                        cx.now,
                    )?;
                }
                Op::Halt { reason } => {
                    check_text("reason", reason, MAX_NOTE, false).map_err(Error::Invalid)?;
                    let r = reason.trim();
                    let why = if r.is_empty() { "halted by a human" } else { r };
                    move_to(t, Status::Halted, Some(why.into()), cx.now)?;
                }
                Op::Edit {
                    goal,
                    role,
                    runner,
                    constraints,
                    plan,
                    note,
                } => {
                    if t.status != Status::Draft {
                        return Err(Error::Conflict(format!(
                            "{} is {}: only a draft can be edited",
                            t.id,
                            t.status.as_str()
                        )));
                    }
                    if let Some(g) = goal {
                        t.goal = check_goal(g).map_err(Error::Invalid)?;
                    }
                    if let Some(r) = role {
                        t.role = *r;
                    }
                    if let Some(r) = runner {
                        t.runner = *r;
                    }
                    if let Some(c) = constraints {
                        t.constraints = resolve(c, cx.rules, cx.scope).map_err(Error::Invalid)?;
                    }
                    if let Some(p) = plan {
                        t.plan = check_plan(p).map_err(Error::Invalid)?;
                    }
                    role_fits(t.role, &t.constraints).map_err(Error::Invalid)?;
                    if let Some(n) = note {
                        check_text("note", n, MAX_NOTE, true).map_err(Error::Invalid)?;
                        t.note = n.trim().into();
                    }
                    t.updated = cx.now.into();
                }
            }
            Ok((before, t.clone()))
        })
    }

    /// Move task `id` to `to` (the agent side's moves: start, wait,
    /// resume, done, failed). Illegal moves are errors.
    pub fn transition(
        &self,
        engagement: &str,
        id: &str,
        to: Status,
        reason: Option<String>,
        now: &str,
    ) -> Result<Task, Error> {
        self.change(engagement, |book| {
            if matches!(to, Status::Running | Status::Approved) {
                self.stopped()?;
            }
            let t = find(book, id)?;
            move_to(t, to, reason, now)?;
            Ok(t.clone())
        })
    }

    /// Add to task `id`'s log. A finished task's log is closed.
    pub fn log(&self, engagement: &str, id: &str, event: Event, now: &str) -> Result<(), Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            if t.status.finished() {
                return Err(Error::Conflict(format!(
                    "{} is {}",
                    t.id,
                    t.status.as_str()
                )));
            }
            push(t, event, now)
        })
    }

    /// File the agent's report on task `id` (#81), replacing any before.
    /// Only while the task is unfinished: a finished task's report is final.
    /// `done` ends it as done, which only a running task can: one waiting on
    /// the tester can't finish around the request. All or nothing.
    pub fn report(
        &self,
        engagement: &str,
        id: &str,
        text: &str,
        done: bool,
        now: &str,
    ) -> Result<Task, Error> {
        let text = text.replace("\r\n", "\n");
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("report: empty".into()));
        }
        check_text("report", text, MAX_REPORT, true).map_err(Error::Invalid)?;
        self.change(engagement, |book| {
            let t = find(book, id)?;
            if t.status.finished() {
                return Err(Error::Conflict(format!(
                    "{} is {}: its report is final",
                    t.id,
                    t.status.as_str()
                )));
            }
            if done && t.status != Status::Running {
                return Err(Error::Conflict(format!(
                    "{} is {}: only a running task can finish",
                    t.id,
                    t.status.as_str()
                )));
            }
            t.report = Some(Report {
                text: text.into(),
                at: now.into(),
            });
            t.updated = now.into();
            if done {
                move_to(t, Status::Done, None, now)?;
            }
            Ok(t.clone())
        })
    }

    /// Pause running task `id` on `pending`: it waits on a human (#56).
    /// Only from running; a task already waiting stays as it is.
    pub fn pause(&self, engagement: &str, id: &str, pending: Pending) -> Result<Task, Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            let now = pending.asked.clone();
            let what = format!("{} {}: {}", pending.method, pending.target, pending.reason);
            move_to(
                t,
                Status::WaitingOnApproval,
                Some(pending.reason.clone()),
                &now,
            )?;
            t.pending = Some(pending);
            push(
                t,
                Event::Approval {
                    what,
                    outcome: "asked".into(),
                },
                &now,
            )?;
            Ok(t.clone())
        })
    }

    /// Log a request sent under task `id`, then apply its stop conditions:
    /// reaching one halts the task (for good; the work is given again).
    pub fn log_send(
        &self,
        engagement: &str,
        id: &str,
        event: Event,
        now: &str,
    ) -> Result<Task, Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            if t.status.finished() {
                return Err(Error::Conflict(format!(
                    "{} is {}",
                    t.id,
                    t.status.as_str()
                )));
            }
            push(t, event, now)?;
            let count = |f: fn(u16) -> bool| {
                t.log
                    .iter()
                    .filter(|e| matches!(e.event, Event::Request { status: Some(s), .. } if f(s)))
                    .count()
            };
            let stop = &t.constraints.stop;
            let hit = match (stop.server_errors, stop.throttled) {
                (Some(n), _) if count(|s| (500..600).contains(&s)) >= n as usize => {
                    Some(format!("stop condition: {n} server errors (5xx)"))
                }
                (_, Some(n)) if count(|s| s == 429) >= n as usize => {
                    Some(format!("stop condition: throttled {n} times (429)"))
                }
                _ => None,
            };
            if let Some(why) = hit {
                move_to(t, Status::Halted, Some(why), now)?;
            }
            Ok(t.clone())
        })
    }

    /// A human's answer to the request task `id` paused on (#56). Approving
    /// resumes it with a grant for that one request (once) or with wider
    /// constraints (for the task, never past the RoE); denying resumes it
    /// with the reason, so the agent can try another way.
    pub fn decide(
        &self,
        engagement: &str,
        id: &str,
        d: &Decision,
        cx: &Context,
    ) -> Result<(Task, Task), Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            let before = t.clone();
            let p = match (&t.status, &t.pending) {
                (Status::WaitingOnApproval, Some(p)) => p.clone(),
                _ => {
                    return Err(Error::Conflict(format!(
                        "{} isn't waiting on approval (it is {})",
                        t.id,
                        t.status.as_str()
                    )))
                }
            };
            let what = format!("{} {}", p.method, p.target);
            match d {
                Decision::ApproveOnce => {
                    self.stopped()?;
                    authorize_rules(cx)?;
                    let expires = cx
                        .now
                        .parse::<jiff::Timestamp>()
                        .ok()
                        .and_then(|n| {
                            n.checked_add(jiff::SignedDuration::from_mins(GRANT_MINUTES))
                                .ok()
                        })
                        .map(|t| t.to_string())
                        .ok_or_else(|| Error::Invalid(format!("bad time {:?}", cx.now)))?;
                    let ev = Event::Approval {
                        what,
                        outcome: "approved once".into(),
                    };
                    push(t, ev, cx.now)?;
                    move_to(t, Status::Running, None, cx.now)?;
                    t.grant = Some(Grant {
                        fingerprint: p.fingerprint.clone(),
                        expires,
                    });
                }
                Decision::ApproveForTask => {
                    self.stopped()?;
                    authorize_rules(cx)?;
                    let req = t
                        .constraints
                        .widened(&p.ask, cx.rules)
                        .map_err(Error::Invalid)?;
                    t.constraints = resolve(&req, cx.rules, cx.scope).map_err(|e| {
                        Error::Invalid(format!("not within the rules of engagement: {e}"))
                    })?;
                    let ev = Event::Approval {
                        what,
                        outcome: "approved for the task".into(),
                    };
                    push(t, ev, cx.now)?;
                    move_to(t, Status::Running, None, cx.now)?;
                }
                Decision::Deny { reason } => {
                    let r = reason.trim();
                    if r.is_empty() {
                        return Err(Error::Invalid(
                            "deny: say why, so the agent can try another way".into(),
                        ));
                    }
                    check_text("reason", r, MAX_NOTE, false).map_err(Error::Invalid)?;
                    let ev = Event::Approval {
                        what: what.clone(),
                        outcome: format!("denied: {r}"),
                    };
                    push(t, ev, cx.now)?;
                    let why = format!("{what} was denied: {r}");
                    move_to(t, Status::Running, Some(why), cx.now)?;
                }
            }
            Ok((before, t.clone()))
        })
    }

    /// How many of the engagement's tasks wait on a human.
    pub fn waiting(&self, engagement: &str) -> usize {
        self.lock()
            .get(engagement)
            .map(|b| {
                b.tasks
                    .iter()
                    .filter(|t| t.status == Status::WaitingOnApproval)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Charge one request to task `id`: only while it runs and has budget
    /// left. Checked and counted under one lock, so racing sends can't
    /// overspend.
    /// Count one model call against task `id` (#63), whatever its status:
    /// the tokens were spent. Past its token cap an unfinished task is
    /// halted, for good. Returns the task after.
    pub fn charge_model(
        &self,
        engagement: &str,
        id: &str,
        used: crate::model::Usage,
        ms: u64,
        now: &str,
    ) -> Result<Task, Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            let u = &mut t.usage;
            u.calls = u.calls.saturating_add(1);
            u.input_tokens = u.input_tokens.saturating_add(used.input_tokens);
            u.output_tokens = u.output_tokens.saturating_add(used.output_tokens);
            u.model_ms = u.model_ms.saturating_add(ms);
            t.updated = now.into();
            let (spent, cap) = (t.usage.tokens(), t.constraints.tokens);
            if spent >= cap && !t.status.finished() {
                move_to(
                    t,
                    Status::Halted,
                    Some(format!(
                        "used its model token budget ({spent} of {cap} tokens)"
                    )),
                    now,
                )?;
            }
            Ok(t.clone())
        })
    }

    pub fn charge(&self, engagement: &str, id: &str, now: &str) -> Result<u32, Error> {
        self.charge_as(engagement, id, now, None)
    }

    /// Record `n` requests a task already sent (#13 slice 2: what taphttp
    /// counted on a sandbox command's credential, whose budget was the
    /// task's remaining one). Counted whatever the task's state: it's what
    /// happened, not a permission.
    pub fn spend(&self, engagement: &str, id: &str, n: u32, now: &str) -> Result<(), Error> {
        if n == 0 {
            return Ok(());
        }
        self.change(engagement, |book| {
            let t = find(book, id)?;
            t.spent = t.spent.saturating_add(n);
            t.updated = now.into();
            Ok(())
        })
    }

    /// [`Store::charge`] for a request sent on an "approve once": the
    /// grant must be there, for this fingerprint, unexpired, and is used
    /// up here, under the same lock. It may go past the budget.
    pub fn charge_granted(
        &self,
        engagement: &str,
        id: &str,
        now: &str,
        fingerprint: &str,
    ) -> Result<u32, Error> {
        self.charge_as(engagement, id, now, Some(fingerprint))
    }

    fn charge_as(
        &self,
        engagement: &str,
        id: &str,
        now: &str,
        granted: Option<&str>,
    ) -> Result<u32, Error> {
        self.change(engagement, |book| {
            self.stopped()?;
            let t = find(book, id)?;
            if t.status != Status::Running {
                return Err(Error::Conflict(format!(
                    "{} is {}: only a running task sends",
                    t.id,
                    t.status.as_str()
                )));
            }
            if let Some(fp) = granted {
                let good = t
                    .grant
                    .as_ref()
                    .is_some_and(|g| g.fingerprint == fp && !expired(&g.expires, now));
                if !good {
                    return Err(Error::Conflict(format!(
                        "{}: no approval for this request (used, expired, or for another one)",
                        t.id
                    )));
                }
                t.grant = None;
                t.spent += 1;
                t.updated = now.into();
                return Ok(t.constraints.budget.saturating_sub(t.spent));
            }
            if t.spent >= t.constraints.budget {
                return Err(Error::Spent {
                    id: t.id.clone(),
                    budget: t.constraints.budget,
                });
            }
            t.spent += 1;
            t.updated = now.into();
            Ok(t.constraints.budget - t.spent)
        })
    }
}

/// Whether `expires` is at or before `now` (both RFC 3339); unreadable
/// counts as expired.
fn expired(expires: &str, now: &str) -> bool {
    match (
        expires.parse::<jiff::Timestamp>(),
        now.parse::<jiff::Timestamp>(),
    ) {
        (Ok(e), Ok(n)) => e <= n,
        _ => true,
    }
}

fn authorize_rules(cx: &Context) -> Result<(), Error> {
    if !cx.rules.agents_allowed {
        return Err(Error::Conflict(
            "the rules of engagement don't allow agents".into(),
        ));
    }
    Ok(())
}

fn find<'a>(book: &'a mut Book, id: &str) -> Result<&'a mut Task, Error> {
    book.tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| Error::NotFound(id.into()))
}

fn push(t: &mut Task, event: Event, now: &str) -> Result<(), Error> {
    if t.log.len() >= MAX_LOG {
        return Err(Error::Full);
    }
    t.log.push(Entry {
        ts: now.into(),
        event,
    });
    t.updated = now.into();
    Ok(())
}

fn move_to(t: &mut Task, to: Status, reason: Option<String>, now: &str) -> Result<(), Error> {
    let from = t.status;
    if !from.can_become(to) {
        return Err(Error::Conflict(format!(
            "{} is {}: it can't become {}",
            t.id,
            from.as_str(),
            to.as_str()
        )));
    }
    t.status = to;
    t.reason = reason.clone();
    // Leaving the wait (resumed, failed, halted) drops the request: an
    // approval can never outlive it.
    if to != Status::WaitingOnApproval {
        t.pending = None;
    }
    // A finished task's approvals die with it.
    if to.finished() {
        t.grant = None;
    }
    if to == Status::Running && t.started.is_none() {
        t.started = Some(now.into());
    }
    if to.finished() {
        t.ended = Some(now.into());
    }
    // A full log doesn't stop a halt: the status is what matters.
    let _ = push(t, Event::Status { from, to, reason }, now);
    t.updated = now.into();
    Ok(())
}

/// The saved e-stop. A file that is there but can't be read counts as
/// engaged: failing safe beats a stop that silently disappears.
fn load_estop(path: &Path) -> Option<EStop> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(unreadable_estop(&e.to_string())),
    };
    Some(serde_json::from_str(&text).unwrap_or_else(|e| unreadable_estop(&e.to_string())))
}

fn unreadable_estop(why: &str) -> EStop {
    tracing::error!("the e-stop file can't be read ({why}); treating the e-stop as engaged");
    EStop {
        since: "unknown".into(),
        by: "spall".into(),
        reason: format!("the saved e-stop couldn't be read ({why}), so it holds"),
    }
}

fn load(path: &Path) -> anyhow::Result<(String, Book)> {
    use anyhow::Context as _;
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{}: not JSON", path.display()))?;
    let version = v.get("schema_version").and_then(|s| s.as_u64());
    anyhow::ensure!(
        version == Some(u64::from(SCHEMA_VERSION)),
        "{}: schema_version {version:?}; this spall reads {SCHEMA_VERSION} (upgrade spall, or move the file aside)",
        path.display()
    );
    let body: FileBody = serde_json::from_value(v)
        .with_context(|| format!("{}: not a tasks file", path.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str());
    anyhow::ensure!(
        stem == Some(body.engagement.as_str()),
        "{}: engagement {:?} doesn't match the file name",
        path.display(),
        body.engagement
    );
    anyhow::ensure!(
        body.tasks.iter().all(|t| t.engagement == body.engagement),
        "{}: holds another engagement's task",
        path.display()
    );
    Ok((
        body.engagement,
        Book {
            next: body.next,
            tasks: body.tasks,
        },
    ))
}

fn save(dir: &Path, engagement: &str, book: &Book) -> anyhow::Result<()> {
    let body = FileBody {
        schema_version: SCHEMA_VERSION,
        engagement: engagement.into(),
        next: book.next,
        tasks: book.tasks.clone(),
    };
    let mut bytes = serde_json::to_vec_pretty(&body)?;
    bytes.push(b'\n');
    write_atomic(&dir.join(format!("{engagement}.json")), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roe::{Actor, Class};

    const NOW: &str = "2026-11-23T15:00:00Z";

    fn rules() -> Rules {
        Rules {
            agents_allowed: true,
            methods: vec!["GET".into(), "POST".into(), "PUT".into()],
            safe_methods: vec!["GET".into(), "HEAD".into()],
            rate_limit: Some(5.0),
            ..Rules::default()
        }
    }

    fn scope() -> Scope {
        Scope::parse("app.test, *.api.test, !admin.api.test").unwrap()
    }

    fn new(goal: &str) -> New {
        New {
            goal: goal.into(),
            role: Role::Mapper,
            runner: None,
            constraints: Requested::default(),
            plan: vec!["crawl".into(), "replay GETs".into()],
            note: String::new(),
        }
    }

    #[test]
    fn the_status_machine() {
        use Status::*;
        let legal = [
            (Draft, Approved),
            (Approved, Running),
            (Running, WaitingOnApproval),
            (WaitingOnApproval, Running),
            (Running, Done),
            (Running, Failed),
            (WaitingOnApproval, Failed),
            (Draft, Halted),
            (Approved, Halted),
            (Running, Halted),
            (WaitingOnApproval, Halted),
        ];
        for from in Status::ALL {
            for to in Status::ALL {
                assert_eq!(
                    from.can_become(to),
                    legal.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
        for s in [Done, Halted, Failed] {
            assert!(s.finished() && Status::ALL.iter().all(|t| !s.can_become(*t)));
        }
        assert_eq!(
            serde_json::to_value(WaitingOnApproval).unwrap(),
            "waiting_on_approval"
        );
    }

    #[test]
    fn left_out_constraints_come_from_the_roe() {
        let c = resolve(&Requested::default(), &rules(), &scope()).unwrap();
        assert!(c.hosts.is_empty());
        assert_eq!(c.methods, ["GET", "POST", "PUT"]);
        assert_eq!(c.safe_methods, ["GET"], "HEAD isn't one of the methods");
        assert_eq!(c.rate_limit, Some(5.0));
        assert!(!c.destructive);
        assert_eq!(
            (c.budget, c.time_limit_minutes),
            (DEFAULT_BUDGET, DEFAULT_MINUTES)
        );
        let any = Rules {
            methods: vec![],
            ..rules()
        };
        let c = resolve(&Requested::default(), &any, &scope()).unwrap();
        assert!(
            c.methods.is_empty(),
            "the RoE allows any method; so does the task"
        );
        assert_eq!(c.safe_methods, ["GET", "HEAD"]);
    }

    #[test]
    fn asking_for_more_than_the_roe_is_refused_with_a_reason() {
        let r = rules();
        let s = scope();
        let err = |req: Requested| resolve(&req, &r, &s).unwrap_err();
        assert!(err(Requested {
            methods: Some(vec!["DELETE".into()]),
            ..Default::default()
        })
        .contains("DELETE isn't allowed by the rules of engagement"));
        assert!(err(Requested {
            methods: Some(vec![]),
            ..Default::default()
        })
        .contains("at least one"));
        assert!(err(Requested {
            methods: Some(vec!["get".into()]),
            ..Default::default()
        })
        .contains("uppercase"));
        assert!(err(Requested {
            safe_methods: Some(vec!["POST".into()]),
            ..Default::default()
        })
        .contains("want POST confirmed"));
        assert!(err(Requested {
            methods: Some(vec!["POST".into()]),
            safe_methods: Some(vec!["GET".into()]),
            ..Default::default()
        })
        .contains("isn't one of the task's methods"));
        assert!(err(Requested {
            rate_limit: Some(6.0),
            ..Default::default()
        })
        .contains("RoE's 5"));
        assert!(err(Requested {
            rate_limit: Some(0.0),
            ..Default::default()
        })
        .contains("more than 0"));
        assert!(err(Requested {
            rate_limit: Some(f64::NAN),
            ..Default::default()
        })
        .contains("more than 0"));
        assert!(err(Requested {
            destructive: Some(true),
            ..Default::default()
        })
        .contains("destructive"));
        assert!(err(Requested {
            budget: Some(0),
            ..Default::default()
        })
        .contains("budget"));
        assert!(err(Requested {
            budget: Some(MAX_BUDGET + 1),
            ..Default::default()
        })
        .contains("budget"));
        assert!(err(Requested {
            time_limit_minutes: Some(0),
            ..Default::default()
        })
        .contains("time_limit"));
        assert!(err(Requested {
            stop: Some(Stop {
                server_errors: Some(0),
                throttled: None
            }),
            ..Default::default()
        })
        .contains("at least 1"));
        for (h, why) in [
            ("other.test", "isn't in the engagement's scope"),
            ("admin.api.test", "isn't in the engagement's scope"),
            ("api.test", "isn't in the engagement's scope"),
            ("*.api.test", "one by one, not patterns"),
            ("", "isn't a host name"),
        ] {
            let e = err(Requested {
                hosts: Some(vec![h.into()]),
                ..Default::default()
            });
            assert!(e.contains(why), "{h}: {e}");
        }
        let all = scope().with_all(true);
        assert!(
            resolve(
                &Requested {
                    hosts: Some(vec!["other.test".into()]),
                    ..Default::default()
                },
                &r,
                &all
            )
            .unwrap_err()
            .contains("isn't in the engagement's scope"),
            "scope all on is this process, not the engagement"
        );
    }

    #[test]
    fn narrower_constraints_are_kept() {
        let c = resolve(
            &Requested {
                hosts: Some(vec![
                    "APP.test".into(),
                    "v1.api.test:8443".into(),
                    "app.test".into(),
                ]),
                methods: Some(vec!["GET".into()]),
                safe_methods: Some(vec![]),
                rate_limit: Some(0.5),
                budget: Some(20),
                time_limit_minutes: Some(15),
                stop: Some(Stop {
                    server_errors: Some(3),
                    throttled: Some(1),
                }),
                ..Default::default()
            },
            &rules(),
            &scope(),
        )
        .unwrap();
        assert_eq!(c.hosts, ["app.test", "v1.api.test"]);
        assert_eq!(c.methods, ["GET"]);
        assert!(c.safe_methods.is_empty(), "every send asks");
        assert_eq!(
            (c.rate_limit, c.budget, c.time_limit_minutes),
            (Some(0.5), 20, 15)
        );
        assert!(c.allows_host(&scope(), "app.test:443"));
        assert!(
            !c.allows_host(&scope(), "v2.api.test"),
            "in scope, not in the task"
        );
        let open = resolve(&Requested::default(), &rules(), &scope()).unwrap();
        assert!(open.allows_host(&scope(), "v2.api.test"));
        assert!(!open.allows_host(&scope(), "admin.api.test"));
    }

    /// No combination of asked-for constraints, against any of these RoEs,
    /// resolves to anything wider than the RoE.
    #[test]
    fn no_constraint_combination_widens_the_roe() {
        let roes = [
            rules(),
            Rules {
                methods: vec![],
                ..rules()
            },
            Rules {
                rate_limit: None,
                destructive_allowed: true,
                ..rules()
            },
            Rules {
                safe_methods: vec![],
                rate_limit: Some(0.2),
                ..rules()
            },
        ];
        let scope = scope();
        let opt = |v: Vec<&str>| Some(v.into_iter().map(String::from).collect::<Vec<_>>());
        let method_sets = [
            None,
            opt(vec!["GET"]),
            opt(vec!["POST", "DELETE"]),
            opt(vec!["PATCH"]),
            opt(vec!["GET", "HEAD"]),
        ];
        let safe_sets = [
            None,
            opt(vec![]),
            opt(vec!["GET"]),
            opt(vec!["POST"]),
            opt(vec!["HEAD", "DELETE"]),
        ];
        let rates = [None, Some(0.1), Some(5.0), Some(50.0), Some(5000.0)];
        let hosts = [
            None,
            opt(vec!["app.test"]),
            opt(vec!["x.api.test", "evil.test"]),
            opt(vec!["admin.api.test"]),
        ];
        let all_methods = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];
        let probes = [
            "app.test",
            "x.api.test",
            "admin.api.test",
            "evil.test",
            "api.test",
        ];
        let mut resolved = 0;
        for roe in &roes {
            for m in &method_sets {
                for s in &safe_sets {
                    for rate in rates {
                        for h in &hosts {
                            for destructive in [None, Some(true), Some(false)] {
                                let req = Requested {
                                    hosts: h.clone(),
                                    methods: m.clone(),
                                    safe_methods: s.clone(),
                                    rate_limit: rate,
                                    destructive,
                                    ..Default::default()
                                };
                                let Ok(c) = resolve(&req, roe, &scope) else {
                                    continue;
                                };
                                resolved += 1;
                                let layer = c.layer(roe);
                                for method in all_methods {
                                    for class in [Class::Send, Class::Destructive] {
                                        if layer.check_action(Actor::Agent, class, method).is_ok() {
                                            assert!(
                                                roe.check_action(Actor::Agent, class, method)
                                                    .is_ok(),
                                                "{req:?} {method} {class:?}"
                                            );
                                        }
                                    }
                                    if layer.is_safe(method) {
                                        assert!(roe.is_safe(method), "{req:?}: {method} safe");
                                    }
                                }
                                if let Some(ceiling) = roe.rate_limit {
                                    assert!(c.rate_limit.is_some_and(|r| r <= ceiling), "{req:?}");
                                }
                                for p in probes {
                                    if c.allows_host(&scope, p) {
                                        assert!(scope.contains(p), "{req:?}: {p}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            resolved > 100,
            "the combinations exercise the happy path too: {resolved}"
        );
    }

    #[test]
    fn no_task_without_an_authorized_engagement_that_allows_agents() {
        let now: jiff::Timestamp = NOW.parse().unwrap();
        let p = Period::default();
        assert!(authorized(&rules(), &p, &scope(), now).is_ok());
        let no_agents = Rules {
            agents_allowed: false,
            ..rules()
        };
        assert!(authorized(&no_agents, &p, &scope(), now)
            .unwrap_err()
            .contains("agents"));
        assert!(
            authorized(&rules(), &p, &Scope::default().with_all(true), now)
                .unwrap_err()
                .contains("no scope")
        );
        let expired = Period {
            expires: Some("2026-11-01".into()),
            ..Period::default()
        };
        assert!(authorized(&rules(), &expired, &scope(), now)
            .unwrap_err()
            .contains("expired"));
        let windowed = Rules {
            windows: vec![crate::roe::Window {
                days: vec![crate::roe::Day::Sun],
                start: "01:00".into(),
                end: "02:00".into(),
            }],
            ..rules()
        };
        assert!(
            authorized(&windowed, &p, &scope(), now).is_ok(),
            "a task can be planned outside a window"
        );
    }

    fn cx<'a>(r: &'a Rules, s: &'a Scope) -> Context<'a> {
        Context {
            rules: r,
            scope: s,
            now: NOW,
        }
    }

    #[test]
    fn only_a_tester_is_given_destructive_actions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = Rules {
            destructive_allowed: true,
            ..rules()
        };
        let s = scope();
        let destructive = |role| New {
            role,
            constraints: Requested {
                destructive: Some(true),
                ..Requested::default()
            },
            ..new("break it")
        };
        for role in [Role::Scoper, Role::Mapper, Role::Reporter] {
            let e = store
                .create("eng-1", &destructive(role), "human", &cx(&r, &s))
                .unwrap_err();
            assert!(e.to_string().contains("tester"), "{role:?}: {e}");
        }
        let t = store
            .create("eng-1", &destructive(Role::Tester), "human", &cx(&r, &s))
            .unwrap();
        assert!(t.constraints.destructive);
        // Nor can a draft be edited into it.
        let e = store
            .apply(
                "eng-1",
                &t.id,
                &Op::Edit {
                    goal: None,
                    role: Some(Role::Mapper),
                    runner: None,
                    constraints: None,
                    plan: None,
                    note: None,
                },
                &cx(&r, &s),
            )
            .unwrap_err();
        assert!(e.to_string().contains("destructive"), "{e}");
        assert_eq!(store.get("eng-1", &t.id).unwrap().role, Role::Tester);
    }

    #[test]
    fn create_edit_approve_halt_and_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (r, s) = (rules(), scope());
        let t1 = store
            .create("eng-1", &new("map the API"), "human", &cx(&r, &s))
            .unwrap();
        let t2 = store
            .create("eng-1", &new("second"), "human", &cx(&r, &s))
            .unwrap();
        assert_eq!((t1.id.as_str(), t2.id.as_str()), ("T-1", "T-2"));
        assert_eq!(t1.status, Status::Draft);
        assert_eq!(
            t1.plan[0],
            Step {
                text: "crawl".into(),
                done: false
            }
        );
        assert_eq!(
            store
                .list("eng-1")
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["T-2", "T-1"]
        );

        let edit = Op::Edit {
            goal: Some("  map the v1 API ".into()),
            role: Some(Role::Tester),
            runner: None,
            constraints: Some(Requested {
                budget: Some(10),
                ..Default::default()
            }),
            plan: None,
            note: Some("my boss said".into()),
        };
        let (_, after) = store.apply("eng-1", "T-1", &edit, &cx(&r, &s)).unwrap();
        assert_eq!(
            (after.goal.as_str(), after.role, after.constraints.budget),
            ("map the v1 API", Role::Tester, 10)
        );
        let wide = Op::Edit {
            goal: None,
            role: None,
            runner: None,
            constraints: Some(Requested {
                rate_limit: Some(99.0),
                ..Default::default()
            }),
            plan: None,
            note: None,
        };
        assert!(matches!(
            store.apply("eng-1", "T-1", &wide, &cx(&r, &s)),
            Err(Error::Invalid(_))
        ));

        let no_agents = Rules {
            agents_allowed: false,
            ..rules()
        };
        assert!(matches!(
            store.apply("eng-1", "T-1", &Op::Approve, &cx(&no_agents, &s)),
            Err(Error::Conflict(_))
        ));
        let (before, after) = store
            .apply("eng-1", "T-1", &Op::Approve, &cx(&r, &s))
            .unwrap();
        assert_eq!(
            (before.status, after.status),
            (Status::Draft, Status::Approved)
        );
        assert!(
            matches!(store.apply("eng-1", "T-1", &edit, &cx(&r, &s)), Err(Error::Conflict(m)) if m.contains("only a draft"))
        );
        assert!(matches!(
            store.apply("eng-1", "T-1", &Op::Approve, &cx(&r, &s)),
            Err(Error::Conflict(_))
        ));

        // The agent side's moves, the log and the budget.
        assert!(
            matches!(store.charge("eng-1", "T-1", NOW), Err(Error::Conflict(m)) if m.contains("only a running task"))
        );
        let t = store
            .transition("eng-1", "T-1", Status::Running, None, NOW)
            .unwrap();
        assert_eq!(t.started.as_deref(), Some(NOW));
        for left in (0..10).rev() {
            assert_eq!(store.charge("eng-1", "T-1", NOW).unwrap(), left);
        }
        assert!(matches!(
            store.charge("eng-1", "T-1", NOW),
            Err(Error::Spent { budget: 10, .. })
        ));
        store
            .log(
                "eng-1",
                "T-1",
                Event::Request {
                    exchange: Some("e1".into()),
                    method: "GET".into(),
                    target: "https://app.test/".into(),
                    check: Some("WSTG-INFO-02".into()),
                    outcome: "sent".into(),
                    status: Some(200),
                },
                NOW,
            )
            .unwrap();
        assert!(matches!(
            store.transition("eng-1", "T-1", Status::Approved, None, NOW),
            Err(Error::Conflict(_))
        ));

        let (_, halted) = store
            .apply(
                "eng-1",
                "T-1",
                &Op::Halt {
                    reason: String::new(),
                },
                &cx(&r, &s),
            )
            .unwrap();
        assert_eq!(halted.status, Status::Halted);
        assert_eq!(halted.reason.as_deref(), Some("halted by a human"));
        assert_eq!(halted.ended.as_deref(), Some(NOW));
        let kinds: Vec<String> = halted
            .log
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(kinds, ["status", "status", "request", "status"]);
        assert!(matches!(
            store.log(
                "eng-1",
                "T-1",
                Event::Turn {
                    text: "more".into()
                },
                NOW
            ),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            store.apply(
                "eng-1",
                "T-1",
                &Op::Halt {
                    reason: String::new()
                },
                &cx(&r, &s)
            ),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            store.apply("eng-1", "T-9", &Op::Approve, &cx(&r, &s)),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn bad_input_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (r, s) = (rules(), scope());
        let bad = |n: New| store.create("eng-1", &n, "human", &cx(&r, &s)).unwrap_err();
        assert!(bad(new("  ")).to_string().contains("goal"));
        assert!(bad(new(&"g".repeat(MAX_GOAL + 1)))
            .to_string()
            .contains("goal"));
        assert!(bad(New {
            plan: vec!["a\u{1b}[31m".into()],
            ..new("g")
        })
        .to_string()
        .contains("control"));
        assert!(bad(New {
            plan: vec![String::new()],
            ..new("g")
        })
        .to_string()
        .contains("empty step"));
        assert!(bad(New {
            plan: vec!["s".into(); MAX_STEPS + 1],
            ..new("g")
        })
        .to_string()
        .contains("steps"));
        assert!(serde_json::from_str::<New>(r#"{"goal":"g","role":"admin"}"#).is_err());
        assert!(
            serde_json::from_str::<New>(r#"{"goal":"g","role":"mapper","status":"running"}"#)
                .is_err(),
            "status isn't asked for"
        );
        assert!(
            serde_json::from_str::<Op>(r#"{"op":"start"}"#).is_err(),
            "machine moves aren't human ops"
        );
        assert!(store.list("eng-1").is_empty(), "nothing half-made");
    }

    #[test]
    fn tasks_survive_a_restart_and_stay_in_their_engagement() {
        let dir = tempfile::tempdir().unwrap();
        let (r, s) = (rules(), scope());
        {
            let store = Store::open(dir.path()).unwrap();
            store
                .create("eng-1", &new("one"), "human", &cx(&r, &s))
                .unwrap();
            store
                .create("eng-2", &new("two"), "human", &cx(&r, &s))
                .unwrap();
            store
                .apply(
                    "eng-1",
                    "T-1",
                    &Op::Halt {
                        reason: "wrong host".into(),
                    },
                    &cx(&r, &s),
                )
                .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        let one = store.list("eng-1");
        assert_eq!(one.len(), 1);
        assert_eq!(
            (
                one[0].goal.as_str(),
                one[0].status,
                one[0].reason.as_deref()
            ),
            ("one", Status::Halted, Some("wrong host"))
        );
        assert_eq!(store.list("eng-2")[0].goal, "two");
        assert!(
            store.get("eng-2", "T-1").is_some_and(|t| t.goal == "two"),
            "ids count per engagement"
        );
        assert!(store.list("eng-3").is_empty());
        let next = store
            .create("eng-1", &new("three"), "human", &cx(&r, &s))
            .unwrap();
        assert_eq!(next.id, "T-2", "numbering survives too");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("eng-1.json")), 0o600);
        }
    }

    #[test]
    fn a_corrupt_or_newer_file_stops_the_store() {
        for (name, body, why) in [
            ("eng-1.json", "{", "not JSON"),
            (
                "eng-1.json",
                r#"{"schema_version": 2, "engagement": "eng-1", "next": 1, "tasks": []}"#,
                "schema_version",
            ),
            (
                "eng-1.json",
                r#"{"schema_version": 1, "engagement": "other", "next": 1, "tasks": []}"#,
                "doesn't match",
            ),
            (
                "eng-1.json",
                r#"{"schema_version": 1, "engagement": "eng-1", "next": 1, "tasks": [{"id": "T-1"}]}"#,
                "not a tasks file",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(name), body).unwrap();
            let err = format!("{:#}", Store::open(dir.path()).err().expect("refused"));
            assert!(err.contains(why), "{why}: {err}");
        }
    }

    #[test]
    fn racing_charges_never_overspend() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(dir.path()).unwrap());
        let (r, s) = (rules(), scope());
        let n = New {
            constraints: Requested {
                budget: Some(25),
                ..Default::default()
            },
            ..new("race")
        };
        store.create("e", &n, "human", &cx(&r, &s)).unwrap();
        store.apply("e", "T-1", &Op::Approve, &cx(&r, &s)).unwrap();
        store
            .transition("e", "T-1", Status::Running, None, NOW)
            .unwrap();
        let ok: usize = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..8)
                .map(|_| {
                    let st = store.clone();
                    sc.spawn(move || {
                        (0..10)
                            .filter(|_| st.charge("e", "T-1", NOW).is_ok())
                            .count()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert_eq!(ok, 25);
        assert_eq!(store.get("e", "T-1").unwrap().spent, 25);
    }

    #[test]
    fn a_task_keeps_its_agents_report_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(&store, Requested::default());
        assert_eq!(store.get("e", "T-1").unwrap().report, None);

        let t = store
            .report(
                "e",
                "T-1",
                "  Stack: Apache\r\nbehind Cloudflare.\n\n",
                false,
                NOW,
            )
            .unwrap();
        let r = t.report.unwrap();
        assert_eq!(
            r.text, "Stack: Apache\nbehind Cloudflare.",
            "trimmed, newlines kept"
        );
        assert_eq!(r.at, NOW);

        // Filing again replaces it; it survives a reopen.
        store.report("e", "T-1", "second", false, NOW).unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.get("e", "T-1").unwrap().report.unwrap().text,
            "second"
        );
        assert!(store.get("e", "T-1").unwrap().summary()["report"]["text"] == "second");

        for bad in ["", "   \n", "bell\u{7}", &"x".repeat(MAX_REPORT + 1)] {
            assert!(
                matches!(
                    store.report("e", "T-1", bad, false, NOW),
                    Err(Error::Invalid(_))
                ),
                "{bad:.20}"
            );
        }
        assert!(store
            .report("e", "T-1", &"x".repeat(MAX_REPORT), false, NOW)
            .is_ok());

        // A finished task's report is final.
        store
            .transition("e", "T-1", Status::Done, None, NOW)
            .unwrap();
        assert!(matches!(
            store.report("e", "T-1", "late", false, NOW),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            store.report("e", "T-9", "nobody", false, NOW),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_final_report_ends_a_running_task_as_done() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(&store, Requested::default());
        store
            .pause(
                "e",
                "T-1",
                pending(Ask::Method {
                    method: "POST".into(),
                }),
            )
            .unwrap();
        // Waiting on the tester: it can't finish around the request.
        assert!(matches!(
            store.report("e", "T-1", "all done", true, NOW),
            Err(Error::Conflict(_))
        ));
        let t = store.get("e", "T-1").unwrap();
        assert_eq!(
            (t.status, t.report),
            (Status::WaitingOnApproval, None),
            "nothing kept"
        );

        store
            .transition("e", "T-1", Status::Running, None, NOW)
            .unwrap();
        let t = store.report("e", "T-1", "all done", true, NOW).unwrap();
        assert_eq!(t.status, Status::Done);
        assert_eq!(t.ended.as_deref(), Some(NOW));
        assert_eq!(t.report.unwrap().text, "all done");
        // A bad report finishes nothing.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(&store, Requested::default());
        assert!(store.report("e", "T-1", " ", true, NOW).is_err());
        assert_eq!(store.get("e", "T-1").unwrap().status, Status::Running);
    }

    fn running(store: &Store, constraints: Requested) {
        let (r, s) = (rules(), scope());
        let n = New {
            constraints,
            ..new("t")
        };
        store.create("e", &n, "human", &cx(&r, &s)).unwrap();
        store.apply("e", "T-1", &Op::Approve, &cx(&r, &s)).unwrap();
        store
            .transition("e", "T-1", Status::Running, None, NOW)
            .unwrap();
    }

    #[test]
    fn who_runs_a_task_and_what_its_model_costs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (r, s) = (rules(), scope());
        let t = store.create("e", &new("t"), "human", &cx(&r, &s)).unwrap();
        assert_eq!(
            t.runner,
            Runner::External,
            "an outside MCP client, unless asked"
        );
        assert_eq!(t.constraints.tokens, DEFAULT_TOKENS);
        let n = New {
            runner: Some(Runner::Spall),
            constraints: Requested {
                tokens: Some(5000),
                ..Requested::default()
            },
            ..new("t2")
        };
        let t = store.create("e", &n, "human", &cx(&r, &s)).unwrap();
        assert_eq!((t.runner, t.constraints.tokens), (Runner::Spall, 5000));
        for bad in [0, MAX_TOKENS + 1] {
            let n = New {
                constraints: Requested {
                    tokens: Some(bad),
                    ..Requested::default()
                },
                ..new("t3")
            };
            assert!(matches!(
                store.create("e", &n, "human", &cx(&r, &s)),
                Err(Error::Invalid(_))
            ));
        }
        // A draft can change hands; an approved task can't.
        let (_, t) = store
            .apply(
                "e",
                "T-1",
                &Op::Edit {
                    goal: None,
                    role: None,
                    runner: Some(Runner::Spall),
                    constraints: None,
                    plan: None,
                    note: None,
                },
                &cx(&r, &s),
            )
            .unwrap();
        assert_eq!(t.runner, Runner::Spall);

        // Each model call is counted; past the token cap the task halts.
        store.apply("e", "T-2", &Op::Approve, &cx(&r, &s)).unwrap();
        store
            .transition("e", "T-2", Status::Running, None, NOW)
            .unwrap();
        let used = crate::model::Usage {
            input_tokens: 3000,
            output_tokens: 1000,
        };
        let t = store.charge_model("e", "T-2", used, 1500, NOW).unwrap();
        assert_eq!(
            t.usage,
            ModelUsage {
                calls: 1,
                input_tokens: 3000,
                output_tokens: 1000,
                model_ms: 1500
            }
        );
        assert_eq!(t.status, Status::Running);
        let t = store.charge_model("e", "T-2", used, 500, NOW).unwrap();
        assert_eq!(t.usage.calls, 2);
        assert_eq!(t.status, Status::Halted, "8000 tokens of 5000");
        assert!(t.reason.unwrap().contains("token budget"));
        // Still counted after it ends: the tokens were spent.
        let t = store.charge_model("e", "T-2", used, 1, NOW).unwrap();
        assert_eq!(t.usage.calls, 3);
        // Kept across a restart; an older file without them reads as zero.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get("e", "T-2").unwrap().usage.calls, 3);
    }

    fn sent(status: u16) -> Event {
        Event::Request {
            exchange: Some("r-1".into()),
            method: "GET".into(),
            target: "https://app.test/".into(),
            check: None,
            outcome: "sent".into(),
            status: Some(status),
        }
    }

    #[test]
    fn a_pause_keeps_the_request_until_the_wait_ends() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(&store, Requested::default());
        let p = pending(Ask::Unsafe {
            method: "POST".into(),
        });
        let t = store.pause("e", "T-1", p.clone()).unwrap();
        assert_eq!(
            (t.status, t.pending.as_ref()),
            (Status::WaitingOnApproval, Some(&p))
        );
        assert_eq!(t.reason.as_deref(), Some(p.reason.as_str()));
        let asked = serde_json::to_value(t.log.last().unwrap()).unwrap();
        assert_eq!(
            (asked["kind"].as_str(), asked["outcome"].as_str()),
            (Some("approval"), Some("asked"))
        );
        assert!(
            matches!(store.pause("e", "T-1", p.clone()), Err(Error::Conflict(_))),
            "already waiting"
        );
        // Kept across a restart, then dropped when the wait ends.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get("e", "T-1").unwrap().pending, Some(p));
        let (r, s) = (rules(), scope());
        let (_, halted) = store
            .apply(
                "e",
                "T-1",
                &Op::Halt {
                    reason: String::new(),
                },
                &cx(&r, &s),
            )
            .unwrap();
        assert!(
            halted.pending.is_none(),
            "no approval outlives the task's wait"
        );
    }

    #[test]
    fn a_stop_condition_halts_the_task_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(
            &store,
            Requested {
                stop: Some(Stop {
                    server_errors: Some(2),
                    throttled: Some(3),
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            store.log_send("e", "T-1", sent(500), NOW).unwrap().status,
            Status::Running
        );
        assert_eq!(
            store.log_send("e", "T-1", sent(429), NOW).unwrap().status,
            Status::Running
        );
        assert_eq!(
            store.log_send("e", "T-1", sent(200), NOW).unwrap().status,
            Status::Running
        );
        let t = store.log_send("e", "T-1", sent(503), NOW).unwrap();
        assert_eq!(t.status, Status::Halted);
        assert_eq!(
            t.reason.as_deref(),
            Some("stop condition: 2 server errors (5xx)")
        );
        assert!(
            Status::ALL.iter().all(|s| !Status::Halted.can_become(*s)),
            "never resumed"
        );
        assert!(matches!(
            store.log_send("e", "T-1", sent(200), NOW),
            Err(Error::Conflict(_))
        ));

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(
            &store,
            Requested {
                stop: Some(Stop {
                    server_errors: None,
                    throttled: Some(2),
                }),
                ..Default::default()
            },
        );
        store.log_send("e", "T-1", sent(429), NOW).unwrap();
        let t = store.log_send("e", "T-1", sent(429), NOW).unwrap();
        assert_eq!(
            t.reason.as_deref(),
            Some("stop condition: throttled 2 times (429)")
        );
    }

    #[test]
    fn the_e_stop_halts_every_task_for_good_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (r, sc) = (rules(), scope());
        {
            let store = Store::open(dir.path()).unwrap();
            running(&store, Requested::default());
            store
                .create("e", &new("draft"), "human", &cx(&r, &sc))
                .unwrap();
            store
                .create("other", &new("elsewhere"), "human", &cx(&r, &sc))
                .unwrap();
            store
                .apply("other", "T-1", &Op::Approve, &cx(&r, &sc))
                .unwrap();
            let done = store
                .create("e", &new("done"), "human", &cx(&r, &sc))
                .unwrap();
            store
                .apply("e", &done.id, &Op::Approve, &cx(&r, &sc))
                .unwrap();
            store
                .transition("e", &done.id, Status::Running, None, NOW)
                .unwrap();
            store
                .transition("e", &done.id, Status::Done, None, NOW)
                .unwrap();

            let st = store.engage("human", "something looks wrong", NOW);
            assert!(st.errors.is_empty(), "{:?}", st.errors);
            let mut halted = st.halted.clone();
            halted.sort();
            assert_eq!(
                halted,
                ["e/T-1", "e/T-2", "other/T-1"],
                "every unfinished task, every engagement"
            );
            assert_eq!(
                store.get("e", "T-3").unwrap().status,
                Status::Done,
                "finished stays finished"
            );
            let t = store.get("e", "T-1").unwrap();
            assert_eq!(
                (t.status, t.reason.as_deref()),
                (Status::Halted, Some("e-stop: something looks wrong"))
            );

            // Nothing runs, nothing is charged, nothing is approved.
            assert!(matches!(
                store.charge("e", "T-1", NOW),
                Err(Error::Conflict(_))
            ));
            store
                .create("e", &new("new"), "human", &cx(&r, &sc))
                .unwrap();
            assert!(
                matches!(store.apply("e", "T-4", &Op::Approve, &cx(&r, &sc)), Err(Error::Conflict(m)) if m.contains("e-stop"))
            );
            let again = store.engage("human", "second press", "2026-11-23T16:00:00Z");
            assert_eq!(
                (again.estop.since.as_str(), again.estop.reason.as_str()),
                (NOW, "something looks wrong"),
                "the first stop stands"
            );
        }
        // Back up stopped after a restart.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.estop().unwrap().reason, "something looks wrong");
        assert!(matches!(
            store.apply("e", "T-4", &Op::Approve, &cx(&r, &sc)),
            Err(Error::Conflict(_))
        ));

        // Clearing lets new work run; halted tasks stay halted.
        assert!(store.clear_estop().unwrap().is_some());
        assert!(store.estop().is_none());
        assert!(
            store.clear_estop().unwrap().is_none(),
            "clearing twice is fine"
        );
        assert_eq!(
            store.get("e", "T-4").unwrap().status,
            Status::Halted,
            "the second press halted the draft made after the first"
        );
        let fresh = store
            .create("e", &new("again"), "human", &cx(&r, &sc))
            .unwrap();
        store
            .apply("e", &fresh.id, &Op::Approve, &cx(&r, &sc))
            .unwrap();
        assert!(store
            .transition("e", "T-1", Status::Running, None, NOW)
            .is_err());
        assert!(
            Store::open(dir.path()).unwrap().estop().is_none(),
            "cleared on disk too"
        );
    }

    #[test]
    fn an_unreadable_e_stop_file_holds_the_stop() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(ESTOP_FILE), "{not json").unwrap();
        let store = Store::open(dir.path()).unwrap();
        let e = store.estop().expect("fails safe: engaged");
        assert!(e.reason.contains("couldn't be read"), "{}", e.reason);
        running_fails(&store);
    }

    fn running_fails(store: &Store) {
        let (r, s) = (rules(), scope());
        store.create("e", &new("t"), "human", &cx(&r, &s)).unwrap();
        assert!(store.apply("e", "T-1", &Op::Approve, &cx(&r, &s)).is_err());
    }

    fn pending(ask: Ask) -> Pending {
        Pending {
            method: "POST".into(),
            target: "https://app.test/x".into(),
            host_header: None,
            request: serde_json::json!({"id": "e1", "method": "POST"}),
            fingerprint: "f1".repeat(32),
            ask,
            reason: "T-1 sends POST only with a human's approval".into(),
            asked: NOW.into(),
        }
    }

    /// A store with T-1 running under `c`, paused on `ask`.
    fn paused(c: Requested, ask: Ask) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        running(&store, c);
        store.pause("e", "T-1", pending(ask)).unwrap();
        (dir, store)
    }

    const LATER: &str = "2026-11-23T15:05:00Z";
    const MUCH_LATER: &str = "2026-11-23T15:11:00Z";

    #[test]
    fn approve_once_grants_that_request_once_until_it_expires() {
        let (r, sc) = (rules(), scope());
        let (_d, store) = paused(
            Requested {
                budget: Some(1),
                ..Default::default()
            },
            Ask::Unsafe {
                method: "POST".into(),
            },
        );
        let (before, after) = store
            .decide("e", "T-1", &Decision::ApproveOnce, &cx(&r, &sc))
            .unwrap();
        assert_eq!(
            (before.status, after.status),
            (Status::WaitingOnApproval, Status::Running)
        );
        assert!(after.pending.is_none());
        let g = after.grant.clone().unwrap();
        assert_eq!(
            (g.fingerprint.as_str(), g.expires.as_str()),
            ("f1".repeat(32).as_str(), "2026-11-23T15:10:00Z")
        );
        assert_eq!(
            after.constraints, before.constraints,
            "once changes nothing else"
        );

        // Another request's fingerprint: no.
        assert!(matches!(
            store.charge_granted("e", "T-1", LATER, &"00".repeat(32)),
            Err(Error::Conflict(_))
        ));
        // This one: yes, once (past the budget of 1 is fine: it was approved).
        store.charge("e", "T-1", LATER).unwrap();
        store
            .charge_granted("e", "T-1", LATER, &"f1".repeat(32))
            .unwrap();
        assert_eq!(store.get("e", "T-1").unwrap().spent, 2);
        assert!(
            matches!(store.charge_granted("e", "T-1", LATER, &"f1".repeat(32)), Err(Error::Conflict(m)) if m.contains("no approval")),
            "used"
        );

        // Expired.
        let (_d, store) = paused(Requested::default(), Ask::Confirm);
        store
            .decide("e", "T-1", &Decision::ApproveOnce, &cx(&r, &sc))
            .unwrap();
        assert!(store
            .charge_granted("e", "T-1", MUCH_LATER, &"f1".repeat(32))
            .is_err());

        // Halted: the grant dies with the task.
        let (_d, store) = paused(Requested::default(), Ask::Confirm);
        store
            .decide("e", "T-1", &Decision::ApproveOnce, &cx(&r, &sc))
            .unwrap();
        store
            .apply(
                "e",
                "T-1",
                &Op::Halt {
                    reason: String::new(),
                },
                &cx(&r, &sc),
            )
            .unwrap();
        assert!(store.get("e", "T-1").unwrap().grant.is_none());
        assert!(store
            .charge_granted("e", "T-1", LATER, &"f1".repeat(32))
            .is_err());
    }

    #[test]
    fn approve_for_the_task_widens_it_within_the_roe() {
        let (r, sc) = (rules(), scope());
        let decide = |c: Requested, ask: Ask| {
            let (_d, store) = paused(c, ask);
            store
                .decide("e", "T-1", &Decision::ApproveForTask, &cx(&r, &sc))
                .map(|(_, a)| a.constraints)
        };
        let c = decide(
            Requested {
                hosts: Some(vec!["app.test".into()]),
                ..Default::default()
            },
            Ask::Host {
                host: "v1.api.test".into(),
            },
        )
        .unwrap();
        assert_eq!(c.hosts, ["app.test", "v1.api.test"]);
        let c = decide(
            Requested {
                methods: Some(vec!["GET".into()]),
                ..Default::default()
            },
            Ask::Method {
                method: "POST".into(),
            },
        )
        .unwrap();
        assert_eq!(c.methods, ["GET", "POST"]);
        assert_eq!(
            c.safe_methods,
            ["GET"],
            "the RoE wants POST confirmed: still asks"
        );
        let c = decide(
            Requested {
                budget: Some(30),
                time_limit_minutes: Some(20),
                ..Default::default()
            },
            Ask::Budget,
        )
        .unwrap();
        assert_eq!(c.budget, 60);
        let c = decide(
            Requested {
                time_limit_minutes: Some(20),
                ..Default::default()
            },
            Ask::Time,
        )
        .unwrap();
        assert_eq!(c.time_limit_minutes, 40);
        let c = decide(
            Requested {
                safe_methods: Some(vec![]),
                ..Default::default()
            },
            Ask::Unsafe {
                method: "GET".into(),
            },
        )
        .unwrap();
        assert_eq!(c.safe_methods, ["GET"]);

        // Never past the RoE: it wants POST confirmed, DELETE isn't allowed,
        // evil.test isn't in scope, and an RoE confirmation is only "once".
        for (ask, why) in [
            (
                Ask::Unsafe {
                    method: "POST".into(),
                },
                "want POST confirmed",
            ),
            (
                Ask::Method {
                    method: "DELETE".into(),
                },
                "DELETE isn't allowed",
            ),
            (
                Ask::Host {
                    host: "evil.test".into(),
                },
                "isn't in the engagement's scope",
            ),
            (Ask::Confirm, "approve it once"),
        ] {
            let c = Requested {
                methods: Some(vec!["GET".into()]),
                hosts: Some(vec!["app.test".into()]),
                ..Default::default()
            };
            let e = decide(c, ask).unwrap_err().to_string();
            assert!(e.contains(why), "{why}: {e}");
        }
        let (_d, store) = paused(Requested::default(), Ask::Confirm);
        assert!(store
            .decide("e", "T-1", &Decision::ApproveForTask, &cx(&r, &sc))
            .is_err());
        assert_eq!(
            store.get("e", "T-1").unwrap().status,
            Status::WaitingOnApproval,
            "a refused approval changes nothing"
        );
    }

    #[test]
    fn deny_resumes_the_task_with_the_reason() {
        let (r, sc) = (rules(), scope());
        let (_d, store) = paused(
            Requested::default(),
            Ask::Unsafe {
                method: "POST".into(),
            },
        );
        assert!(matches!(
            store.decide(
                "e",
                "T-1",
                &Decision::Deny { reason: " ".into() },
                &cx(&r, &sc)
            ),
            Err(Error::Invalid(_))
        ));
        let (_, t) = store
            .decide(
                "e",
                "T-1",
                &Decision::Deny {
                    reason: "not on prod".into(),
                },
                &cx(&r, &sc),
            )
            .unwrap();
        assert_eq!(t.status, Status::Running);
        assert_eq!(
            t.reason.as_deref(),
            Some("POST https://app.test/x was denied: not on prod")
        );
        assert!(t.pending.is_none() && t.grant.is_none());
        let logged = serde_json::to_value(&t.log[t.log.len() - 2]).unwrap();
        assert_eq!(logged["outcome"], "denied: not on prod");
        assert!(
            matches!(store.decide("e", "T-1", &Decision::ApproveOnce, &cx(&r, &sc)), Err(Error::Conflict(m)) if m.contains("isn't waiting"))
        );
    }

    #[test]
    fn no_approval_while_stopped_or_once_agents_are_not_allowed() {
        let (r, sc) = (rules(), scope());
        let (_d, store) = paused(Requested::default(), Ask::Confirm);
        let no_agents = Rules {
            agents_allowed: false,
            ..rules()
        };
        assert!(store
            .decide("e", "T-1", &Decision::ApproveOnce, &cx(&no_agents, &sc))
            .is_err());
        store.engage("human", "stop", NOW);
        assert!(
            store
                .decide("e", "T-1", &Decision::ApproveOnce, &cx(&r, &sc))
                .is_err(),
            "halted by the stop"
        );
    }
}
