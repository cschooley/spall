//! `POST /api/replay` (#17): resend a captured exchange through taphttp,
//! optionally with a different method, headers or body.
//!
//! The order is fixed: the same-origin guard (in `web::guard`, before any
//! of this), input validation, the original fetched from taphttp, the
//! action gate (scope, method policy, confirmation), then taphttp's
//! `POST /replay`. Nothing reaches taphttp unless the gate allows it, and
//! every attempt with a valid exchange id gets exactly one audit record.

use crate::audit::{AuditLog, Outcome, Record};
use crate::confirm::{fingerprint, Confirmations};
use crate::engagement::{Engagement, Error as EngagementError, New, Patch, Store};
use crate::gate::{self, Action, Decision, Scope};
use crate::lever::Lever;
use crate::roe::{self, Actor, Class, Limiter};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use taphttp_types::TrafficEntry;

/// Request bodies up to this size (the JSON, headers and body together).
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
/// A replay waits on the target, so it gets longer than a history query.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HEADERS: usize = 256;
const MAX_HEADER_NAME: usize = 256;
const MAX_HEADER_VALUE: usize = 16 * 1024;
const MAX_METHOD: usize = 32;

/// What the browser (or the console) sends. Unknown fields, `meta`
/// included, are ignored: spall sets the attribution itself.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReplayRequest {
    pub id: String,
    #[serde(default)]
    pub method: Option<String>,
    /// The whole ordered header list, replacing the captured one.
    #[serde(default)]
    pub req_headers: Option<Vec<(String, String)>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_encoding: Option<String>,
    /// A confirmation token from an earlier 428 for this exact request.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// HTTP `token` characters (RFC 9110), used for methods and header names.
fn is_token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Check the edits; `Err` is a message safe to show.
pub fn validate(req: &ReplayRequest) -> Result<(), String> {
    if !crate::web::valid_exchange_id(&req.id) {
        return Err("invalid exchange id".into());
    }
    if let Some(m) = &req.method {
        if !is_token(m, MAX_METHOD) {
            return Err("method must be an HTTP token (letters, digits, -)".into());
        }
    }
    if let Some(headers) = &req.req_headers {
        if headers.len() > MAX_HEADERS {
            return Err(format!("at most {MAX_HEADERS} headers"));
        }
        for (name, value) in headers {
            if !is_token(name, MAX_HEADER_NAME) {
                return Err(format!("invalid header name {name:?}"));
            }
            if value.len() > MAX_HEADER_VALUE
                || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
            {
                return Err(format!("invalid value for header {name:?}"));
            }
        }
    }
    match req.body_encoding.as_deref() {
        None | Some("utf8") | Some("base64") => {}
        Some(_) => return Err("body_encoding must be \"utf8\" or \"base64\"".into()),
    }
    Ok(())
}

/// The body for taphttp's `POST /replay`: only the fields the caller
/// changed, plus spall's attribution. serde_json's map is ordered, so the
/// bytes are canonical and double as the confirmation fingerprint input.
pub fn forward_body(req: &ReplayRequest) -> Vec<u8> {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), req.id.clone().into());
    if let Some(v) = &req.method {
        m.insert("method".into(), v.clone().into());
    }
    if let Some(v) = &req.req_headers {
        let pairs: Vec<serde_json::Value> = v
            .iter()
            .map(|(k, val)| serde_json::json!([k, val]))
            .collect();
        m.insert("req_headers".into(), pairs.into());
    }
    if let Some(v) = &req.body {
        m.insert("body".into(), v.clone().into());
    }
    if let Some(v) = &req.body_encoding {
        m.insert("body_encoding".into(), v.clone().into());
    }
    m.insert("meta".into(), serde_json::json!({ "actor": "human" }));
    serde_json::to_vec(&serde_json::Value::Object(m)).unwrap_or_default()
}

/// What the gate needs to know about the request that would be sent.
pub struct Effective {
    pub method: String,
    pub url: String,
    pub host_header: Option<String>,
}

pub fn effective(original: &TrafficEntry, req: &ReplayRequest) -> Effective {
    let host_header = match &req.req_headers {
        Some(h) => h
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("host"))
            .map(|(_, v)| v.clone()),
        None => original.req_headers.get("host").map(String::from),
    };
    Effective {
        method: req
            .method
            .clone()
            .unwrap_or_else(|| original.method.clone()),
        url: original.url.clone(),
        host_header,
    }
}

