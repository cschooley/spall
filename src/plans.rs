//! Plans (#65): what the intake's orchestrator makes of a tester's request.
//! A plan is a list of role tasks (each a `tasks::New` with its cost), the
//! checks it raised (warnings, a pick list, a proposal for a human) and an
//! estimate. It is a draft until the tester approves it; only then are its
//! tasks made, approved, and run through the gate like any other. Chat
//! produces plans; only approved tasks act.
//!
//! Stored per engagement at `<engagements>/plans/<engagement>.json` (0600).

use crate::engagement::{create_dir_private, write_atomic};
use crate::tasks::New;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_PLANS: usize = 200;
pub const MAX_TASKS: usize = 50;
pub const MAX_REQUEST: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Approved,
    Discarded,
}

/// One task the plan would make.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Planned {
    pub task: New,
    /// What it costs, in words ("~5 requests, safe methods").
    pub cost: String,
    /// Requests it may send (its budget).
    pub requests: u32,
    /// It may send methods the RoE wants asked for.
    #[serde(default)]
    pub unsafe_sends: bool,
    /// The candidate it's about, for a retest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

/// Ambiguity, shown as a list to pick from, never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub question: String,
    pub options: Vec<String>,
}

/// Something only a human can do, drafted for them: an engagement change
/// is policy, never the orchestrator's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Proposal {
    /// Extend an engagement whose window has closed.
    Extension {
        engagement: String,
        ends: Option<String>,
        why: String,
    },
    /// A new engagement, when there's none authorized to work in.
    Engagement {
        name: String,
        scope: Vec<String>,
        why: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    /// The engagement the plan works in (not always the active one).
    pub engagement: String,
    /// The tester's words, kept: "my boss said" is a note, not a mystery.
    pub request: String,
    pub asked_by: String,
    pub status: Status,
    /// The orchestrator's one-line summary.
    pub summary: String,
    pub tasks: Vec<Planned>,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    pub created: String,
    pub updated: String,
    /// The tasks made when it was approved.
    #[serde(default)]
    pub made: Vec<String>,
}

/// "14 tasks, ~300 requests, 2 unsafe".
pub fn estimate(tasks: &[Planned]) -> String {
    let requests: u64 = tasks.iter().map(|t| u64::from(t.requests)).sum();
    let unsafe_n = tasks.iter().filter(|t| t.unsafe_sends).count();
    let n = tasks.len();
    format!(
        "{n} task{}, ~{requests} requests, {}",
        if n == 1 { "" } else { "s" },
        if unsafe_n == 0 {
            "safe methods only".to_string()
        } else {
            format!("{unsafe_n} with unsafe sends (each asked for)")
        }
    )
}

/// The tester's change to a draft plan.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Leave task `index` (from 0) out.
    Drop {
        index: usize,
    },
    /// Give task `index` this budget.
    Budget {
        index: usize,
        budget: u32,
    },
    Discard,
    /// Make, approve and start its tasks (the caller does that).
    Approve,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    Conflict(String),
    Save(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no plan {id}"),
            Error::Invalid(s) | Error::Conflict(s) => f.write_str(s),
            Error::Save(s) => write!(f, "saving the plan: {s}"),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    next: u32,
    plans: Vec<Plan>,
}

pub struct Store {
    dir: PathBuf,
    books: Mutex<HashMap<String, Book>>,
}

