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
    validate_headers(req.req_headers.as_deref())?;
    match req.body_encoding.as_deref() {
        None | Some("utf8") | Some("base64") => {}
        Some(_) => return Err("body_encoding must be \"utf8\" or \"base64\"".into()),
    }
    Ok(())
}

fn validate_headers(headers: Option<&[(String, String)]>) -> Result<(), String> {
    if let Some(headers) = headers {
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
    Ok(())
}

/// URLs up to this long.
const MAX_URL: usize = 8 * 1024;

/// A new request, not a replay (#74): GET or HEAD to an absolute URL,
/// sent with taphttp's `POST /send`. Unknown fields, `meta` included, are
/// ignored: spall sets the attribution itself.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    /// `GET` (the default) or `HEAD`; other methods are `replay`'s, which
    /// starts from a captured request.
    #[serde(default)]
    pub method: Option<String>,
    /// The whole ordered header list; without it taphttp sends its own
    /// `user-agent` (and `host` from the URL).
    #[serde(default)]
    pub req_headers: Option<Vec<(String, String)>>,
    /// A confirmation token from an earlier 428 for this exact request.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// Check a fetch; `Ok` is what the gate decides on, `Err` a message safe
/// to show. The URL must be absolute `http`/`https` with a host and no
/// user info, so the host the gate checks is the one taphttp connects to.
pub fn validate_fetch(req: &FetchRequest) -> Result<Effective, String> {
    let method = req.method.clone().unwrap_or_else(|| "GET".into());
    if method != "GET" && method != "HEAD" {
        return Err(format!(
            "fetch sends GET or HEAD, not {method:?}: other methods are replay's (edit a captured request)"
        ));
    }
    let url = req.url.as_str();
    if url.len() > MAX_URL || url.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err(format!(
            "url must be at most {MAX_URL} bytes, with no spaces or control characters"
        ));
    }
    let uri: hyper::Uri = url
        .parse()
        .map_err(|_| "url must be an absolute http or https URL".to_string())?;
    let scheme_ok = matches!(uri.scheme_str(), Some("http") | Some("https"));
    let authority = uri.authority().map(|a| a.as_str()).unwrap_or_default();
    if !scheme_ok || uri.host().is_none_or(str::is_empty) {
        return Err("url must be an absolute http or https URL".into());
    }
    if authority.contains('@') {
        return Err(
            "url must not carry user info (user:pass@); send an authorization header".into(),
        );
    }
    validate_headers(req.req_headers.as_deref())?;
    Ok(Effective {
        method,
        url: url.to_string(),
        host_header: req.req_headers.as_ref().and_then(|h| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("host"))
                .map(|(_, v)| v.clone())
        }),
    })
}

/// The body for taphttp's `POST /send`, canonical like [`forward_body`]:
/// it is also the approval fingerprint's input.
pub fn fetch_body(req: &FetchRequest, eff: &Effective, who: Who) -> Vec<u8> {
    let mut m = serde_json::Map::new();
    m.insert("method".into(), eff.method.clone().into());
    m.insert("url".into(), eff.url.clone().into());
    if let Some(v) = &req.req_headers {
        let pairs: Vec<serde_json::Value> = v
            .iter()
            .map(|(k, val)| serde_json::json!([k, val]))
            .collect();
        m.insert("req_headers".into(), pairs.into());
    }
    m.insert("meta".into(), who.meta());
    serde_json::to_vec(&serde_json::Value::Object(m)).unwrap_or_default()
}

/// Who is sending: the tester (the browser or console), or an agent under
/// a task (#55). Only spall's own code picks this; no request can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who<'a> {
    Human,
    Agent { task: &'a str },
}

impl Who<'_> {
    /// The attribution taphttp stores with the replay (`meta`).
    pub fn meta(&self) -> serde_json::Value {
        match self {
            Who::Human => serde_json::json!({ "actor": "human" }),
            Who::Agent { task } => serde_json::json!({ "actor": "agent", "task": task }),
        }
    }
}

/// The body for taphttp's `POST /replay`: only the fields the caller
/// changed, plus spall's attribution. serde_json's map is ordered, so the
/// bytes are canonical and double as the confirmation fingerprint input.
pub fn forward_body(req: &ReplayRequest) -> Vec<u8> {
    forward_body_as(req, Who::Human)
}

/// [`forward_body`], attributed to `who`.
pub fn forward_body_as(req: &ReplayRequest, who: Who) -> Vec<u8> {
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
    m.insert("meta".into(), who.meta());
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
        /// The override for a too-wide entry (#83); audited.
        #[serde(default)]
        wide: bool,
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
        /// The override for too-wide entries among `add` (#83); audited.
        #[serde(default)]
        wide: bool,
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
    /// Each task's own rate ceiling (#55), on top of the engagement's.
    pub task_limiters: Mutex<std::collections::HashMap<String, Limiter>>,
    /// Candidates flagged in each engagement (#42).
    pub candidates: crate::candidates::Store,
    /// Tasks in each engagement (#54).
    pub tasks: crate::tasks::Store,
    /// `--allow-unredacted-export`: the opt-in an unredacted export needs
    /// on top of asking for one (#51).
    pub allow_unredacted_export: bool,
    /// spall's and taphttp's own addresses: never an agent's target (#73).
    pub stack: crate::stack::Stack,
    /// spall's own agent host is running (#63): a verb's task (#67) is
    /// drafted for it to run, not for an outside client.
    pub spall_runs: AtomicBool,
    /// Quad mode (#66): the burst underway, if any, and the last scorecard.
    pub quad: crate::quad::Boost,
    /// The intake's plans (#65).
    pub plans: crate::plans::Store,
    /// Where finding hosts first resolved (#65), and the resolver.
    pub addresses: Arc<crate::addresses::Store>,
    pub resolver: std::sync::RwLock<Arc<dyn crate::addresses::Resolve>>,
}

/// Drafts one agent's task may make with verbs (#67) before it's told to
/// leave the rest to the tester.
pub const MAX_AGENT_DRAFTS: usize = 5;

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
        T::Conflict(_) | T::Full | T::Spent { .. } => StatusCode::CONFLICT,
        T::Save(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(st, &e.to_string())
}

