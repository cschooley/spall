//! The agent's tools (#58, #60): what an MCP session (#58) or spall's own
//! agent host (#63) can do, generated from the command registry
//! (`commands::offered`) and run here, server-side, under a task.
//!
//! Every call is admitted by `commands::admit_audited` (an agent is never
//! handed a policy or export command, whatever its flag says), acts only
//! for its own task in the active engagement, and is logged to that task.
//! Anything carrying captured traffic goes back through `untrusted` (#59).
//! Errors say what to do next, so an agent doesn't retry blindly.

use crate::commands::{self, Refusal};
use crate::hub::Hub;
use crate::replay::{FetchRequest, ReplayRequest, Replayer};
use crate::roe::Actor;
use crate::tasks::{Event, Status, Task};
use crate::untrusted;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use taphttp_types::TrafficEntry;

/// Candidates one task may propose; past it, a looping agent is told to
/// stop and let the tester catch up.
pub const MAX_PROPOSALS: usize = 20;
/// Exchanges a `filter` returns.
const LIST_LIMIT: usize = 50;

/// A tool's answer, as text for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub is_error: bool,
}

fn ok(text: impl Into<String>) -> Output {
    Output {
        text: text.into(),
        is_error: false,
    }
}

fn err(text: impl Into<String>) -> Output {
    Output {
        text: text.into(),
        is_error: true,
    }
}

/// Everything a tool needs.
#[derive(Clone)]
pub struct Tools {
    pub replayer: Arc<Replayer>,
    pub hub: Arc<Hub>,
}

/// MCP tool definitions: the commands an agent is offered.
pub fn definitions() -> Vec<Value> {
    commands::offered()
        .map(|c| {
            json!({
                "name": c.name,
                "description": c.desc,
                "inputSchema": c.args(),
            })
        })
        .collect()
}

fn args<T: for<'de> Deserialize<'de>>(name: &str, v: &Value) -> Result<T, Output> {
    let v = if v.is_null() { json!({}) } else { v.clone() };
    serde_json::from_value(v)
        .map_err(|e| err(format!("{name}: bad arguments ({e}); see its inputSchema")))
}