fn valid(id: &str) -> bool {
    (1..=48).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Store {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        create_dir_private(dir)?;
        let mut books = HashMap::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            let Some(eng) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| path.extension().and_then(|x| x.to_str()) == Some("json"))
            else {
                continue;
            };
            let bytes = std::fs::read(&path)?;
            let book: Book = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            books.insert(eng.to_string(), book);
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            books: Mutex::new(books),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Book>> {
        self.books.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn change<T>(
        &self,
        eng: &str,
        f: impl FnOnce(&mut Book) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if !valid(eng) {
            return Err(Error::Invalid(format!("not an engagement id: {eng:?}")));
        }
        let mut all = self.lock();
        let mut book = all.remove(eng).unwrap_or_default();
        let before = serde_json::to_vec(&book).unwrap_or_default();
        let out = f(&mut book);
        let saved = match &out {
            Ok(_) => serde_json::to_vec(&book)
                .map_err(anyhow::Error::from)
                .and_then(|b| write_atomic(&self.dir.join(format!("{eng}.json")), &b)),
            Err(_) => Ok(()),
        };
        if out.is_err() || saved.is_err() {
            book = serde_json::from_slice(&before).unwrap_or_default();
        }
        all.insert(eng.to_string(), book);
        saved.map_err(|e| Error::Save(format!("{e:#}")))?;
        out
    }

    /// Keep a new draft plan; its id (unique across engagements) is given
    /// here.
    pub fn add(&self, mut plan: Plan) -> Result<Plan, Error> {
        if plan.tasks.len() > MAX_TASKS {
            return Err(Error::Invalid(format!(
                "a plan has at most {MAX_TASKS} tasks"
            )));
        }
        let eng = plan.engagement.clone();
        if !valid(&eng) {
            return Err(Error::Invalid(format!("not an engagement id: {eng:?}")));
        }
        let mut all = self.lock();
        let next = all.values().map(|b| b.next).max().unwrap_or(0).max(1);
        let mut book = all.remove(&eng).unwrap_or_default();
        let before = serde_json::to_vec(&book).unwrap_or_default();
        if book.plans.len() >= MAX_PLANS {
            book.plans.remove(0);
        }
        plan.id = format!("P-{next}");
        book.next = next + 1;
        book.plans.push(plan.clone());
        let saved = serde_json::to_vec(&book)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.dir.join(format!("{eng}.json")), &b));
        if saved.is_err() {
            book = serde_json::from_slice(&before).unwrap_or_default();
        }
        all.insert(eng, book);
        saved.map_err(|e| Error::Save(format!("{e:#}")))?;
        Ok(plan)
    }

    /// Every plan, newest first, across engagements.
    pub fn list(&self) -> Vec<Plan> {
        let mut all: Vec<Plan> = self
            .lock()
            .values()
            .flat_map(|b| b.plans.iter().cloned())
            .collect();
        all.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        all
    }

    pub fn find(&self, id: &str) -> Option<Plan> {
        self.lock()
            .values()
            .flat_map(|b| b.plans.iter())
            .find(|p| p.id == id)
            .cloned()
    }

    /// A human's edit of a draft (drop, budget, discard). `Approve` is the
    /// caller's: it makes the tasks, then [`Store::approved`].
    pub fn edit(&self, eng: &str, id: &str, op: &Op, now: &str) -> Result<Plan, Error> {
        self.change(eng, |book| {
            let p = book
                .plans
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| Error::NotFound(id.into()))?;
            if p.status != Status::Draft {
                return Err(Error::Conflict(format!(
                    "{id} is {}: only a draft changes",
                    if p.status == Status::Approved {
                        "approved"
                    } else {
                        "discarded"
                    }
                )));
            }
            let at = |i: usize, n: usize| {
                if i < n {
                    Ok(i)
                } else {
                    Err(Error::Invalid(format!("no task {i} in {id} (it has {n})")))
                }
            };
            match op {
                Op::Drop { index } => {
                    let i = at(*index, p.tasks.len())?;
                    p.tasks.remove(i);
                }
                Op::Budget { index, budget } => {
                    let i = at(*index, p.tasks.len())?;
                    if !(1..=crate::tasks::MAX_BUDGET).contains(budget) {
                        return Err(Error::Invalid(format!(
                            "budget: 1 to {}",
                            crate::tasks::MAX_BUDGET
                        )));
                    }
                    let t = &mut p.tasks[i];
                    t.requests = *budget;
                    t.task.constraints.budget = Some(*budget);
                    t.cost = format!("~{budget} requests");
                }
                Op::Discard => p.status = Status::Discarded,
                Op::Approve => {
                    return Err(Error::Invalid("approve makes tasks: not an edit".into()))
                }
            }
            p.updated = now.into();
            Ok(p.clone())
        })
    }

    /// Mark `id` approved, with the tasks it made.
    pub fn approved(
        &self,
        eng: &str,
        id: &str,
        made: Vec<String>,
        now: &str,
    ) -> Result<Plan, Error> {
        self.change(eng, |book| {
            let p = book
                .plans
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| Error::NotFound(id.into()))?;
            p.status = Status::Approved;
            p.made = made;
            p.updated = now.into();
            Ok(p.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(requests: u32, unsafe_sends: bool) -> Planned {
        Planned {
            task: New {
                goal: "retest".into(),
                role: crate::tasks::Role::Tester,
                runner: None,
                constraints: Default::default(),
                plan: Vec::new(),
                note: String::new(),
            },
            cost: format!("~{requests} requests"),
            requests,
            unsafe_sends,
            candidate: None,
        }
    }

    fn plan(eng: &str, tasks: Vec<Planned>) -> Plan {
        Plan {
            id: String::new(),
            engagement: eng.into(),
            request: "re-scan last week's findings".into(),
            asked_by: "human".into(),
            status: Status::Draft,
            summary: String::new(),
            tasks,
            warnings: Vec::new(),
            choice: None,
            proposal: None,
            created: "2026-11-24T10:00:00Z".into(),
            updated: "2026-11-24T10:00:00Z".into(),
            made: Vec::new(),
        }
    }

    #[test]
    fn the_estimate_counts_tasks_requests_and_unsafe_ones() {
        let ts: Vec<Planned> = (0..14).map(|i| planned(20 + (i % 3), i < 2)).collect();
        assert_eq!(
            estimate(&ts),
            "14 tasks, ~293 requests, 2 with unsafe sends (each asked for)"
        );
        assert_eq!(
            estimate(&[planned(5, false)]),
            "1 task, ~5 requests, safe methods only"
        );
    }

    #[test]
    fn drafts_are_edited_then_approved_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let p = s
            .add(plan(
                "evilcorp-1",
                vec![planned(5, false), planned(9, true)],
            ))
            .unwrap();
        assert_eq!(p.id, "P-1");
        let p = s
            .edit(
                "evilcorp-1",
                "P-1",
                &Op::Budget {
                    index: 1,
                    budget: 3,
                },
                "t",
            )
            .unwrap();
        assert_eq!(p.tasks[1].task.constraints.budget, Some(3));
        assert!(s
            .edit("evilcorp-1", "P-1", &Op::Drop { index: 5 }, "t")
            .is_err());
        let p = s
            .edit("evilcorp-1", "P-1", &Op::Drop { index: 0 }, "t")
            .unwrap();
        assert_eq!(p.tasks.len(), 1);
        s.approved("evilcorp-1", "P-1", vec!["T-3".into()], "t")
            .unwrap();
        let e = s.edit("evilcorp-1", "P-1", &Op::Discard, "t").unwrap_err();
        assert!(e.to_string().contains("only a draft"), "{e}");
        let again = Store::open(dir.path()).unwrap();
        let p = again.find("P-1").unwrap();
        assert_eq!(
            (p.status, p.made.as_slice()),
            (Status::Approved, &["T-3".to_string()][..])
        );
        assert!(s.add(plan("../x", vec![])).is_err());
        assert_eq!(
            s.add(plan("other-2", vec![])).unwrap().id,
            "P-2",
            "ids are unique across engagements"
        );
    }
}
