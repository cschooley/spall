//! spall as agent host (#63): the loop that calls the tester's model and
//! runs its tool calls, for tasks run by spall (`Runner::Spall`).
//!
//! The loop holds no power of its own. Its tools are the registry's agent
//! commands, run by `agent::Tools::call` exactly as an MCP session's are
//! (admitted, logged to the task, untrusted output wrapped), so every send
//! goes through the gate; there is no MCP hop and no side door.
//!
//! - It starts an approved task and stops when the task finishes: the
//!   model ends its turn (done), declines or fails (failed), or a human
//!   halt, the e-stop or the token cap ends it (halted). A model call in
//!   flight is dropped the moment that happens.
//! - While the task waits on approval it parks, then tells the model what
//!   the tester decided, and any steering notes, before its next turn.
//! - Every model call is counted on the task (tokens, calls, time); each
//!   turn's text goes in the task log, truncated, and the whole
//!   conversation (0600) beside the task file.

use crate::agent::{Output, Tools};
use crate::model::{Conversation, Model, Stop, ToolDef};
use crate::tasks::{Event, Runner, Status, Task};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the host looks for work and checks a running task's state.
pub const TICK: Duration = Duration::from_millis(250);
/// A turn's text in the task log; the transcript has all of it.
pub const MAX_TURN_TEXT: usize = 4000;
/// Model turns one task may take, whatever its token cap.
pub const MAX_TURNS: u32 = 500;

const HOW: &str = "You are run by spall itself, not by an outside client: there is no human in this \
conversation, only tool results and, between turns, the tester's notes and approval decisions, which \
spall adds as plain text after your tool results. Work through your task with your tools. When the \
task is done, or you can't go further, file your report (report), then answer with a short closing line \
and no tool calls: that ends the task. If you end without filing one, your closing answer becomes the report. A paused request is decided by \
the tester; you'll be told the outcome here before your next turn.";

/// The system prompt for `t`. Fixed for the task's whole conversation.
pub fn system_prompt(t: &Task) -> String {
    format!(
        "{}\n\n{HOW}\n\nYour task is {}.\n\n{}",
        crate::mcp::INSTRUCTIONS,
        t.id,
        t.role.brief()
    )
}

/// The conversation's first message: the task as the tester wrote it.
pub fn opening(t: &Task) -> String {
    let mut s = format!("Task {}: {}\n", t.id, t.goal);
    if !t.plan.is_empty() {
        s.push_str("\nPlan:\n");
        for (i, step) in t.plan.iter().enumerate() {
            s.push_str(&format!("{}. {}\n", i + 1, step.text));
        }
    }
    if !t.note.is_empty() {
        s.push_str(&format!("\nThe tester's note: {}\n", t.note));
    }
    s.push_str(
        "\nCall task first to see your constraints, the engagement's scope and rules, then begin.",
    );
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push_str(" … (truncated; the whole turn is in the task's transcript)");
    t
}

/// What happened under the task since the model last looked: the
/// tester's notes and approval decisions, as text for the model.
fn news(t: &Task, seen: usize) -> Option<String> {
    let lines: Vec<String> = t
        .log
        .iter()
        .skip(seen)
        .filter_map(|e| match &e.event {
            Event::Note { by, text } => Some(format!("- {by} (the tester) says: {text}")),
            Event::Approval { what, outcome } if outcome != "asked" => {
                Some(format!("- approval of {what}: {outcome}"))
            }
            _ => None,
        })
        .collect();
    (!lines.is_empty()).then(|| format!("Since your last turn:\n{}", lines.join("\n")))
}

pub struct Host {
    tools: Tools,
    model: Arc<dyn Model>,
    dir: PathBuf,
    driving: Mutex<HashSet<String>>,
}

impl Host {
    /// A host over `model`, keeping transcripts under `dir`.
    pub fn new(tools: Tools, model: Arc<dyn Model>, dir: &Path) -> Arc<Host> {
        Arc::new(Host {
            tools,
            model,
            dir: dir.to_path_buf(),
            driving: Mutex::new(HashSet::new()),
        })
    }

    /// Provider and model, for the status bar.
    pub fn describe(&self) -> String {
        self.model.describe()
    }

    fn transcript(&self, eng: &str, id: &str) -> PathBuf {
        self.dir.join(eng).join(format!("{id}.json"))
    }