/// A response from the replay path as (status, JSON body).
async fn read(res: axum::response::Response) -> (StatusCode, Value) {
    let status = res.status();
    let bytes = res
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn error_of(v: &Value) -> String {
    v.get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("no reason given")
        .to_string()
}

/// What the send path (`replay`, `fetch`) answered, for the model: the
/// recorded exchange through `untrusted`, or what happened and what to do.
fn answer(
    t: &Task,
    status: StatusCode,
    v: &Value,
    what: &str,
    note: &str,
) -> Result<Output, Output> {
    let why = error_of(v);
    match status {
        StatusCode::OK | StatusCode::BAD_GATEWAY if v.get("id").is_some() => {
            let e: Option<TrafficEntry> = serde_json::from_value(v.clone()).ok();
            let head = if status == StatusCode::OK {
                format!("sent{note}: {what}")
            } else {
                format!("sent{note}, but the target didn't answer: {what}")
            };
            let body = match e {
                Some(e) => untrusted::wrap_json("exchange", &untrusted::exchange(&e, None)),
                None => String::new(),
            };
            Ok(ok(format!("{head}\n{body}")))
        }
        StatusCode::ACCEPTED => Err(err(format!(
            "paused{note}: {}. {} now waits on a human's approval of exactly this request. Don't retry it or work around it. Call task to see when it's decided; if it's approved once, send the identical request again; if it's denied, the reason says why.",
            v["reason"].as_str().unwrap_or("outside the task's constraints"),
            t.id
        ))),
        StatusCode::FORBIDDEN => Err(err(format!(
            "refused{note}: {why}. No approval can change this (it's the engagement's scope or rules, or the task can't send): don't retry it; try something else, or stop and tell the tester."
        ))),
        StatusCode::TOO_MANY_REQUESTS => Err(err(format!(
            "rate limited: {why}. Wait {}s, then send it again.",
            v["retry_after_secs"].as_f64().map(|s| format!("{:.1}", s.max(0.1))).unwrap_or_else(|| "a moment".into())
        ))),
        StatusCode::CONFLICT => Err(err(format!("not sent: {why}. Call task to see where it stands."))),
        StatusCode::BAD_REQUEST => Err(err(format!("not sent: {why}"))),
        _ => Err(err(format!("not sent ({status}): {why}"))),
    }
}

impl Tools {
    /// Run tool `name` for task `task`.
    pub async fn call(&self, task: &str, name: &str, a: &Value) -> Output {
        let r = &self.replayer;
        let eng = r.engagements.active();
        if let Err(why) =
            commands::admit_audited(&r.audit, Some(eng.id.clone()), Actor::Agent, name)
        {
            let names: Vec<&str> = commands::offered().map(|c| c.name).collect();
            let next = match why {
                Refusal::Forbidden(_) => "Ask the tester: it's theirs to do in spall.",
                _ => "",
            };
            return err(format!(
                "{name}: {why}. {next} Your tools: {}.",
                names.join(", ")
            ));
        }
        let Some(t) = r.tasks.get(&eng.id, task) else {
            return err(format!(
                "your task {task} isn't in the active engagement ({}): the tester may have switched engagements. Stop and tell them.",
                eng.id
            ));
        };
        // An approved task starts with its agent's first action: that's
        // when it runs and its time limit starts (halt and e-stop still win).
        let t = if t.status == Status::Approved && name != "task" {
            let now = taphttp_types::Timestamp::now().to_string();
            match r
                .tasks
                .transition(&eng.id, task, Status::Running, None, &now)
            {
                Ok(t) => t,
                Err(e) => return err(format!("{task} can't start: {e}")),
            }
        } else {
            t
        };
        if t.status.finished() && name != "task" {
            return err(format!(
                "{task} is {}{}: nothing more runs under it. Stop; the tester gives new work as a new task.",
                t.status.as_str(),
                t.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
            ));
        }
        let out = match name {
            "filter" => self.filter(a).await,
            "open" => self.open(&t, a).await,
            "replay" => self.replay(&t, a).await,
            "fetch" => self.fetch(&t, a).await,
            "hosts" => self.hosts(a),
            "rules" => rules(a),
            "task" => self.task(&t, a),
            "propose_finding" => self.propose(&t, a).await,
            "report" => self.report(&t, a),
            _ => Err(err(format!("{name}: offered but not built yet"))),
        };
        let out = out.unwrap_or_else(|e| e);
        // A send logs its own request; everything else is a command.
        if !matches!(name, "replay" | "fetch") {
            let outcome = if out.is_error { "error" } else { "ok" };
            let now = taphttp_types::Timestamp::now().to_string();
            let _ = r.tasks.log(
                &eng.id,
                task,
                Event::Command {
                    name: name.into(),
                    outcome: outcome.into(),
                },
                &now,
            );
        }
        out
    }

    async fn exchange(&self, id: &str) -> Result<TrafficEntry, Output> {
        if !crate::web::valid_exchange_id(id) {
            return Err(err(format!("not an exchange id: {id:?}")));
        }
        match self
            .replayer
            .lever
            .get_bytes(&format!("/exchanges/{id}"))
            .await
        {
            Ok((StatusCode::OK, b)) => serde_json::from_slice(&b)
                .map_err(|_| err("taphttp returned an unreadable exchange")),
            Ok((StatusCode::NOT_FOUND, _)) => {
                Err(err(format!("no exchange {id}: list some with filter")))
            }
            Ok((s, _)) => Err(err(format!("taphttp answered {s}; try again shortly"))),
            Err(_) => Err(err("taphttp is unreachable; try again shortly")),
        }
    }

    async fn filter(&self, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            #[serde(default)]
            query: String,
        }
        let a: A = args("filter", a)?;
        let mut params: Vec<(String, String)> = vec![("limit".into(), LIST_LIMIT.to_string())];
        let mut words = Vec::new();
        for tok in a.query.split_whitespace() {
            match tok.split_once(':') {
                Some(("host", v)) if !v.is_empty() => params.push(("host".into(), v.into())),
                Some(("method", v))
                    if v.chars().all(|c| c.is_ascii_alphabetic()) && !v.is_empty() =>
                {
                    params.push(("method".into(), v.to_ascii_uppercase()))
                }
                Some(("status", v)) if is_status(v) => {
                    params.push(("status".into(), v.to_ascii_lowercase()))
                }
                _ => words.push(tok),
            }
        }
        if !words.is_empty() {
            params.push(("q".into(), words.join(" ")));
        }
        let qs = serde_urlencoded::to_string(&params).unwrap_or_default();
        let list: Vec<TrafficEntry> = match self
            .replayer
            .lever
            .get_bytes(&format!("/exchanges?{qs}"))
            .await
        {
            Ok((StatusCode::OK, b)) => serde_json::from_slice(&b).unwrap_or_default(),
            Ok((s, _)) => return Err(err(format!("taphttp answered {s}; try again shortly"))),
            Err(_) => return Err(err("taphttp is unreachable; try again shortly")),
        };
        let rows: Vec<Value> = list.iter().map(untrusted::summary).collect();
        Ok(ok(format!(
            "{} exchange(s), newest first (at most {LIST_LIMIT}). open <id> for one.\n{}",
            rows.len(),
            untrusted::wrap_json("exchanges", &json!(rows))
        )))
    }

    async fn open(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            id: String,
            body_offset: Option<usize>,
            body_length: Option<usize>,
        }
        let a: A = args("open", a)?;
        let e = self.exchange(&a.id).await?;
        let range = match (a.body_offset, a.body_length) {
            (None, None) => None,
            (o, l) => Some((o.unwrap_or(0), l.unwrap_or(untrusted::BODY_DEFAULT))),
        };
        let r = &self.replayer;
        let now = taphttp_types::Timestamp::now().to_string();
        let _ = r.tasks.log(
            &t.engagement,
            &t.id,
            Event::Read {
                exchange: e.id.clone(),
            },
            &now,
        );
        Ok(ok(untrusted::wrap_json(
            "exchange",
            &untrusted::exchange(&e, range),
        )))
    }

    async fn replay(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Header {
            name: String,
            value: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            id: String,
            method: Option<String>,
            #[serde(default)]
            headers: Vec<Header>,
            #[serde(default)]
            drop_headers: Vec<String>,
            body: Option<String>,
            #[serde(default)]
            yes: bool,
        }
        let a: A = args("replay", a)?;
        let req_headers = if a.headers.is_empty() && a.drop_headers.is_empty() {
            None
        } else {
            let original = self.exchange(&a.id).await?;
            let gone = |n: &str| {
                a.drop_headers.iter().any(|d| d.eq_ignore_ascii_case(n))
                    || a.headers.iter().any(|h| h.name.eq_ignore_ascii_case(n))
            };
            let mut list: Vec<(String, String)> = original
                .req_headers
                .iter()
                .filter(|(k, _)| !gone(k))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            list.extend(a.headers.iter().map(|h| (h.name.clone(), h.value.clone())));
            Some(list)
        };
        let req = ReplayRequest {
            id: a.id.clone(),
            method: a.method.clone(),
            req_headers,
            body_encoding: a.body.as_ref().map(|_| "utf8".to_string()),
            body: a.body.clone(),
            confirm: None,
        };
        let note = if a.yes {
            " (yes is ignored: agents don't confirm sends; a human approves them)"
        } else {
            ""
        };
        let (status, v) = read(self.replayer.agent_replay(&t.id, req).await).await;
        let what = format!("replay {} of {}", v["id"].as_str().unwrap_or("?"), a.id);
        answer(t, status, &v, &what, note)
    }

    /// A new GET or HEAD (#74): the same gate and answers as `replay`.
    async fn fetch(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Header {
            name: String,
            value: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            url: String,
            method: Option<String>,
            headers: Option<Vec<Header>>,
        }
        let a: A = args("fetch", a)?;
        let req = FetchRequest {
            url: a.url.clone(),
            method: a.method.clone(),
            req_headers: a
                .headers
                .map(|h| h.into_iter().map(|h| (h.name, h.value)).collect()),
            confirm: None,
        };
        let method = req.method.clone().unwrap_or_else(|| "GET".into());
        let (status, v) = read(self.replayer.agent_fetch(&t.id, req).await).await;
        let what = format!(
            "fetch {} ({method} {})",
            v["id"].as_str().unwrap_or("?"),
            a.url
        );
        answer(t, status, &v, &what, "")
    }

    fn hosts(&self, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            #[serde(default)]
            unknown: bool,
        }
        let a: A = args("hosts", a)?;
        let scope = self.replayer.engagements.scope();
        let mut listing = self.hub.hosts.list(&scope);
        if a.unknown {
            listing
                .hosts
                .retain(|h| h.state == crate::gate::HostState::Unknown);
        }
        let v = serde_json::to_value(&listing).unwrap_or_default();
        Ok(ok(format!(
            "hosts seen in traffic, with their scope state (in, out, unknown); only a human changes scope.\n{}",
            untrusted::wrap_json("hosts", &v)
        )))
    }

    fn task(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            id: Option<String>,
        }
        let a: A = args("task", a)?;
        if a.id.as_deref().is_some_and(|i| i != t.id) {
            return Err(err(format!("an agent sees only its own task: {}", t.id)));
        }
        let e = self.replayer.engagements.active();
        let mine = json!({
            "id": t.id,
            "goal": t.goal,
            "role": t.role,
            "status": t.status,
            "reason": t.reason,
            "spent": t.spent,
            "constraints": t.constraints,
            "approved_once_pending": t.grant.is_some(),
            "engagement": {
                "id": e.id,
                "name": e.name,
                "scope": e.scope,
                "rules": {
                    "methods": e.rules.methods,
                    "sent_without_asking": e.rules.safe_methods,
                    "rate_limit": e.rules.rate_limit,
                    "destructive_allowed": e.rules.destructive_allowed,
                },
                "testing_now": self.replayer.testing_now(),
            },
        });
        let what_now = match t.status {
            Status::Draft => "A draft: nothing sends until the tester approves it.",
            Status::Approved => {
                "Approved: it starts with your first action; act within its constraints."
            }
            Status::Running => "Running: you may act within its constraints.",
            Status::WaitingOnApproval => {
                "Waiting on the tester's approval: don't send; check back."
            }
            Status::Done | Status::Halted | Status::Failed => "Finished: stop.",
        };
        let recent: Vec<&crate::tasks::Entry> = t.log.iter().rev().take(20).collect();
        let activity = json!({ "pending": t.pending, "recent_log_newest_first": recent });
        Ok(ok(format!(
            "{what_now}\n{}\nRecent activity (requests carry captured URLs):\n{}",
            serde_json::to_string_pretty(&mine).unwrap_or_default(),
            untrusted::wrap_json("task activity", &activity)
        )))
    }

    fn report(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            text: String,
            #[serde(default)]
            done: bool,
        }
        let a: A = args("report", a)?;
        match self.replayer.agent_report(&t.id, &a.text, a.done) {
            Ok(t) if t.status == Status::Done => Ok(ok(format!(
                "filed, and {} is done: the tester sees your report on it. Nothing more runs under it; stop here.",
                t.id
            ))),
            Ok(_) => Ok(ok(format!(
                "filed: the tester sees it on {}. Filing again replaces it; file with done: true when you're finished.",
                t.id
            ))),
            Err(e) => Err(err(format!("report not filed: {e}"))),
        }
    }

    async fn propose(&self, t: &Task, a: &Value) -> Result<Output, Output> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct A {
            exchange_id: String,
            rule: String,
            message: String,
            parameter: Option<String>,
            level: Option<String>,
            #[serde(default)]
            evidence: Vec<String>,
        }
        let a: A = args("propose_finding", a)?;
        // A name, so the same input isn't a new finding each time it's
        // described differently (#78).
        if let Some(p) = &a.parameter {
            let name_ok = !p.is_empty()
                && p.len() <= 100
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.[]$:@".contains(c));
            if !name_ok {
                return Err(err(format!(
                    "parameter must be the name of the query parameter, header, cookie or field it's about (like `id`, `Server` or `session`), not a description: {p:?}. Describe it in message; leave parameter out for a whole-host or whole-endpoint issue."
                )));
            }
        }
        let proposed = t
            .log
            .iter()
            .filter(|e| matches!(&e.event, Event::Command { name, outcome } if name == "propose_finding" && outcome == "ok"))
            .count();
        if proposed >= MAX_PROPOSALS {
            return Err(err(format!(
                "{} has proposed {MAX_PROPOSALS} candidates already: that's the cap per task. Stop proposing and let the tester triage.",
                t.id
            )));
        }
        // Evidence is real: exchanges this task sent or read.
        let seen = |x: &str| {
            t.log.iter().any(|e| match &e.event {
                Event::Request {
                    exchange: Some(id), ..
                } => id == x,
                Event::Read { exchange } => exchange == x,
                _ => false,
            })
        };
        let all: Vec<&String> = std::iter::once(&a.exchange_id)
            .chain(a.evidence.iter())
            .collect();
        let unseen: Vec<&str> = all
            .iter()
            .filter(|x| !seen(x))
            .map(|x| x.as_str())
            .collect();
        if !unseen.is_empty() {
            return Err(err(format!(
                "evidence must be exchanges {} sent or read (open them first): {}",
                t.id,
                unseen.join(", ")
            )));
        }
        let flag: crate::candidates::Flag = serde_json::from_value(json!({
            "exchange": a.exchange_id,
            "rule": a.rule,
            "parameter": a.parameter,
            "level": a.level,
            "message": a.message,
            "replays": a.evidence,
        }))
        .map_err(|e| err(format!("propose_finding: {e}")))?;
        let by = format!("agent:{}", t.id);
        let (status, v) = read(self.replayer.flag_as(flag, &by, Some(&t.id)).await).await;
        match status {
            StatusCode::CREATED => Ok(ok(format!(
                "proposed {} ({}): it waits in the tester's Findings queue as a candidate. Only a human confirms or promotes it.",
                v["id"].as_str().unwrap_or("?"),
                a.rule
            ))),
            StatusCode::CONFLICT => {
                let ex = &v["existing"];
                let id = ex["id"].as_str().unwrap_or("?");
                let place = v["place"].as_str().unwrap_or("the same place");
                let added = if v["seen_again"] == true {
                    "; your evidence was added to it"
                } else {
                    ""
                };
                let sup = ex["finding"]["suppressions"].as_array().and_then(|s| s.first()).cloned();
                Err(err(match sup {
                    Some(s) => format!(
                        "already decided: {id} covers this ({}, {place}) and the tester set it aside ({}): {}{added}. Don't propose it again.",
                        a.rule,
                        s["properties"]["flux/disposition"].as_str().unwrap_or("suppressed"),
                        s["justification"].as_str().unwrap_or("")
                    ),
                    None => format!(
                        "duplicate: {id} ({}) already covers {} on {place}{added}. It's in the tester's hands; don't propose it again (this doesn't count against your cap).",
                        ex["finding"]["properties"]["flux/status"].as_str().unwrap_or("candidate"),
                        a.rule
                    ),
                }))
            }
            _ => Err(err(format!("not proposed: {}", error_of(&v)))),
        }
    }
}