/// A scope change (#21 follow-up, exclusions #34). Entries are saved on
/// the active engagement (#35); `all` is for the running process only.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ScopeChange {
    Add {
        entry: String,
    },
    Exclude {
        entry: String,
    },
    /// `!entry` removes an exclusion; a plain entry leaves both lists.
    Remove {
        entry: String,
    },
    All {
        on: bool,
    },
    /// Several entries at once (a pasted list, #38; an OpenAPI file, #40):
    /// all or nothing, one audit record.
    Apply {
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
    },
}

/// Everything the handler needs.
pub struct Replayer {
    pub lever: Arc<Lever>,
    /// The engagements; the active one's scope is what the gate enforces.
    /// Changed by the tester only.
    pub engagements: Store,
    /// Every host allowed (`scope all on`, `SPALL_SCOPE=*`): this process only.
    pub all_hosts: AtomicBool,
    /// The rules' rate ceiling, shared by every send (#36).
    pub limiter: Mutex<Limiter>,
    pub confirmations: Confirmations,
    pub audit: AuditLog,
    /// Candidates flagged in each engagement (#42).
    pub candidates: crate::candidates::Store,
    /// Tasks in each engagement (#54).
    pub tasks: crate::tasks::Store,
    /// `--allow-unredacted-export`: the opt-in an unredacted export needs
    /// on top of asking for one (#51).
    pub allow_unredacted_export: bool,
}

fn json(status: StatusCode, body: serde_json::Value) -> Response {
    (status, axum::Json(body)).into_response()
}

fn error(status: StatusCode, msg: &str) -> Response {
    json(status, serde_json::json!({ "error": msg }))
}

/// taphttp's body as JSON for the browser: a JSON body as-is, anything
/// else (a plain-text error) wrapped as a string.
/// A task store error as an HTTP answer.
fn task_error(e: &crate::tasks::Error) -> Response {
    use crate::tasks::Error as T;
    let st = match e {
        T::NotFound(_) => StatusCode::NOT_FOUND,
        T::Invalid(_) => StatusCode::BAD_REQUEST,
        T::Conflict(_) | T::Full => StatusCode::CONFLICT,
        T::Save(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(st, &e.to_string())
}

/// At most 80 characters of free text, on one line, for an audit reason.
fn short(s: &str) -> String {
    let one: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if one.chars().count() > 80 {
        format!("{}…", one.chars().take(79).collect::<String>())
    } else {
        one
    }
}

fn upstream_json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::json!({ "error": String::from_utf8_lossy(body).trim() }))
}

impl Replayer {
    /// The scope as it is now (each replay decides on one snapshot).
    pub fn scope_now(&self) -> Scope {
        self.engagements
            .scope()
            .with_all(self.all_hosts.load(Ordering::SeqCst))
    }

    /// Apply a change, save it on the engagement and audit it. Returns the
    /// new scope and whether anything changed; on error nothing changed.
    pub fn change_scope(&self, change: &ScopeChange) -> Result<(Scope, bool), EngagementError> {
        let before = self.scope_now().entries();
        let (changed, what) = match change {
            ScopeChange::Add { entry } => {
                let (_, _, c) = self.engagements.change_scope(|s| s.add(entry))?;
                (c, format!("add {}", entry.trim()))
            }
            ScopeChange::Exclude { entry } => {
                let (_, _, c) = self.engagements.change_scope(|s| s.exclude(entry))?;
                (
                    c,
                    format!("exclude {}", entry.trim().trim_start_matches('!').trim()),
                )
            }
            ScopeChange::Remove { entry } => {
                let (_, _, c) = self.engagements.change_scope(|s| Ok(s.remove(entry)))?;
                (c, format!("remove {}", entry.trim()))
            }
            ScopeChange::Apply { add, exclude } => {
                if add.len() + exclude.len() > crate::scope_list::MAX_LINES {
                    return Err(EngagementError::Invalid(format!(
                        "at most {} entries at once",
                        crate::scope_list::MAX_LINES
                    )));
                }
                let (before, after, _) = self.engagements.change_scope(|s| {
                    for e in add {
                        s.add(e)?;
                    }
                    for e in exclude {
                        s.exclude(e)?;
                    }
                    Ok(())
                })?;
                (
                    before != after,
                    format!("apply {} in, {} excluded", add.len(), exclude.len()),
                )
            }
            ScopeChange::All { on } => {
                let was = self.all_hosts.swap(*on, Ordering::SeqCst);
                (
                    was != *on,
                    format!("all hosts {}", if *on { "on" } else { "off" }),
                )
            }
        };
        let now = self.scope_now();
        let mut rec = Record::new("scope", "", Outcome::ScopeChanged);
        rec.reason = Some(what);
        rec.scope_all = now.allows_all();
        rec.scope_before = Some(before);
        rec.scope_after = Some(now.entries());
        rec.error = (!changed).then(|| "no change".to_string());
        self.audit(&rec);
        if changed && now.allows_all() {
            tracing::warn!("scope: every host is in scope until restart or `scope all off`");
        }
        Ok((now, changed))
    }