    /// The task's whole conversation, if it has one.
    pub fn read_transcript(&self, eng: &str, id: &str) -> Option<Conversation> {
        let bytes = std::fs::read(self.transcript(eng, id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_transcript(&self, eng: &str, id: &str, c: &Conversation) {
        let path = self.transcript(eng, id);
        let r = path
            .parent()
            .map(|d| crate::engagement::create_dir_private(d).map_err(anyhow::Error::from))
            .transpose()
            .and_then(|_| Ok(serde_json::to_vec(c)?))
            .and_then(|b| crate::engagement::write_atomic(&path, &b));
        if let Err(e) = r {
            tracing::warn!("saving {eng}/{id}'s transcript: {e:#}");
        }
    }

    /// Look for spall-run tasks to drive, forever.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        loop {
            self.clone().pick_up();
            tokio::time::sleep(TICK).await;
        }
    }

    /// Start a driver for each spall-run task in the active engagement
    /// that's approved or underway and not driven yet.
    pub fn pick_up(self: Arc<Self>) {
        let r = &self.tools.replayer;
        if r.tasks.estop().is_some() {
            return;
        }
        let eng = r.engagements.active().id;
        for t in r.tasks.list(&eng) {
            if t.runner != Runner::Spall
                || !matches!(
                    t.status,
                    Status::Approved | Status::Running | Status::WaitingOnApproval
                )
            {
                continue;
            }
            let key = format!("{eng}/{}", t.id);
            if !self.driving_lock().insert(key.clone()) {
                continue;
            }
            let host = self.clone();
            let eng = eng.clone();
            tokio::spawn(async move {
                host.drive(&eng, &t.id).await;
                host.driving_lock().remove(&key);
            });
        }
    }

    fn driving_lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.driving.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now() -> String {
        taphttp_types::Timestamp::now().to_string()
    }

    /// The task, while the loop may still act for it.
    fn live(&self, eng: &str, id: &str) -> Option<Task> {
        let r = &self.tools.replayer;
        if r.tasks.estop().is_some() {
            return None;
        }
        r.tasks.get(eng, id).filter(|t| !t.status.finished())
    }

    /// Resolves when the loop must stop: the task finished (halt, e-stop)
    /// or is gone.
    async fn ended(&self, eng: &str, id: &str) {
        while self.live(eng, id).is_some() {
            tokio::time::sleep(TICK).await;
        }
    }

    /// The model ended without filing a report (#81): its closing answer
    /// becomes one, so what it found isn't lost with the conversation.
    fn closing_report(&self, eng: &str, id: &str, text: &str) {
        let r = &self.tools.replayer;
        let text = text.trim();
        if text.is_empty() || r.tasks.get(eng, id).is_none_or(|t| t.report.is_some()) {
            return;
        }
        let text: String = text.chars().take(crate::tasks::MAX_REPORT).collect();
        if let Err(e) = r.agent_report(id, &text, false) {
            tracing::debug!("{eng}/{id}: closing report: {e}");
        }
    }

    fn end(&self, eng: &str, id: &str, to: Status, reason: Option<String>) {
        let r = &self.tools.replayer;
        if let Err(e) = r.tasks.transition(eng, id, to, reason, &Self::now()) {
            // Already finished (halted, e-stop): that's the end too.
            tracing::debug!("{eng}/{id}: {e}");
        }
    }

    /// Run task `id`'s loop until it finishes.
    pub async fn drive(&self, eng: &str, id: &str) {
        let r = &self.tools.replayer;
        let Some(t) = self.live(eng, id) else { return };
        let t = if t.status == Status::Approved {
            match r
                .tasks
                .transition(eng, id, Status::Running, None, &Self::now())
            {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("{eng}/{id} can't start: {e}");
                    return;
                }
            }
        } else {
            t
        };
        let tools: Vec<ToolDef> = crate::model::tool_defs(t.role);
        let system = system_prompt(&t);
        let mut c = self
            .read_transcript(eng, id)
            .unwrap_or_else(|| self.model.start(&opening(&t)));
        // Notes and decisions before now are in the opening or already told.
        let mut seen = t.log.len();
        let mut turns = 0u32;
        loop {
            let Some(t) = self.live(eng, id) else { return };
            if r.engagements.active().id != eng || t.status == Status::WaitingOnApproval {
                tokio::time::sleep(TICK).await;
                continue;
            }
            if let Some(n) = news(&t, seen) {
                self.model.answer(&mut c, &[], Some(&n));
            }
            seen = t.log.len();
            if turns >= MAX_TURNS {
                self.end(
                    eng,
                    id,
                    Status::Failed,
                    Some(format!("took {MAX_TURNS} model turns without finishing")),
                );
                return;
            }
            turns += 1;
            let started = Instant::now();
            let turn = tokio::select! {
                turn = self.model.turn(&system, &tools, &mut c) => turn,
                () = self.ended(eng, id) => return,
            };
            let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let turn = match turn {
                Ok(turn) => turn,
                Err(e) => {
                    self.end(eng, id, Status::Failed, Some(format!("model: {e}")));
                    return;
                }
            };
            let now = Self::now();
            match r.tasks.charge_model(eng, id, turn.usage, ms, &now) {
                Ok(t) if t.status.finished() => {
                    self.save_transcript(eng, id, &c);
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("{eng}/{id}: counting the model call: {e}");
                    return;
                }
            }
            if !turn.text.trim().is_empty() {
                let text = truncate(turn.text.trim(), MAX_TURN_TEXT);
                if let Err(e) = r.tasks.log(eng, id, Event::Turn { text }, &now) {
                    tracing::debug!("{eng}/{id}: {e}");
                }
            }
            match &turn.stop {
                Stop::EndTurn if turn.calls.is_empty() => {
                    self.save_transcript(eng, id, &c);
                    self.closing_report(eng, id, &turn.text);
                    self.end(eng, id, Status::Done, None);
                    return;
                }
                Stop::Refusal => {
                    self.save_transcript(eng, id, &c);
                    self.end(
                        eng,
                        id,
                        Status::Failed,
                        Some("the model declined to go on".into()),
                    );
                    return;
                }
                Stop::Other(why) if turn.calls.is_empty() => {
                    self.save_transcript(eng, id, &c);
                    self.end(
                        eng,
                        id,
                        Status::Failed,
                        Some(format!("the model stopped ({why})")),
                    );
                    return;
                }
                Stop::MaxTokens if turn.calls.is_empty() => {
                    self.model.answer(
                        &mut c,
                        &[],
                        Some("Your last answer was cut off at the output limit. Go on, more briefly."),
                    );
                    self.save_transcript(eng, id, &c);
                    continue;
                }
                _ => {}
            }
            let mut results: Vec<(String, Output)> = Vec::new();
            for call in &turn.calls {
                if self.live(eng, id).is_none() {
                    self.save_transcript(eng, id, &c);
                    return;
                }
                let out = if turn.stop == Stop::MaxTokens {
                    Output {
                        text: format!("{}: not run. Your answer was cut off at the output limit, so its arguments may be incomplete. Make the call again, with less around it.", call.name),
                        is_error: true,
                    }
                } else {
                    match &call.input {
                        Ok(a) => self.tools.call(id, &call.name, a).await,
                        Err(e) => Output {
                            text: format!("{}: not run. Its arguments weren't JSON ({e}); see its input schema.", call.name),
                            is_error: true,
                        },
                    }
                };
                results.push((call.id.clone(), out));
            }
            self.model.answer(&mut c, &results, None);
            self.save_transcript(eng, id, &c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Entry;

    fn entry(event: Event) -> Entry {
        Entry {
            ts: "2026-11-24T10:00:00Z".into(),
            event,
        }
    }

    #[test]
    fn news_is_notes_and_decisions_since_last_look() {
        let log = vec![
            entry(Event::Note {
                by: "human".into(),
                text: "old".into(),
            }),
            entry(Event::Approval {
                what: "POST https://app.test/x".into(),
                outcome: "asked".into(),
            }),
            entry(Event::Command {
                name: "filter".into(),
                outcome: "ok".into(),
            }),
            entry(Event::Approval {
                what: "POST https://app.test/x".into(),
                outcome: "denied: not today".into(),
            }),
            entry(Event::Note {
                by: "human".into(),
                text: "try /admin".into(),
            }),
        ];
        let t = Task {
            log,
            ..serde_json::from_value(serde_json::json!({
                "id": "T-1", "engagement": "e", "goal": "g", "role": "tester",
                "constraints": {"hosts": [], "methods": [], "safe_methods": [], "rate_limit": null,
                    "destructive": false, "budget": 1, "time_limit_minutes": 1},
                "plan": [], "status": "running", "reason": null, "spent": 0, "asked_by": "human",
                "note": "", "created": "x", "updated": "x", "started": null, "ended": null, "log": []
            }))
            .unwrap()
        };
        let n = news(&t, 1).unwrap();
        assert!(!n.contains("old") && !n.contains("asked"), "{n}");
        assert!(
            n.contains("denied: not today") && n.contains("try /admin"),
            "{n}"
        );
        assert_eq!(news(&t, 5), None);
        assert!(opening(&t).starts_with("Task T-1: g"));
        assert!(system_prompt(&t).contains("untrusted"));
    }

    #[test]
    fn long_turns_are_truncated_in_the_log() {
        let s = "x".repeat(MAX_TURN_TEXT + 10);
        let t = truncate(&s, MAX_TURN_TEXT);
        assert!(t.ends_with("transcript)"));
        assert_eq!(truncate("short", 10), "short");
    }
}
