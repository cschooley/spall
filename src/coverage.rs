//! Coverage (#94): which endpoint was tested for which WSTG category, by
//! whom and how. Recorded by `sweep` (each path it probed, under its
//! check's rule), by an agent's `record_coverage` (with an exchange it
//! sent or read), and by the tester marking a cell. Candidates add "found"
//! on top (`surface::view`). Stored per engagement at
//! `<engagements>/coverage/<engagement>.json` (0600).

use crate::engagement::{create_dir_private, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_PER_ENGAGEMENT: usize = 50_000;
pub const MAX_NOTE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The endpoint (`surface::key`).
    pub key: String,
    /// A WSTG category code (`surface::CATEGORIES`).
    pub category: String,
    /// `human` or `agent:T-n`.
    pub by: String,
    pub at: String,
    /// `sweep <check>`, `agent`, `marked`.
    pub how: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

pub struct Store {
    dir: PathBuf,
    books: Mutex<HashMap<String, Vec<Record>>>,
}

fn valid(id: &str) -> bool {
    (1..=48).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Store {
    pub fn open(dir: &Path) -> anyhow::Result<Store> {
        create_dir_private(dir)?;
        let mut books = HashMap::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let p = entry.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(eng) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let list: Vec<Record> = serde_json::from_slice(&std::fs::read(&p)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            books.insert(eng.to_string(), list);
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            books: Mutex::new(books),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Record>>> {
        self.books.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Record one; the same endpoint, category, author and way replaces the
    /// earlier record (the newest time and exchange win).
    pub fn add(&self, eng: &str, mut r: Record) -> Result<(), String> {
        if !valid(eng) {
            return Err(format!("not an engagement id: {eng:?}"));
        }
        if crate::surface::CATEGORIES
            .iter()
            .all(|(c, _)| *c != r.category)
        {
            let codes: Vec<&str> = crate::surface::CATEGORIES.iter().map(|(c, _)| *c).collect();
            return Err(format!("category: one of {}", codes.join(", ")));
        }
        if r.key.len() > 2048 || r.key.split('|').count() != 3 {
            return Err("not an endpoint".into());
        }
        r.note = r
            .note
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NOTE)
            .collect();
        let mut all = self.lock();
        let list = all.entry(eng.to_string()).or_default();
        list.retain(|x| {
            !(x.key == r.key && x.category == r.category && x.by == r.by && x.how == r.how)
        });
        if list.len() >= MAX_PER_ENGAGEMENT {
            list.remove(0);
        }
        list.push(r);
        let saved = serde_json::to_vec(list)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.dir.join(format!("{eng}.json")), &b));
        if let Err(e) = saved {
            tracing::warn!("saving {eng}'s coverage: {e:#}");
        }
        Ok(())
    }

    pub fn list(&self, eng: &str) -> Vec<Record> {
        self.lock().get(eng).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(by: &str, how: &str, at: &str) -> Record {
        Record {
            key: "app.test|GET|/orders/{}".into(),
            category: "ATHZ".into(),
            by: by.into(),
            at: at.into(),
            how: how.into(),
            exchange: None,
            note: String::new(),
        }
    }

    #[test]
    fn records_are_kept_replaced_by_the_same_way_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        s.add("eng-1", rec("human", "marked", "t1")).unwrap();
        s.add("eng-1", rec("human", "marked", "t2")).unwrap();
        s.add("eng-1", rec("agent:T-2", "agent", "t3")).unwrap();
        let again = Store::open(dir.path()).unwrap();
        let l = again.list("eng-1");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].at, "t2");
        let mut bad = rec("human", "marked", "t");
        bad.category = "XSS".into();
        assert!(s.add("eng-1", bad).unwrap_err().contains("ATHZ"));
        let mut bad = rec("human", "marked", "t");
        bad.key = "nope".into();
        assert!(s.add("eng-1", bad).is_err());
        assert!(s.add("../x", rec("human", "marked", "t")).is_err());
    }
}