    /// Whether the rules allow testing right now (dates, windows), and why not.
    pub fn testing_now(&self) -> serde_json::Value {
        let e = self.engagements.active();
        match e.rules.check_time(&e.period(), jiff::Timestamp::now()) {
            Ok(()) => serde_json::json!({ "allowed": true, "reason": null }),
            Err(r) => serde_json::json!({ "allowed": false, "reason": r }),
        }
    }

    /// Change the active engagement's details; audited with before/after.
    pub fn update_engagement(&self, patch: Patch) -> Result<(Engagement, bool), EngagementError> {
        let (e, changes) = self.engagements.update(patch)?;
        let changed = !changes.is_empty();
        let mut rec = Record::new("engagement", "", Outcome::EngagementChanged);
        rec.reason = Some(format!(
            "set {}",
            if changed {
                changes.keys().cloned().collect::<Vec<_>>().join(", ")
            } else {
                "nothing".into()
            }
        ));
        rec.changes = changed.then(|| serde_json::to_value(&changes).unwrap_or_default());
        rec.scope_all = self.scope_now().allows_all();
        rec.error = (!changed).then(|| "no change".to_string());
        self.audit(&rec);
        Ok((e, changed))
    }

    /// Create an engagement and make it active; audited. Every-host scope
    /// is switched off: it was a decision about the previous engagement.
    pub fn create_engagement(&self, new: New) -> Result<Engagement, EngagementError> {
        let before = self.engagements.active().id;
        let e = self.engagements.create(new)?;
        self.switched("new", &before, &e);
        Ok(e)
    }

    /// Switch the active engagement (by id or name); audited.
    pub fn activate_engagement(&self, which: &str) -> Result<Engagement, EngagementError> {
        let (before, e) = self.engagements.activate(which)?;
        self.switched("use", &before.id, &e);
        Ok(e)
    }

    fn switched(&self, how: &str, before: &str, e: &Engagement) {
        let was_all = self.all_hosts.swap(false, Ordering::SeqCst);
        let mut rec = Record::new("engagement", "", Outcome::EngagementChanged);
        rec.engagement = Some(before.to_string());
        rec.reason = Some(format!("{how} {}", e.id));
        rec.changes = Some(serde_json::json!({
            "active": { "before": before, "after": e.id },
            "scope_all": { "before": was_all, "after": false },
        }));
        rec.scope_after = Some(e.scope.clone());
        self.audit(&rec);
    }