fn is_status(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 3
        && (b'1'..=b'5').contains(&b[0])
        && ((b[1].is_ascii_digit() && b[2].is_ascii_digit())
            || (b[1].eq_ignore_ascii_case(&b'x') && b[2].eq_ignore_ascii_case(&b'x')))
}

fn rules(a: &Value) -> Result<Output, Output> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct A {
        #[serde(default)]
        text: String,
    }
    let a: A = args("rules", a)?;
    let t = a.text.to_lowercase();
    let list: Vec<Value> = crate::rules::CATALOG
        .iter()
        .filter(|e| {
            t.is_empty()
                || e.id.to_lowercase().contains(&t)
                || e.title.to_lowercase().contains(&t)
                || e.cwe
                    .is_some_and(|c| t.trim_start_matches("cwe-") == c.to_string())
        })
        .map(|e| json!({ "id": e.id, "title": e.title, "cwe": e.cwe }))
        .collect();
    Ok(ok(format!(
        "{} rule(s) ({}); use the id in propose_finding.\n{}",
        list.len(),
        crate::rules::WSTG_VERSION,
        serde_json::to_string_pretty(&list).unwrap_or_default()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_are_the_offered_commands_and_nothing_a_human_keeps() {
        let names: Vec<String> = definitions()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "filter",
                "open",
                "replay",
                "fetch",
                "hosts",
                "rules",
                "task",
                "propose_finding",
                "report"
            ]
        );
        for d in definitions() {
            assert_eq!(d["inputSchema"]["type"], "object");
            assert!(d["description"].as_str().is_some_and(|s| !s.is_empty()));
        }
        for n in [
            "scope",
            "roe",
            "engagement",
            "promote",
            "export",
            "estop",
            "flag",
        ] {
            assert!(!names.iter().any(|x| x == n), "{n}");
        }
    }

    #[test]
    fn status_filters() {
        for ok in ["200", "404", "5xx", "2XX"] {
            assert!(is_status(ok), "{ok}");
        }
        for bad in ["600", "20", "2x0", "abc", ""] {
            assert!(!is_status(bad), "{bad}");
        }
    }

    #[test]
    fn rules_search_by_id_title_or_cwe() {
        let out = rules(&json!({"text": "cwe-89"})).unwrap();
        assert!(out.text.contains("WSTG-INPV-05"), "{}", out.text);
        assert!(rules(&json!({"bogus": 1}))
            .unwrap_err()
            .text
            .contains("bad arguments"));
        assert!(rules(&Value::Null).unwrap().text.starts_with("75 rule(s)"));
    }
}