/// Lowercase hex, for fingerprints.
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Whether a failed charge was the budget running out (rather than the
/// task leaving `running`).
fn task_of_budget(e: &crate::tasks::Error) -> bool {
    matches!(e, crate::tasks::Error::Spent { .. })
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
            ScopeChange::Add { entry, wide } => {
                let over = *wide && crate::gate::too_wide(entry).ok().flatten().is_some();
                let (_, _, c) = self.engagements.change_scope(|s| {
                    if *wide {
                        s.add_wide(entry)
                    } else {
                        s.add(entry)
                    }
                })?;
                (
                    c,
                    format!(
                        "add {}{}",
                        entry.trim(),
                        if over {
                            " (too wide: added with --wide)"
                        } else {
                            ""
                        }
                    ),
                )
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
            ScopeChange::Apply { add, exclude, wide } => {
                if add.len() + exclude.len() > crate::scope_list::MAX_LINES {
                    return Err(EngagementError::Invalid(format!(
                        "at most {} entries at once",
                        crate::scope_list::MAX_LINES
                    )));
                }
                let over: Vec<&str> = if *wide {
                    add.iter()
                        .filter(|e| crate::gate::too_wide(e).ok().flatten().is_some())
                        .map(|e| e.trim())
                        .collect()
                } else {
                    Vec::new()
                };
                let (before, after, _) = self.engagements.change_scope(|s| {
                    for e in add {
                        if *wide {
                            s.add_wide(e)?;
                        } else {
                            s.add(e)?;
                        }
                    }
                    for e in exclude {
                        s.exclude(e)?;
                    }
                    Ok(())
                })?;
                (
                    before != after,
                    format!(
                        "apply {} in, {} excluded{}",
                        add.len(),
                        exclude.len(),
                        if over.is_empty() {
                            String::new()
                        } else {
                            format!(" (too wide, added with --wide: {})", over.join(", "))
                        }
                    ),
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
        self.flag_as(flag, "human", None).await
    }

    /// [`Replayer::flag`] by `by` (`human`, or `agent:T-3` proposing under
    /// task `task`, #60). Same rules, same fingerprint match, same audit.
    pub async fn flag_as(
        &self,
        flag: crate::candidates::Flag,
        by: &str,
        task: Option<&str>,
    ) -> Response {
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
        // The same issue (#78): exactly (the fingerprint), or in other
        // words (the rule at its granularity, any state).
        let key = crate::candidates::dedupe_key(rule, &entry.url, flag.parameter.as_deref());
        let found = match self.candidates.matching(&eng, &finding) {
            Some(c) => Some(("exact", c)),
            None => self.candidates.similar(&eng, &key).map(|c| ("similar", c)),
        };
        let mut possible_duplicate_of = None;
        if let Some((how, existing)) = found {
            let place = crate::candidates::place(rule, &entry.url, flag.parameter.as_deref());
            if task.is_some() {
                // An agent's: never a sibling. What it saw joins the original.
                return self.seen_again(&eng, existing, how, &place, &entry, &flag, by, task, &now);
            }
            if how == "exact" || !flag.force {
                let error = if how == "exact" {
                    format!(
                        "already flagged as {} (same endpoint, parameter and rule)",
                        existing.id
                    )
                } else {
                    format!(
                        "possibly a duplicate of {} ({}): the same rule on {place}. Add this exchange to its evidence, or flag anyway if it's a different issue.",
                        existing.id, rule.id
                    )
                };
                return json(
                    StatusCode::CONFLICT,
                    serde_json::json!({ "error": error, "match": how, "existing": existing }),
                );
            }
            possible_duplicate_of = Some(existing.id);
        }
        let c = Candidate {
            id: new_id(),
            created: now.clone(),
            updated: now,
            flagged_by: by.into(),
            promoted: None,
            possible_duplicate_of,
            seen_again: Vec::new(),
            notes: Vec::new(),
            notes_in_export: false,
            retests: Vec::new(),
            finding,
        };
        let mut rec = Record::new("candidate", &entry.id, Outcome::CandidateChanged);
        if let Some(t) = task {
            rec.actor = "agent".into();
            rec.task = Some(t.into());
        }
        rec.method = Some(entry.method.clone());
        rec.target = Some(entry.url.clone());
        let verb = if task.is_some() { "propose" } else { "flag" };
        rec.reason = Some(format!("{verb} {} as {}", c.id, rule.id));
        rec.result_id = Some(c.id.clone());
        match self.candidates.add(&eng, c) {
            Ok(c) => {
                self.audit(&rec);
                self.note_address(&eng, &entry.url);
                (StatusCode::CREATED, axum::Json(c)).into_response()
            }
            Err(e) => {
                rec.error = Some(e.to_string());
                self.audit(&rec);
                error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            }
        }
    }

    /// An agent proposed what `existing` already covers (#78): its exchanges
    /// join the original as a sighting (audited) unless they're all there
    /// already. 409 with the original, `match`, `place` and `seen_again`.
    #[allow(clippy::too_many_arguments)]
    fn seen_again(
        &self,
        eng: &str,
        existing: crate::candidates::Candidate,
        how: &str,
        place: &str,
        entry: &TrafficEntry,
        flag: &crate::candidates::Flag,
        by: &str,
        task: Option<&str>,
        now: &str,
    ) -> Response {
        let new: Vec<String> = std::iter::once(&flag.exchange)
            .chain(&flag.replays)
            .filter(|x| !existing.exchanges().contains(x))
            .cloned()
            .collect();
        let mut seen = false;
        let mut existing = existing;
        if !new.is_empty() {
            let s = crate::candidates::Sighting {
                by: by.into(),
                at: now.into(),
                exchanges: new,
            };
            let mut rec = Record::new("candidate", &entry.id, Outcome::CandidateChanged);
            rec.actor = if task.is_some() { "agent" } else { "human" }.into();
            rec.task = task.map(String::from);
            rec.method = Some(entry.method.clone());
            rec.target = Some(entry.url.clone());
            rec.result_id = Some(existing.id.clone());
            rec.reason = Some(format!("seen again {} by {by} ({how} match)", existing.id));
            match self.candidates.seen_again(eng, &existing.id, s) {
                Ok(c) => {
                    existing = c;
                    seen = true;
                }
                Err(e) => rec.error = Some(e.to_string()),
            }
            self.audit(&rec);
        }
        json(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": format!("{} already covers this ({place})", existing.id),
                "match": how,
                "place": place,
                "seen_again": seen,
                "existing": existing,
            }),
        )
    }

    /// Candidate `id` as it would be exported, and what redaction changes
    /// (#49). Nothing is stored or sent.
    pub fn redaction(&self, id: &str) -> Response {
        let eng = self.engagements.active().id;
        let Some(c) = self.candidates.get(&eng, id) else {
            return error(StatusCode::NOT_FOUND, &format!("no candidate {id}"));
        };
        let (finding, changes) = flux::redact::preview(&c.exported());
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

    pub fn resolver(&self) -> Arc<dyn crate::addresses::Resolve> {
        self.resolver
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Where a finding's host resolves now, kept as its baseline if it has
    /// none (#65): a later retest flags a host that moved. In-scope hosts
    /// only; looked up in the background so flagging doesn't wait.
    fn note_address(&self, eng: &str, url: &str) {
        let host = gate::url_host(url).unwrap_or_default();
        if host.is_empty() || self.scope_now().explain(&host).state != gate::HostState::In {
            return;
        }
        if self.addresses.get(eng, &host).is_some() {
            return;
        }
        let (resolver, store, eng) = (self.resolver(), self.addresses.clone(), eng.to_string());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let ips = resolver.lookup(&host).await;
                store.first(
                    &eng,
                    &host,
                    ips,
                    &taphttp_types::Timestamp::now().to_string(),
                );
            });
        }
    }

    /// A human's change to plan `id` (#65): drop or re-budget a task,
    /// discard it, or approve it, which makes its tasks and approves them
    /// in the active engagement (only if the plan is for it, and it's
    /// authorized now). Audited.
    pub fn plan_op(
        &self,
        id: &str,
        op: crate::plans::Op,
    ) -> Result<crate::plans::Plan, (StatusCode, String)> {
        use crate::plans::{Error as PE, Op};
        use crate::tasks::{authorized, Context};
        let status = |e: &PE| match e {
            PE::NotFound(_) => StatusCode::NOT_FOUND,
            PE::Invalid(_) => StatusCode::BAD_REQUEST,
            PE::Conflict(_) => StatusCode::CONFLICT,
            PE::Save(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let plan = self
            .plans
            .find(id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no plan {id}")))?;
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("plan", "", Outcome::TaskChanged);
        rec.engagement = Some(plan.engagement.clone());
        rec.result_id = Some(id.to_string());
        let res = match op {
            Op::Approve => {
                (|| {
                    if plan.status != crate::plans::Status::Draft {
                        return Err((StatusCode::CONFLICT, format!("{id} isn't a draft")));
                    }
                    let e = self.engagements.active();
                    if plan.engagement != e.id {
                        return Err((
                        StatusCode::CONFLICT,
                        format!("{id} is for engagement {}: make it active first (only you switch engagements)", plan.engagement),
                    ));
                    }
                    if plan.tasks.is_empty() {
                        return Err((
                            StatusCode::CONFLICT,
                            format!("{id} has no tasks to approve"),
                        ));
                    }
                    let scope = self.engagements.scope();
                    authorized(&e.rules, &e.period(), &scope, jiff::Timestamp::now())
                        .map_err(|why| (StatusCode::CONFLICT, format!("no tasks: {why}")))?;
                    let cx = Context {
                        rules: &e.rules,
                        scope: &scope,
                        now: &now,
                    };
                    let mut made: Vec<String> = Vec::new();
                    for p in &plan.tasks {
                        match self
                            .tasks
                            .create(&e.id, &p.task, &format!("human (plan {id})"), &cx)
                        {
                            Ok(t) => made.push(t.id),
                            Err(err) => {
                                // All or nothing: the drafts made so far go.
                                for t in &made {
                                    let _ = self.tasks.apply(
                                        &e.id,
                                        t,
                                        &crate::tasks::Op::Halt {
                                            reason: format!("{id} wasn't approved: {err}"),
                                        },
                                        &cx,
                                    );
                                }
                                return Err((StatusCode::CONFLICT, format!("{id}: a task couldn't be made ({err}); nothing was approved")));
                            }
                        }
                    }
                    for t in &made {
                        if let Err(err) =
                            self.tasks.apply(&e.id, t, &crate::tasks::Op::Approve, &cx)
                        {
                            return Err((StatusCode::CONFLICT, format!("{t}: {err}")));
                        }
                    }
                    self.plans
                        .approved(&e.id, id, made, &now)
                        .map_err(|e| (status(&e), e.to_string()))
                })()
            }
            op => self
                .plans
                .edit(&plan.engagement, id, &op, &now)
                .map_err(|e| (status(&e), e.to_string())),
        };
        match &res {
            Ok(p) => {
                rec.reason = Some(match p.status {
                    crate::plans::Status::Approved => {
                        format!("approve {id}: made {}", p.made.join(", "))
                    }
                    crate::plans::Status::Discarded => format!("discard {id}"),
                    crate::plans::Status::Draft => format!("edit {id}"),
                });
            }
            Err((_, why)) => {
                rec.reason = Some(format!("{id}: refused"));
                rec.error = Some(why.clone());
            }
        }
        self.audit(&rec);
        res
    }

    /// Totals quad is scored on (#66): requests charged, model tokens and
    /// agent candidates in `engagement`.
    fn quad_tally(&self, engagement: &str) -> crate::quad::Tally {
        let tasks = self.tasks.list(engagement);
        crate::quad::Tally {
            requests: tasks.iter().map(|t| u64::from(t.spent)).sum(),
            tokens: tasks.iter().map(|t| t.usage.tokens()).sum(),
            candidates: self
                .candidates
                .list(engagement)
                .iter()
                .filter(|c| c.flagged_by.starts_with("agent"))
                .count() as u64,
        }
    }

    fn quad_log(&self, engagement: &str, ids: &[String], phase: &str, workers: u32, seconds: u64) {
        let now = taphttp_types::Timestamp::now().to_string();
        for id in ids {
            let ev = crate::tasks::Event::Quad {
                phase: phase.into(),
                workers,
                seconds,
            };
            if let Err(e) = self.tasks.log(engagement, id, ev, &now) {
                tracing::debug!("{engagement}/{id}: quad {phase}: {e}");
            }
        }
    }

    fn live_tasks(&self, engagement: &str) -> Vec<String> {
        use crate::tasks::Status;
        self.tasks
            .list(engagement)
            .into_iter()
            .filter(|t| {
                matches!(
                    t.status,
                    Status::Approved | Status::Running | Status::WaitingOnApproval
                )
            })
            .map(|t| t.id)
            .collect()
    }

    /// Start quad (#66) for `seconds` (default 120, at most 600) in the
    /// active engagement. Refused while the e-stop is engaged, while one is
    /// on, or with no live agent task to boost. Audited; logged on each live
    /// task with its worker count.
    pub fn quad_start(
        &self,
        seconds: Option<u64>,
    ) -> Result<crate::quad::On, (StatusCode, String)> {
        use crate::quad::{boosted, On, FACTOR, WORKERS};
        self.quad_now();
        let e = self.engagements.active();
        let mut rec = Record::new("quad", "", Outcome::Quad);
        rec.engagement = Some(e.id.clone());
        let res = (|| {
            let secs = crate::quad::seconds(seconds).map_err(|w| (StatusCode::BAD_REQUEST, w))?;
            if self.tasks.estop().is_some() {
                return Err((
                    StatusCode::CONFLICT,
                    "the e-stop is engaged: nothing goes quad".to_string(),
                ));
            }
            let live = self.live_tasks(&e.id);
            if live.is_empty() {
                return Err((
                    StatusCode::CONFLICT,
                    "no live agent task to boost (approved, running or waiting): a halted or finished one never goes quad".to_string(),
                ));
            }
            let capped = live
                .iter()
                .filter_map(|id| self.tasks.get(&e.id, id))
                .any(|t| boosted(t.constraints.rate_limit, e.rules.rate_limit).1)
                .then(|| {
                    format!(
                        "capped at the rules of engagement's {} sends per second",
                        e.rules.rate_limit.unwrap_or_default()
                    )
                });
            let now = jiff::Timestamp::now();
            let until = now
                .checked_add(jiff::SignedDuration::from_secs(secs as i64))
                .unwrap_or(now);
            let on = On {
                engagement: e.id.clone(),
                started: now.to_string(),
                until: until.to_string(),
                seconds: secs,
                workers: WORKERS * FACTOR,
                capped,
                base: self.quad_tally(&e.id),
                tasks: live.clone(),
            };
            let on = self.quad.start(on).map_err(|w| (StatusCode::CONFLICT, w))?;
            self.quad_log(&e.id, &live, "start", on.workers, secs);
            Ok(on)
        })();
        match &res {
            Ok(on) => {
                rec.reason = Some(format!(
                    "start: {}s, {} workers{}",
                    on.seconds,
                    on.workers,
                    on.capped
                        .as_deref()
                        .map(|c| format!(", {c}"))
                        .unwrap_or_default()
                ));
            }
            Err((_, why)) => {
                rec.reason = Some("start".into());
                rec.error = Some(why.clone());
            }
        }
        self.audit(&rec);
        res
    }

    /// End quad now, if it's on: scored, logged, audited.
    pub fn quad_end(&self, ended: crate::quad::Ended) -> Option<crate::quad::Scorecard> {
        let on = self.quad.current()?;
        let tally = self.quad_tally(&on.engagement);
        let (on, card) = self.quad.end(ended, jiff::Timestamp::now(), tally)?;
        self.quad_log(&on.engagement, &on.tasks, "end", on.workers, card.seconds);
        let mut rec = Record::new("quad", "", Outcome::Quad);
        rec.engagement = Some(on.engagement.clone());
        rec.reason = Some(format!("end ({:?}): {}", ended, card.card()));
        self.audit(&rec);
        Some(card)
    }

    /// The burst underway, ending it first if it has worn off.
    pub fn quad_now(&self) -> Option<crate::quad::On> {
        let on = self.quad.current()?;
        if self.quad.active_for(&on.engagement, jiff::Timestamp::now()) {
            return Some(on);
        }
        self.quad_end(crate::quad::Ended::Time);
        None
    }

    /// An exchange from the lever API, or why not (status, message).
    pub async fn exchange_or(&self, id: &str) -> Result<TrafficEntry, (StatusCode, String)> {
        if !crate::web::valid_exchange_id(id) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("not an exchange id: {id:?}"),
            ));
        }
        match self.lever.get_bytes(&format!("/exchanges/{id}")).await {
            Ok((StatusCode::OK, b)) => serde_json::from_slice(&b).map_err(|_| {
                (
                    StatusCode::BAD_GATEWAY,
                    "taphttp returned an unreadable exchange".into(),
                )
            }),
            Ok((StatusCode::NOT_FOUND, _)) => {
                Err((StatusCode::NOT_FOUND, format!("no exchange {id}")))
            }
            Ok((s, _)) => Err((StatusCode::BAD_GATEWAY, format!("taphttp answered {s}"))),
            Err(_) => Err((StatusCode::BAD_GATEWAY, "taphttp is unreachable".into())),
        }
    }

    /// Run verb `name` (#67) on object `id` for `asked_by` (`human` or
    /// `agent:T-n`): draft the task it pre-fills, with its cost. Drafts
    /// only; audited like any task made. `Err` is (status, message).
    pub async fn verb(
        &self,
        asked_by: &str,
        name: &str,
        id: &str,
        arg: Option<&str>,
    ) -> Result<(crate::tasks::Task, String), (StatusCode, String)> {
        use crate::tasks::{authorized, Context, Runner};
        use crate::verbs::{Evidence, Exchange, Object, On};
        let v = crate::verbs::find(name)
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no verb {name:?}")))?;
        let e = self.engagements.active();
        let as_x = |x: &TrafficEntry| Exchange {
            id: x.id.clone(),
            method: x.method.clone(),
            url: x.url.clone(),
        };
        let candidate;
        let obj = match v.on {
            On::Exchange => Object::Exchange(as_x(&self.exchange_or(id).await?)),
            On::Candidate => {
                candidate = self
                    .candidates
                    .get(&e.id, id)
                    .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no candidate {id}")))?;
                let mut exchanges = Vec::new();
                for x in candidate.exchanges().iter().take(10) {
                    match self.exchange_or(x).await {
                        Ok(x) => exchanges.push(as_x(&x)),
                        Err((StatusCode::NOT_FOUND, _)) => {}
                        Err(err) => return Err(err),
                    }
                }
                Object::Candidate(Evidence {
                    candidate: &candidate,
                    exchanges,
                })
            }
        };
        let mut d = crate::verbs::draft(name, &obj, arg, &e.rules)
            .map_err(|why| (StatusCode::BAD_REQUEST, why))?;
        if self.spall_runs.load(Ordering::Relaxed) {
            d.new.runner = Some(Runner::Spall);
        }
        d.new.note = format!("{} on {id}: {}", v.label, d.cost);
        let scope = self.engagements.scope();
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("task", "", Outcome::TaskChanged);
        if let Some(task) = asked_by.strip_prefix("agent:") {
            rec.actor = "agent".into();
            rec.task = Some(task.to_string());
        }
        rec.engagement = Some(e.id.clone());
        rec.reason = Some(format!("{name} on {id}"));
        let made = authorized(&e.rules, &e.period(), &scope, jiff::Timestamp::now())
            .map_err(|why| (StatusCode::CONFLICT, format!("no task: {why}")))
            .and_then(|()| {
                if asked_by.starts_with("agent:")
                    && self
                        .tasks
                        .list(&e.id)
                        .iter()
                        .filter(|t| t.asked_by == asked_by)
                        .count()
                        >= MAX_AGENT_DRAFTS
                {
                    return Err((
                        StatusCode::CONFLICT,
                        format!("{asked_by} has drafted {MAX_AGENT_DRAFTS} tasks: leave the rest to the tester"),
                    ));
                }
                let cx = Context {
                    rules: &e.rules,
                    scope: &scope,
                    now: &now,
                };
                self.tasks
                    .create(&e.id, &d.new, asked_by, &cx)
                    .map_err(|err| (StatusCode::CONFLICT, err.to_string()))
            });
        match &made {
            Ok(t) => {
                rec.reason = Some(format!("{name} on {id}: create {} ({})", t.id, d.cost));
                rec.result_id = Some(t.id.clone());
            }
            Err((_, why)) => rec.error = Some(why.clone()),
        }
        self.audit(&rec);
        made.map(|t| (t, d.cost))
    }

    /// An agent's command its role refused (#64), kept as evidence of a
    /// confused or hijacked agent, like `commands::admit_audited`'s.
    pub fn audit_agent_refusal(&self, engagement: &str, task: &str, name: &str, why: &str) {
        let mut rec = Record::new("command", "", Outcome::Denied);
        rec.actor = "agent".into();
        rec.task = Some(task.to_string());
        rec.engagement = Some(engagement.to_string());
        rec.decision = Some("deny");
        let shown: String = name.chars().take(64).collect();
        rec.reason = Some(format!("{shown:?}: {why}"));
        self.audit(&rec);
    }

    /// The agent's report on its task `id` (#81): stored on the task and
    /// audited as the agent's, refusals too. The text itself isn't audited.
    pub fn agent_report(
        &self,
        id: &str,
        text: &str,
        done: bool,
    ) -> Result<crate::tasks::Task, crate::tasks::Error> {
        let e = self.engagements.active();
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("task", "", Outcome::TaskChanged);
        rec.actor = "agent".into();
        rec.task = Some(id.to_string());
        rec.engagement = Some(e.id.clone());
        rec.result_id = Some(id.to_string());
        rec.reason = Some(format!(
            "report {id} ({} characters){}",
            text.trim().chars().count(),
            if done { ", done" } else { "" }
        ));
        let res = self.tasks.report(&e.id, id, text, done, &now);
        if let Err(err) = &res {
            rec.error = Some(err.to_string());
        }
        self.audit(&rec);
        res
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

    /// Why an agent's send is outside its task, if it is: a host or
    /// method it wasn't given, a send it must ask for, its time or budget
    /// used up. `None` sends (the engagement's rules already allowed it).
    fn outside_task(
        &self,
        t: &crate::tasks::Task,
        scope: &Scope,
        eff: &Effective,
        decision: &Decision,
    ) -> Option<(crate::tasks::Ask, String)> {
        use crate::tasks::Ask;
        let c = &t.constraints;
        let id = &t.id;
        let hosts = std::iter::once(gate::url_host(&eff.url).unwrap_or_default())
            .chain(eff.host_header.clone());
        for h in hosts {
            if !c.allows_host(scope, &h) {
                let host = scope.explain(&h).host;
                let why = format!("{host} isn't one of {id}'s hosts ({})", c.hosts.join(", "));
                return Some((Ask::Host { host }, why));
            }
        }
        let layer = c.layer(&self.engagements.active().rules);
        let method = eff.method.clone();
        if let Err(e) = layer.check_action(Actor::Agent, Class::Send, &eff.method) {
            let why = format!(
                "{id}: {}",
                e.replace("the rules of engagement", "the task's constraints")
            );
            return Some((Ask::Method { method }, why));
        }
        if let Decision::NeedsConfirmation(r) = decision {
            return Some((Ask::Confirm, r.clone()));
        }
        if !layer.is_safe(&eff.method) {
            let why = format!("{id} sends {method} only with a human's approval");
            return Some((Ask::Unsafe { method }, why));
        }
        if let Some(started) = t
            .started
            .as_deref()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        {
            let limit = jiff::SignedDuration::from_mins(i64::from(c.time_limit_minutes));
            if jiff::Timestamp::now().duration_since(started) >= limit {
                let why = format!(
                    "{id} has used its time limit ({} min)",
                    c.time_limit_minutes
                );
                return Some((Ask::Time, why));
            }
        }
        if t.spent >= c.budget {
            let why = format!("{id} has spent its budget of {} requests", c.budget);
            return Some((Ask::Budget, why));
        }
        None
    }

    /// A human's answer to the request task `id` is waiting on (#56).
    /// Audited with what was asked and what was decided.
    pub fn task_decide(&self, id: &str, d: crate::tasks::Decision) -> Response {
        use crate::tasks::{Context, Decision};
        let e = self.engagements.active();
        let scope = self.engagements.scope();
        let now = taphttp_types::Timestamp::now().to_string();
        let mut rec = Record::new("task", "", Outcome::TaskChanged);
        rec.result_id = Some(id.to_string());
        let asked = self
            .tasks
            .get(&e.id, id)
            .and_then(|t| t.pending)
            .map(|p| format!("{} {}", p.method, p.target))
            .unwrap_or_default();
        rec.reason = Some(match &d {
            Decision::Deny { reason } => format!("deny {id} {}: {}", short(&asked), short(reason)),
            other => format!("{} {id} {}", other.name(), short(&asked)),
        });
        let cx = Context {
            rules: &e.rules,
            scope: &scope,
            now: &now,
        };
        match self.tasks.decide(&e.id, id, &d, &cx) {
            Ok((before, after)) => {
                rec.changes = Some(serde_json::json!({
                    "status": { "before": before.status, "after": after.status },
                    "constraints": if before.constraints == after.constraints {
                        serde_json::Value::Null
                    } else {
                        serde_json::json!({ "before": before.constraints, "after": after.constraints })
                    },
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

    /// Engage the agent e-stop (#57): every task in every engagement
    /// halted for good, every confirmation token revoked, and the stop
    /// saved so spall comes back up stopped. Audited.
    pub fn engage_estop(&self, reason: &str) -> Response {
        let reason = match short(reason) {
            r if r.is_empty() => "stopped by a human".to_string(),
            r => r,
        };
        let now = taphttp_types::Timestamp::now().to_string();
        let stopped = self.tasks.engage("human", &reason, &now);
        // The e-stop ends quad at once (#66).
        self.quad_end(crate::quad::Ended::Estop);
        let revoked = self.confirmations.clear();
        self.task_limiters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        let mut rec = Record::new("estop", "", Outcome::EStop);
        rec.reason = Some(format!("engaged: {reason}"));
        rec.changes = Some(serde_json::json!({
            "halted": stopped.halted,
            "tokens_revoked": revoked,
        }));
        if !stopped.errors.is_empty() {
            rec.error = Some(stopped.errors.join("; "));
        }
        self.audit(&rec);
        tracing::warn!(
            "agent e-stop engaged: {reason} ({} tasks halted)",
            stopped.halted.len()
        );
        let mut body = serde_json::to_value(&stopped).unwrap_or_default();
        if stopped.errors.is_empty() {
            json(StatusCode::OK, body)
        } else {
            // Stopped in memory all the same; the tester must know it may
            // not survive a restart.
            body["error"] = format!(
                "agents are stopped, but not everything was saved (a restart may lose it): {}",
                stopped.errors.join("; ")
            )
            .into();
            json(StatusCode::INTERNAL_SERVER_ERROR, body)
        }
    }

    /// Clear the e-stop: new tasks may be approved and run. Nothing halted
    /// comes back. A human's call; audited.
    pub fn clear_estop(&self) -> Response {
        let mut rec = Record::new("estop", "", Outcome::EStop);
        match self.tasks.clear_estop() {
            Ok(was) => {
                rec.reason = Some("cleared".into());
                rec.changes = Some(serde_json::json!({ "was": was }));
                self.audit(&rec);
                json(
                    StatusCode::OK,
                    serde_json::json!({ "estop": null, "was": was }),
                )
            }
            Err(e) => {
                rec.error = Some(e.to_string());
                self.audit(&rec);
                error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            }
        }
    }

    /// Pause task `id` on this exact request (#55): it waits on a human,
    /// and the agent is told why. Nothing is sent, and no token is issued.
    fn pause(
        &self,
        rec: &mut Record,
        engagement: &str,
        id: &str,
        eff: &Effective,
        body: &[u8],
        (ask, reason): (crate::tasks::Ask, String),
    ) -> Response {
        let pending = crate::tasks::Pending {
            method: eff.method.clone(),
            target: eff.url.clone(),
            host_header: eff.host_header.clone(),
            fingerprint: hex(&fingerprint(body)),
            ask,
            request: serde_json::from_slice(body).unwrap_or_default(),
            reason: reason.clone(),
            asked: taphttp_types::Timestamp::now().to_string(),
        };
        rec.decision = Some("pause");
        rec.reason = Some(reason.clone());
        match self.tasks.pause(engagement, id, pending) {
            Ok(_) => {
                rec.outcome = Outcome::Paused;
                self.audit(rec);
                json(
                    StatusCode::ACCEPTED,
                    serde_json::json!({
                        "decision": "paused",
                        "task": id,
                        "status": "waiting_on_approval",
                        "reason": reason,
                    }),
                )
            }
            Err(e) => {
                // Already waiting, halted meanwhile, or not saved: in every
                // case nothing is sent.
                rec.outcome = Outcome::Denied;
                rec.error = Some(e.to_string());
                self.audit(rec);
                json(
                    StatusCode::CONFLICT,
                    serde_json::json!({ "error": format!("{reason}; and {e}"), "decision": "deny" }),
                )
            }
        }
    }

    /// Add a send to its task's log (a no-op for a human's).
    fn task_log(&self, engagement: &str, entry: Option<(String, crate::tasks::Event)>) {
        if let Some((id, ev)) = entry {
            let now = taphttp_types::Timestamp::now().to_string();
            if let Err(e) = self.tasks.log_send(engagement, &id, ev, &now) {
                tracing::warn!("task {id}: logging a send: {e}");
            }
        }
    }

    pub fn audit(&self, rec: &Record) {
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
        self.replay(Who::Human, req).await
    }

    /// Agents only, for now: a human replaying to spall itself is odd but
    /// theirs to do (#73).
    async fn stack_refusal(&self, who: &Who<'_>, url: &str) -> Option<String> {
        match who {
            Who::Human => None,
            Who::Agent { .. } => self.stack.refuses(url).await,
        }
    }

    /// The task an agent sends under (#55), or `None` for a human. An
    /// agent acts only under a running task of the active engagement, and
    /// never confirms: approvals come from a human (#56). `Err` is the
    /// refusal, already audited.
    fn agent_task(
        &self,
        who: Who<'_>,
        engagement: &Engagement,
        rec: &mut Record,
        confirm: bool,
    ) -> Result<Option<crate::tasks::Task>, Box<Response>> {
        let Who::Agent { task } = who else {
            return Ok(None);
        };
        let refuse = |rec: &mut Record, why: String| {
            rec.outcome = Outcome::Denied;
            rec.decision = Some("deny");
            rec.reason = Some(why.clone());
            self.audit(rec);
            json(
                StatusCode::FORBIDDEN,
                serde_json::json!({ "error": why, "decision": "deny" }),
            )
        };
        if let Some(e) = self.tasks.estop() {
            return Err(Box::new(refuse(
                rec,
                format!(
                    "the agent e-stop is engaged (since {}: {}); no agent request is sent, and halted tasks never run again",
                    e.since, e.reason
                ),
            )));
        }
        if confirm {
            return Err(Box::new(refuse(
                rec,
                "agents don't confirm sends: a human approves them (the task waits)".into(),
            )));
        }
        match self.tasks.get(&engagement.id, task) {
            None => Err(Box::new(refuse(
                rec,
                format!("no task {task} in engagement {}", engagement.id),
            ))),
            Some(t) if t.status != crate::tasks::Status::Running => {
                let why = t
                    .reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default();
                let after = if t.status == crate::tasks::Status::Halted {
                    "; a halted task never sends again (give the work as a new task)"
                } else {
                    ""
                };
                Err(Box::new(refuse(
                    rec,
                    format!("{task} is {}{why}{after}", t.status.as_str()),
                )))
            }
            Some(t) => Ok(Some(t)),
        }
    }

    /// An agent's replay under `task` (#55). Same path as a human's, with
    /// the task as credentials: the engagement's rules and scope deny;
    /// anything outside the task's own constraints pauses the task for a
    /// human instead; agents never get or use a confirmation token. The
    /// MCP server (#58) and the agent host (#63) call this; there is no
    /// HTTP route to it.
    pub async fn agent_replay(&self, task: &str, req: ReplayRequest) -> Response {
        self.replay(Who::Agent { task }, req).await
    }

    /// `POST /api/fetch` (#74): a human's new GET or HEAD, from the console.
    pub async fn handle_fetch(&self, raw: &[u8]) -> Response {
        match serde_json::from_slice::<FetchRequest>(raw) {
            Ok(req) => self.fetch(Who::Human, req).await,
            Err(_) => error(StatusCode::BAD_REQUEST, "body must be a JSON fetch request"),
        }
    }

    /// An agent's new GET or HEAD under `task` (#74): the same gate as
    /// [`Self::agent_replay`], sent with taphttp's `POST /send`. No HTTP
    /// route; the MCP server and the agent host call it.
    pub async fn agent_fetch(&self, task: &str, req: FetchRequest) -> Response {
        self.fetch(Who::Agent { task }, req).await
    }

    async fn fetch(&self, who: Who<'_>, req: FetchRequest) -> Response {
        let mut rec = Record::new("fetch", "", Outcome::Error);
        rec.actor = match who {
            Who::Human => "human",
            Who::Agent { .. } => "agent",
        }
        .into();
        if let Who::Agent { task } = who {
            rec.task = Some(task.to_string());
        }
        let eff = match validate_fetch(&req) {
            Ok(eff) => eff,
            Err(msg) => {
                rec.error = Some(msg.clone());
                self.audit(&rec);
                return error(StatusCode::BAD_REQUEST, &msg);
            }
        };
        let engagement = self.engagements.active();
        let task = match self.agent_task(who, &engagement, &mut rec, req.confirm.is_some()) {
            Ok(t) => t,
            Err(res) => return *res,
        };
        let body = fetch_body(&req, &eff, who);
        self.gated_send(
            who,
            task,
            &engagement,
            rec,
            eff,
            body,
            "/send",
            req.confirm.as_deref(),
        )
        .await
    }

    async fn replay(&self, who: Who<'_>, req: ReplayRequest) -> Response {
        let actor_name = match who {
            Who::Human => "human",
            Who::Agent { .. } => "agent",
        };
        let task_id = match who {
            Who::Human => None,
            Who::Agent { task } => Some(task.to_string()),
        };
        let stamp = |mut rec: Record| {
            rec.actor = actor_name.into();
            rec.task = task_id.clone();
            rec
        };
        if let Err(msg) = validate(&req) {
            if crate::web::valid_exchange_id(&req.id) {
                let mut rec = stamp(Record::new("replay", &req.id, Outcome::Error));
                rec.error = Some(msg.clone());
                self.audit(&rec);
            }
            return error(StatusCode::BAD_REQUEST, &msg);
        }

        let mut rec = stamp(Record::new("replay", &req.id, Outcome::Error));
        let engagement = self.engagements.active();
        let task = match self.agent_task(who, &engagement, &mut rec, req.confirm.is_some()) {
            Ok(t) => t,
            Err(res) => return *res,
        };

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
        let body = forward_body_as(&req, who);
        self.gated_send(
            who,
            task,
            &engagement,
            rec,
            eff,
            body,
            "/replay",
            req.confirm.as_deref(),
        )
        .await
    }

    /// Everything after "what would be sent" (#74): the stack guard, scope,
    /// the RoE, the task's own layer, confirmation, rate, the charge, then
    /// taphttp (`path`: `/replay` or `/send`), audited and task-logged.
    #[allow(clippy::too_many_arguments)]
    async fn gated_send(
        &self,
        who: Who<'_>,
        task: Option<crate::tasks::Task>,
        engagement: &Engagement,
        mut rec: Record,
        eff: Effective,
        body: Vec<u8>,
        path: &str,
        confirm: Option<&str>,
    ) -> Response {
        rec.method = Some(eff.method.clone());
        rec.target = Some(eff.url.clone());
        rec.host_header = eff.host_header.clone();
        let scope = self.scope_now();
        rec.scope_all = scope.allows_all();
        // Before scope, the RoE and the task: no scope reaches the stack, and
        // it is a refusal, never a pause a human could approve (#73).
        if let (Some(t), Some(why)) = (&task, self.stack_refusal(&who, &eff.url).await) {
            rec.outcome = Outcome::Denied;
            rec.decision = Some("deny");
            rec.reason = Some(why.clone());
            self.audit(&rec);
            let ev = crate::tasks::Event::Request {
                exchange: None,
                method: eff.method.clone(),
                target: eff.url.clone(),
                check: None,
                outcome: "denied".into(),
                status: None,
            };
            self.task_log(&engagement.id, Some((t.id.clone(), ev)));
            return json(
                StatusCode::FORBIDDEN,
                serde_json::json!({ "error": why, "decision": "deny" }),
            );
        }
        // The task's role (#64): a send it can't make is refused before any
        // pause or grant, since no approval widens a role.
        if let Some(t) = &task {
            let safe = engagement.rules.is_safe(&eff.method);
            if let Err(why) = t.role.may_send(&eff.method, safe) {
                let why = format!("{}: {why}", t.id);
                rec.outcome = Outcome::Denied;
                rec.decision = Some("deny");
                rec.reason = Some(why.clone());
                self.audit(&rec);
                let ev = crate::tasks::Event::Request {
                    exchange: None,
                    method: eff.method.clone(),
                    target: eff.url.clone(),
                    check: None,
                    outcome: "denied".into(),
                    status: None,
                };
                self.task_log(&engagement.id, Some((t.id.clone(), ev)));
                return json(
                    StatusCode::FORBIDDEN,
                    serde_json::json!({ "error": why, "decision": "deny" }),
                );
            }
        }
        let layers = [&engagement.rules];
        let decision = gate::decide(
            &scope,
            &layers,
            &engagement.period(),
            &Action {
                method: &eff.method,
                url: &eff.url,
                host_header: eff.host_header.as_deref(),
                actor: match who {
                    Who::Human => Actor::Human,
                    Who::Agent { .. } => Actor::Agent,
                },
                class: Class::Send,
            },
            jiff::Timestamp::now(),
        );
        rec.decision = Some(decision.kind());
        rec.reason = decision.reason().map(String::from);

        let log = |outcome: &str, exchange: Option<String>, status: Option<u16>| {
            task.as_ref().map(|t| {
                let ev = crate::tasks::Event::Request {
                    exchange,
                    method: eff.method.clone(),
                    target: eff.url.clone(),
                    check: None,
                    outcome: outcome.into(),
                    status,
                };
                (t.id.clone(), ev)
            })
        };
        // An "approve once" for this exact request (#56) lets it past the
        // task's own constraints, never past the engagement's (a denial
        // above still stands). It's used up when charged, below.
        let fp = hex(&fingerprint(&body));
        let now_text = taphttp_types::Timestamp::now().to_string();
        let granted = task.as_ref().is_some_and(|t| {
            t.grant.as_ref().is_some_and(|g| {
                g.fingerprint == fp
                    && g.expires.parse::<jiff::Timestamp>().ok()
                        > now_text.parse::<jiff::Timestamp>().ok()
            })
        });
        if granted {
            rec.confirmed = true;
        }
        // The task's own layer: outside it, the task pauses for a human.
        if let (Some(t), false, false) = (&task, matches!(decision, Decision::Deny(_)), granted) {
            if let Some(ask) = self.outside_task(t, &scope, &eff, &decision) {
                return self.pause(&mut rec, &engagement.id, &t.id, &eff, &body, ask);
            }
        }
        match &decision {
            Decision::Deny(reason) => {
                rec.outcome = Outcome::Denied;
                self.audit(&rec);
                self.task_log(&engagement.id, log("denied", None, None));
                return json(
                    StatusCode::FORBIDDEN,
                    serde_json::json!({ "error": reason, "decision": "deny" }),
                );
            }
            // An agent's send that needs confirmation paused above; should
            // that ever change, it is refused here, never sent.
            // An approval (above) is the human's confirmation.
            Decision::NeedsConfirmation(_) if granted => {}
            Decision::NeedsConfirmation(reason) if task.is_some() => {
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
                match confirm {
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
        let mut wait = self
            .limiter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take(rate, Instant::now());
        let mut rate = rate;
        if let (Ok(()), Some(t)) = (&wait, &task) {
            // Then the task's own ceiling (never above the engagement's).
            // Quad (#66): the task's rate ×4, never past the RoE's (rate_of
            // takes the lower).
            let mut c = t.constraints.clone();
            if self.quad.active_for(&engagement.id, jiff::Timestamp::now()) {
                c.rate_limit = crate::quad::boosted(c.rate_limit, engagement.rules.rate_limit).0;
            }
            let task_rate = roe::rate_of(&[&engagement.rules, &c.layer(&engagement.rules)]);
            if task_rate != rate {
                rate = task_rate;
                wait = self
                    .task_limiters
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(format!("{}/{}", engagement.id, t.id))
                    .or_default()
                    .take(task_rate, Instant::now());
            }
        }
        if let Err(wait) = wait {
            let secs = wait.as_secs_f64();
            let msg = format!(
                "the rules of engagement allow {} sends per second; retry in {secs:.1}s",
                rate.unwrap_or_default()
            );
            rec.outcome = Outcome::RateLimited;
            rec.error = Some(msg.clone());
            self.audit(&rec);
            self.task_log(&engagement.id, log("rate_limited", None, None));
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

        // Charged last, under the task store's lock: racing sends can't
        // overspend, and a task halted meanwhile sends nothing.
        if let Some(t) = &task {
            let now = taphttp_types::Timestamp::now().to_string();
            let charged = if granted {
                self.tasks.charge_granted(&engagement.id, &t.id, &now, &fp)
            } else {
                self.tasks.charge(&engagement.id, &t.id, &now)
            };
            if let Err(e) = charged {
                // Spent by a racing send since the check above: ask for more.
                if task_of_budget(&e) {
                    let reason = format!(
                        "{} has spent its budget of {} requests",
                        t.id, t.constraints.budget
                    );
                    let ask = (crate::tasks::Ask::Budget, reason);
                    return self.pause(&mut rec, &engagement.id, &t.id, &eff, &body, ask);
                }
                rec.outcome = Outcome::Denied;
                rec.decision = Some("deny");
                rec.reason = Some(e.to_string());
                self.audit(&rec);
                return json(
                    StatusCode::CONFLICT,
                    serde_json::json!({ "error": e.to_string(), "decision": "deny" }),
                );
            }
        }

        let sent = self.lever.post_json(path, body, REPLAY_TIMEOUT).await;
        if task.is_some() {
            let (outcome, id, status) = match &sent {
                Ok((st, bytes)) => {
                    let v = upstream_json(bytes);
                    let id = v.get("id").and_then(|x| x.as_str()).map(String::from);
                    let code = v
                        .get("status")
                        .and_then(|x| x.as_u64())
                        .and_then(|x| u16::try_from(x).ok());
                    let outcome = match *st {
                        StatusCode::OK => "sent",
                        StatusCode::BAD_GATEWAY if id.is_some() => "failed",
                        _ => "error",
                    };
                    (outcome, id, code)
                }
                Err(_) => ("error", None, None),
            };
            self.task_log(&engagement.id, log(outcome, id, status));
        }
        match sent {
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
                            .unwrap_or("taphttp refused the request")
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
                tracing::warn!("{} {}: {e:#}", rec.action, eff.url);
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