    /// One exchange from taphttp's history, or the response to give.
    async fn exchange(&self, id: &str) -> Result<TrafficEntry, Response> {
        if !crate::web::valid_exchange_id(id) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                &format!("not an exchange id: {id}"),
            ));
        }
        match self.lever.get_bytes(&format!("/exchanges/{id}")).await {
            Ok((StatusCode::OK, body)) => {
                serde_json::from_slice::<TrafficEntry>(&body).map_err(|_| {
                    error(
                        StatusCode::BAD_GATEWAY,
                        "taphttp returned an unreadable exchange",
                    )
                })
            }
            Ok((StatusCode::NOT_FOUND, _)) => {
                Err(error(StatusCode::NOT_FOUND, &format!("no exchange {id}")))
            }
            Ok((st, _)) => Err(error(
                StatusCode::BAD_GATEWAY,
                &format!("taphttp answered {st}"),
            )),
            Err(e) => Err(error(StatusCode::BAD_GATEWAY, &format!("taphttp: {e:#}"))),
        }
    }

    /// The active engagement's candidates, newest first; only those backed
    /// by `exchange` when given.
    pub fn candidates(&self, exchange: Option<&str>) -> Vec<crate::candidates::Candidate> {
        let mut list = self.candidates.list(&self.engagements.active().id);
        if let Some(x) = exchange {
            list.retain(|c| c.exchanges().iter().any(|e| e == x));
        }
        list
    }

    /// Flag an exchange as a candidate (#42, #43). 201 with the candidate;
    /// 409 with the existing one when a candidate shares its fingerprint
    /// (add evidence to it, or reopen it, instead). Audited.
    pub async fn flag(&self, flag: crate::candidates::Flag) -> Response {
        use crate::candidates::{finding_for, new_id, Candidate};
        if let Err(m) = flag.validate() {
            return error(StatusCode::BAD_REQUEST, &m);
        }
        let Some(rule) = crate::rules::find(&flag.rule) else {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("no rule {} (see `rules`)", flag.rule),
            );
        };
        let entry = match self.exchange(&flag.exchange).await {
            Ok(e) => e,
            Err(r) => return r,
        };
        for x in &flag.replays {
            if let Err(r) = self.exchange(x).await {
                return r;
            }
        }
        let now = taphttp_types::Timestamp::now().to_string();
        let finding = finding_for(rule, &flag, &entry, &now);
        let eng = self.engagements.active().id;
        if let Some(existing) = self.candidates.matching(&eng, &finding) {
            return json(
                StatusCode::CONFLICT,
                serde_json::json!({
                    "error": format!("already flagged as {} (same endpoint, parameter and rule)", existing.id),
                    "existing": existing,
                }),
            );
        }
        let c = Candidate {
            id: new_id(),
            created: now.clone(),
            updated: now,
            flagged_by: "human".into(),
            promoted: None,
            finding,
        };
        let mut rec = Record::new("candidate", &entry.id, Outcome::CandidateChanged);
        rec.method = Some(entry.method.clone());
        rec.target = Some(entry.url.clone());
        rec.reason = Some(format!("flag {} as {}", c.id, rule.id));
        rec.result_id = Some(c.id.clone());
        match self.candidates.add(&eng, c) {
            Ok(c) => {
                self.audit(&rec);
                (StatusCode::CREATED, axum::Json(c)).into_response()
            }
            Err(e) => {
                rec.error = Some(e.to_string());
                self.audit(&rec);
                error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            }
        }
    }

    /// Candidate `id` as it would be exported, and what redaction changes
    /// (#49). Nothing is stored or sent.
    pub fn redaction(&self, id: &str) -> Response {
        let eng = self.engagements.active().id;
        let Some(c) = self.candidates.get(&eng, id) else {
            return error(StatusCode::NOT_FOUND, &format!("no candidate {id}"));
        };
        let (finding, changes) = flux::redact::preview(&c.finding);
        json(
            StatusCode::OK,
            serde_json::json!({ "id": c.id, "finding": finding, "changes": changes }),
        )
    }

    /// Export the promoted candidates as a flux report (#51): redacted,
    /// or unredacted only with `--allow-unredacted-export` and an explicit
    /// ask. Audited either way.
    pub fn export(&self, unredacted: bool) -> Response {
        let eng = self.engagements.active().id;
        let mut rec = Record::new("export", "", Outcome::Exported);
        if unredacted && !self.allow_unredacted_export {
            rec.decision = Some("deny");
            rec.error = Some("unredacted export without --allow-unredacted-export".into());
            self.audit(&rec);
            return error(
                StatusCode::FORBIDDEN,
                "an unredacted export needs spall started with --allow-unredacted-export",
            );
        }
        let list: Vec<_> = self.candidates.list(&eng).into_iter().rev().collect();
        let ex = match crate::candidates::export(&list, !unredacted, env!("CARGO_PKG_VERSION")) {
            Ok(ex) => ex,
            Err(problems) => {
                rec.error = Some(format!("not valid flux: {}", problems.join("; ")));
                self.audit(&rec);
                return json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({ "error": "the export isn't valid flux", "problems": problems }),
                );
            }
        };
        if ex.findings == 0 {
            rec.error = Some("nothing promoted".into());
            self.audit(&rec);
            return error(
                StatusCode::CONFLICT,
                "nothing promoted to export (promote confirmed candidates first)",
            );
        }
        rec.reason = Some(format!(
            "exported {} finding{} ({})",
            ex.findings,
            if ex.findings == 1 { "" } else { "s" },
            if unredacted {
                "UNREDACTED".to_string()
            } else {
                format!("redacted, {} secrets replaced", ex.redactions)
            }
        ));
        let body = match serde_json::to_vec_pretty(&ex.report) {
            Ok(b) => b,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
        self.audit(&rec);
        let date: String = taphttp_types::Timestamp::now()
            .to_string()
            .chars()
            .take(10)
            .filter(|c| *c != '-')
            .collect();
        let name = format!(
            "spall-{eng}-{date}{}.sarif",
            if unredacted { "-UNREDACTED" } else { "" }
        );
        (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/sarif+json".to_string(),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
                (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
            ],
            body,
        )
            .into_response()
    }

    /// Triage or edit candidate `id` (#42, #44). Audited with before/after.
    pub async fn triage(&self, id: &str, op: crate::candidates::Op) -> Response {
        use crate::candidates::{new_guid, Error as CErr, Op};
        if let Op::Evidence { exchanges } = &op {
            for x in exchanges {
                if let Err(r) = self.exchange(x).await {
                    return r;
                }
            }
        }
        let eng = self.engagements.active().id;
        let now = taphttp_types::Timestamp::now().to_string();
        let mut guid = new_guid;
        let first = self
            .candidates
            .get(&eng, id)
            .and_then(|c| c.exchanges().first().cloned())
            .unwrap_or_default();
        let mut rec = Record::new("candidate", &first, Outcome::CandidateChanged);
        rec.result_id = Some(id.to_string());
        let why = match &op {
            Op::Reject { justification }
            | Op::Accept { justification }
            | Op::Duplicate { justification, .. } => {
                format!(": {justification}")
            }
            _ => String::new(),
        };
        rec.reason = Some(format!("{} {id}{why}", op.name()));
        match self.candidates.apply(&eng, id, &op, &now, &mut guid) {
            Ok((before, after)) => {
                rec.changes = Some(serde_json::json!({
                    "status": { "before": before.finding.status(), "after": after.finding.status() },
                    "suppressed": { "before": before.finding.is_suppressed(), "after": after.finding.is_suppressed() },
                }));
                self.audit(&rec);
                axum::Json(after).into_response()
            }
            Err(e) => {
                rec.error = Some(e.to_string());
                self.audit(&rec);
                let st = match e {
                    CErr::NotFound(_) => StatusCode::NOT_FOUND,
                    CErr::Invalid(_) => StatusCode::BAD_REQUEST,
                    CErr::Full => StatusCode::CONFLICT,
                    CErr::Save(_) => StatusCode::INTERNAL_SERVER_ERROR,
                };
                error(st, &e.to_string())
            }
        }
    }

    /// The active engagement's tasks, newest first, without their logs.
    pub fn tasks(&self) -> Vec<serde_json::Value> {
        let eng = self.engagements.active().id;
        self.tasks.list(&eng).iter().map(|t| t.summary()).collect()
    }

    /// One of the active engagement's tasks, log included.
    pub fn task(&self, id: &str) -> Option<crate::tasks::Task> {
        self.tasks.get(&self.engagements.active().id, id)
    }

    /// Make a draft task (#54): only in an engagement whose rules allow
    /// agents, with a scope, inside its dates. Audited, refusals too.
    pub fn create_task(&self, new: crate::tasks::New) -> Response {
        use crate::tasks::{authorized, Context};
        let e = self.engagements.active();
        let scope = self.engagements.scope();
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("task", "", Outcome::TaskChanged);
        rec.reason = Some(format!("create: {}", short(&new.goal)));
        if let Err(why) = authorized(&e.rules, &e.period(), &scope, jiff::Timestamp::now()) {
            rec.error = Some(why.clone());
            self.audit(&rec);
            return error(StatusCode::CONFLICT, &format!("no task: {why}"));
        }
        let cx = Context {
            rules: &e.rules,
            scope: &scope,
            now: &now,
        };
        match self.tasks.create(&e.id, &new, "human", &cx) {
            Ok(t) => {
                rec.reason = Some(format!("create {}", t.id));
                rec.result_id = Some(t.id.clone());
                self.audit(&rec);
                json(
                    StatusCode::CREATED,
                    serde_json::to_value(&t).unwrap_or_default(),
                )
            }
            Err(err) => {
                rec.error = Some(err.to_string());
                self.audit(&rec);
                task_error(&err)
            }
        }
    }

    /// A human's change to task `id`: approve, edit a draft, halt (#54).
    /// Audited with the status before and after.
    pub fn task_op(&self, id: &str, op: crate::tasks::Op) -> Response {
        use crate::tasks::{Context, Op};
        let e = self.engagements.active();
        let scope = self.engagements.scope();
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("task", "", Outcome::TaskChanged);
        rec.result_id = Some(id.to_string());
        rec.reason = Some(match &op {
            Op::Halt { reason } if !reason.trim().is_empty() => {
                format!("halt {id}: {}", short(reason))
            }
            _ => format!("{} {id}", op.name()),
        });
        let cx = Context {
            rules: &e.rules,
            scope: &scope,
            now: &now,
        };
        match self.tasks.apply(&e.id, id, &op, &cx) {
            Ok((before, after)) => {
                rec.changes = Some(serde_json::json!({
                    "status": { "before": before.status, "after": after.status },
                }));
                self.audit(&rec);
                axum::Json(after).into_response()
            }
            Err(err) => {
                rec.error = Some(err.to_string());
                self.audit(&rec);
                task_error(&err)
            }
        }
    }

    fn audit(&self, rec: &Record) {
        let mut rec = rec.clone();
        if rec.engagement.is_none() {
            rec.engagement = Some(self.engagements.active().id);
        }
        if let Err(e) = self.audit.record(&rec) {
            tracing::error!(
                "audit log {} write failed: {e}",
                self.audit.path().display()
            );
        }
    }

    pub async fn handle(&self, raw: &[u8]) -> Response {
        let req: ReplayRequest = match serde_json::from_slice(raw) {
            Ok(r) => r,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "body must be a JSON replay request",
                )
            }
        };
        if let Err(msg) = validate(&req) {
            if crate::web::valid_exchange_id(&req.id) {
                let mut rec = Record::new("replay", &req.id, Outcome::Error);
                rec.error = Some(msg.clone());
                self.audit(&rec);
            }
            return error(StatusCode::BAD_REQUEST, &msg);
        }

        let mut rec = Record::new("replay", &req.id, Outcome::Error);

        // The original, to know where this would go.
        let original = match self
            .lever
            .get_bytes(&format!("/exchanges/{}", req.id))
            .await
        {
            Ok((StatusCode::OK, body)) => match serde_json::from_slice::<TrafficEntry>(&body) {
                Ok(e) => e,
                Err(_) => {
                    rec.error = Some("taphttp returned an unreadable exchange".into());
                    self.audit(&rec);
                    return error(
                        StatusCode::BAD_GATEWAY,
                        "taphttp returned an unreadable exchange",
                    );
                }
            },
            Ok((StatusCode::NOT_FOUND, _)) => {
                rec.error = Some("no such exchange".into());
                self.audit(&rec);
                return error(StatusCode::NOT_FOUND, "no such exchange");
            }
            Ok((status, _)) => {
                rec.upstream_status = Some(status.as_u16());
                rec.error = Some("taphttp refused the lookup".into());
                self.audit(&rec);
                return error(StatusCode::BAD_GATEWAY, "taphttp API unavailable");
            }
            Err(e) => {
                tracing::warn!("replay: looking up {}: {e:#}", req.id);
                rec.error = Some("taphttp API unavailable".into());
                self.audit(&rec);
                return error(StatusCode::BAD_GATEWAY, "taphttp API unavailable");
            }
        };

        let eff = effective(&original, &req);
        rec.method = Some(eff.method.clone());
        rec.target = Some(eff.url.clone());
        rec.host_header = eff.host_header.clone();
        let scope = self.scope_now();
        rec.scope_all = scope.allows_all();
        let engagement = self.engagements.active();
        let layers = [&engagement.rules];
        let decision = gate::decide(
            &scope,
            &layers,
            &engagement.period(),
            &Action {
                method: &eff.method,
                url: &eff.url,
                host_header: eff.host_header.as_deref(),
                actor: Actor::Human,
                class: Class::Send,
            },
            jiff::Timestamp::now(),
        );
        rec.decision = Some(decision.kind());
        rec.reason = decision.reason().map(String::from);

        let body = forward_body(&req);
        match &decision {
            Decision::Deny(reason) => {
                rec.outcome = Outcome::Denied;
                self.audit(&rec);
                return json(
                    StatusCode::FORBIDDEN,
                    serde_json::json!({ "error": reason, "decision": "deny" }),
                );
            }
            Decision::NeedsConfirmation(reason) => {
                let fp = fingerprint(&body);
                let now = Instant::now();
                match &req.confirm {
                    Some(token) => {
                        if let Err(refused) = self.confirmations.redeem(token, fp, now) {
                            rec.outcome = Outcome::ConfirmationRefused;
                            rec.error = Some(refused.message().into());
                            self.audit(&rec);
                            return json(
                                StatusCode::CONFLICT,
                                serde_json::json!({
                                    "error": refused.message(),
                                    "decision": "needs_confirmation",
                                }),
                            );
                        }
                        rec.confirmed = true;
                    }
                    None => {
                        let token = match self.confirmations.issue(fp, now) {
                            Ok(t) => t,
                            Err(e) => {
                                tracing::error!("no randomness for a confirmation token: {e}");
                                rec.error = Some("could not issue a confirmation".into());
                                self.audit(&rec);
                                return error(
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    "could not issue a confirmation",
                                );
                            }
                        };
                        rec.outcome = Outcome::NeedsConfirmation;
                        self.audit(&rec);
                        return json(
                            StatusCode::PRECONDITION_REQUIRED,
                            serde_json::json!({
                                "error": reason,
                                "decision": "needs_confirmation",
                                "confirm": token,
                                "expires_in_secs": self.confirmations.ttl().as_secs(),
                            }),
                        );
                    }
                }
            }
            Decision::Allow => {}
        }

        // The rate ceiling counts sends only: denials and unconfirmed
        // attempts above never reach it.
        let rate = roe::rate_of(&layers);
        let wait = self
            .limiter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take(rate, Instant::now());
        if let Err(wait) = wait {
            let secs = wait.as_secs_f64();
            let msg = format!(
                "the rules of engagement allow {} sends per second; retry in {secs:.1}s",
                rate.unwrap_or_default()
            );
            rec.outcome = Outcome::RateLimited;
            rec.error = Some(msg.clone());
            self.audit(&rec);
            let mut res = json(
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({ "error": msg, "decision": "rate_limited", "retry_after_secs": secs }),
            );
            if let Ok(v) =
                axum::http::HeaderValue::from_str(&format!("{}", secs.ceil().max(1.0) as u64))
            {
                res.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
            }
            return res;
        }

        match self.lever.post_json("/replay", body, REPLAY_TIMEOUT).await {
            Ok((status, bytes)) => {
                rec.upstream_status = Some(status.as_u16());
                let value = upstream_json(&bytes);
                let result_id = value.get("id").and_then(|v| v.as_str()).map(String::from);
                match status {
                    StatusCode::OK => {
                        rec.outcome = Outcome::Sent;
                        rec.result_id = result_id;
                        self.audit(&rec);
                        json(StatusCode::OK, value)
                    }
                    // A recorded failed attempt: the entry, with `error` set.
                    StatusCode::BAD_GATEWAY if result_id.is_some() => {
                        rec.outcome = Outcome::Failed;
                        rec.result_id = result_id;
                        rec.error = value
                            .get("error")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        self.audit(&rec);
                        json(StatusCode::BAD_GATEWAY, value)
                    }
                    StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND => {
                        let msg = value
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("taphttp refused the replay")
                            .to_string();
                        rec.error = Some(msg.clone());
                        self.audit(&rec);
                        error(status, &msg)
                    }
                    _ => {
                        rec.error = Some(format!("taphttp answered {status}"));
                        self.audit(&rec);
                        error(StatusCode::BAD_GATEWAY, "taphttp API unavailable")
                    }
                }
            }
            Err(e) => {
                tracing::warn!("replay of {}: {e:#}", req.id);
                rec.error = Some("taphttp API unavailable".into());
                self.audit(&rec);
                error(StatusCode::BAD_GATEWAY, "taphttp API unavailable")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str) -> ReplayRequest {
        ReplayRequest {
            id: id.into(),
            method: None,
            req_headers: None,
            body: None,
            body_encoding: None,
            confirm: None,
        }
    }

    #[test]
    fn unknown_fields_and_browser_meta_are_ignored() {
        let r: ReplayRequest =
            serde_json::from_str(r#"{"id":"a1","meta":{"actor":"agent"},"evil":1}"#).unwrap();
        assert_eq!(r, req("a1"));
        let fwd: serde_json::Value = serde_json::from_slice(&forward_body(&r)).unwrap();
        assert_eq!(
            fwd,
            serde_json::json!({"id": "a1", "meta": {"actor": "human"}})
        );
    }

    #[test]
    fn only_changed_fields_are_forwarded_in_canonical_order() {
        let mut r = req("a1");
        r.method = Some("POST".into());
        r.req_headers = Some(vec![("x-b".into(), "1".into()), ("x-a".into(), "2".into())]);
        r.body = Some("aGk=".into());
        r.body_encoding = Some("base64".into());
        r.confirm = Some("tok".into());
        let text = String::from_utf8(forward_body(&r)).unwrap();
        assert_eq!(
            text,
            r#"{"body":"aGk=","body_encoding":"base64","id":"a1","meta":{"actor":"human"},"method":"POST","req_headers":[["x-b","1"],["x-a","2"]]}"#,
            "keys sorted, header order kept, the token not forwarded"
        );
    }

    #[test]
    fn edits_are_validated() {
        assert_eq!(validate(&req("3f2a-b")), Ok(()));
        let bad = |f: &dyn Fn(&mut ReplayRequest)| {
            let mut r = req("a1");
            f(&mut r);
            validate(&r).is_err()
        };
        assert!(bad(&|r| r.id = "../x".into()));
        assert!(bad(&|r| r.method = Some("GET /x".into())));
        assert!(bad(&|r| r.method = Some(String::new())));
        assert!(bad(&|r| r.method = Some("X".repeat(33))));
        assert!(bad(
            &|r| r.req_headers = Some(vec![("bad name".into(), "v".into())])
        ));
        assert!(bad(
            &|r| r.req_headers = Some(vec![("x".into(), "a\r\nInjected: 1".into())])
        ));
        assert!(bad(
            &|r| r.req_headers = Some(vec![("x".into(), "a\0".into())])
        ));
        assert!(bad(
            &|r| r.req_headers = Some(vec![("x".into(), "v".into()); 257])
        ));
        assert!(bad(&|r| r.body_encoding = Some("hex".into())));
        assert!(!bad(&|r| r.method = Some("PROPFIND".into())));
    }

    fn original() -> TrafficEntry {
        serde_json::from_str(r#"{"id":"o1","ts":"2026-11-21T00:00:00Z","host":"app.test","method":"GET","url":"https://app.test/x","req_headers":[["host","app.test"],["cookie","s=1"]],"status":200,"duration_ms":1}"#).unwrap()
    }

    #[test]
    fn effective_request_uses_overrides_or_the_original() {
        let e = effective(&original(), &req("o1"));
        assert_eq!(
            (e.method.as_str(), e.host_header.as_deref()),
            ("GET", Some("app.test"))
        );
        assert_eq!(e.url, "https://app.test/x");

        let mut r = req("o1");
        r.method = Some("DELETE".into());
        r.req_headers = Some(vec![("Host".into(), "admin.internal".into())]);
        let e = effective(&original(), &r);
        assert_eq!(
            (e.method.as_str(), e.host_header.as_deref()),
            ("DELETE", Some("admin.internal"))
        );

        // Headers replaced without a Host: hyper derives it from the URL.
        r.req_headers = Some(vec![]);
        assert_eq!(effective(&original(), &r).host_header, None);
    }
}
