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
use std::sync::Mutex;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_PER_ENGAGEMENT: usize = 1000;
/// Log entries per task. A task that fills its log can't go on: #55 halts it.
pub const MAX_LOG: usize = 20_000;
pub const MAX_GOAL: usize = 2000;
pub const MAX_NOTE: usize = 2000;
pub const MAX_STEPS: usize = 50;
pub const MAX_STEP: usize = 300;
pub const MAX_HOSTS: usize = 100;
/// Requests a task may be given, at most.
pub const MAX_BUDGET: u32 = 100_000;
pub const DEFAULT_BUDGET: u32 = 100;
/// Minutes a task may run, at most (a day) and by default.
pub const MAX_MINUTES: u32 = 24 * 60;
pub const DEFAULT_MINUTES: u32 = 60;

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
}

/// Asked-for constraints: anything left out comes from the RoE (or the
/// defaults, for what the RoE doesn't have).
#[derive(Debug, Clone, Default, Deserialize)]
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

    Ok(Constraints {
        hosts,
        methods,
        safe_methods,
        rate_limit,
        destructive,
        budget,
        time_limit_minutes,
        stop,
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
    },
    /// A registry command the task ran.
    Command { name: String, outcome: String },
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
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    pub goal: String,
    pub role: Role,
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
    /// Change a draft (nothing else: an approved task is what was approved).
    Edit {
        goal: Option<String>,
        role: Option<Role>,
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
    Save(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no task {id}"),
            Error::Invalid(m) | Error::Conflict(m) => f.write_str(m),
            Error::Full => write!(f, "this engagement has {MAX_PER_ENGAGEMENT} tasks already"),
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

/// Every engagement's tasks, loaded at start.
pub struct Store {
    dir: PathBuf,
    by_engagement: Mutex<HashMap<String, Book>>,
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
        Ok(Store {
            dir: dir.to_path_buf(),
            by_engagement: Mutex::new(by_engagement),
        })
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
        let out = f(&mut book)?;
        save(&self.dir, engagement, &book).map_err(|e| Error::Save(format!("{e:#}")))?;
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
                    authorize_rules(cx)?;
                    move_to(t, Status::Approved, None, cx.now)?;
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
                    if let Some(c) = constraints {
                        t.constraints = resolve(c, cx.rules, cx.scope).map_err(Error::Invalid)?;
                    }
                    if let Some(p) = plan {
                        t.plan = check_plan(p).map_err(Error::Invalid)?;
                    }
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

    /// Charge one request to task `id`: only while it runs and has budget
    /// left. Checked and counted under one lock, so racing sends can't
    /// overspend.
    pub fn charge(&self, engagement: &str, id: &str, now: &str) -> Result<u32, Error> {
        self.change(engagement, |book| {
            let t = find(book, id)?;
            if t.status != Status::Running {
                return Err(Error::Conflict(format!(
                    "{} is {}: only a running task sends",
                    t.id,
                    t.status.as_str()
                )));
            }
            if t.spent >= t.constraints.budget {
                return Err(Error::Conflict(format!(
                    "{} has spent its budget of {} requests",
                    t.id, t.constraints.budget
                )));
            }
            t.spent += 1;
            t.updated = now.into();
            Ok(t.constraints.budget - t.spent)
        })
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
        assert!(
            matches!(store.charge("eng-1", "T-1", NOW), Err(Error::Conflict(m)) if m.contains("budget of 10"))
        );
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
}
