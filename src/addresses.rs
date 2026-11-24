//! Where a host resolved (#65): the first addresses spall saw for each host
//! a finding was made on, per engagement, so a later retest can flag a host
//! that now resolves somewhere else (new IPs, a CDN, a new owner) before
//! anything is sent.
//!
//! This is spall's own lookup, made when a candidate is flagged or
//! proposed: a resolver's view, not the address taphttp connected to.
//! taphttp#24 asks taphttp to record that peer on every exchange; when it
//! does, the evidence's peers replace this.
//!
//! Stored at `<engagements>/addresses/<engagement>.json` (0600).

use crate::engagement::{create_dir_private, write_atomic};
use crate::model::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Looks a host name up. The system resolver in spall; a fake in tests.
pub trait Resolve: Send + Sync {
    /// The host's addresses, sorted; empty if it doesn't resolve.
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Vec<String>>;
}

/// The system resolver, with a short timeout.
pub struct System;

impl Resolve for System {
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            // An IP literal is its own answer.
            if host.parse::<std::net::IpAddr>().is_ok() {
                return vec![host.to_string()];
            }
            let found =
                tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host((host, 443)))
                    .await;
            let mut ips: Vec<String> = match found {
                Ok(Ok(addrs)) => addrs.map(|a| a.ip().to_string()).collect(),
                _ => Vec::new(),
            };
            ips.sort();
            ips.dedup();
            ips
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub ips: Vec<String>,
    pub at: String,
}

/// What a check found for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    Same,
    /// Resolves elsewhere now: was, now.
    Elsewhere(Vec<String>, Vec<String>),
    /// Doesn't resolve now.
    Gone(Vec<String>),
    /// No record of where it resolved before.
    Unknown,
}

/// Compare a host's first addresses with where it resolves `now`. Any
/// shared address counts as the same place (round-robin DNS).
pub fn compare(before: Option<&Seen>, now: &[String]) -> Moved {
    match before {
        None => Moved::Unknown,
        Some(b) if now.is_empty() => Moved::Gone(b.ips.clone()),
        Some(b) if b.ips.iter().any(|ip| now.contains(ip)) => Moved::Same,
        Some(b) => Moved::Elsewhere(b.ips.clone(), now.to_vec()),
    }
}

pub struct Store {
    dir: PathBuf,
    by_eng: Mutex<HashMap<String, BTreeMap<String, Seen>>>,
}

fn valid(id: &str) -> bool {
    (1..=48).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Store {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        create_dir_private(dir)?;
        let mut by_eng = HashMap::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(eng) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let map: BTreeMap<String, Seen> = serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            by_eng.insert(eng.to_string(), map);
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            by_eng: Mutex::new(by_eng),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, BTreeMap<String, Seen>>> {
        self.by_eng.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Keep `ips` as `host`'s first addresses in `eng`, unless it has some
    /// already (the first sight is the baseline) or there are none.
    pub fn first(&self, eng: &str, host: &str, ips: Vec<String>, at: &str) {
        if ips.is_empty() || !valid(eng) {
            return;
        }
        let mut all = self.lock();
        let map = all.entry(eng.to_string()).or_default();
        if map.contains_key(host) || map.len() >= 10_000 {
            return;
        }
        map.insert(host.to_string(), Seen { ips, at: at.into() });
        let saved = serde_json::to_vec(map)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.dir.join(format!("{eng}.json")), &b));
        if let Err(e) = saved {
            tracing::warn!("saving {eng}'s addresses: {e:#}");
        }
    }

    pub fn get(&self, eng: &str, host: &str) -> Option<Seen> {
        self.lock().get(eng)?.get(host).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(ips: &[&str]) -> Seen {
        Seen {
            ips: ips.iter().map(|s| s.to_string()).collect(),
            at: "t".into(),
        }
    }

    #[test]
    fn a_host_is_the_same_while_it_shares_an_address() {
        let now = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            compare(Some(&seen(&["10.0.0.1", "10.0.0.2"])), &now(&["10.0.0.2"])),
            Moved::Same
        );
        assert!(matches!(
            compare(Some(&seen(&["10.0.0.1"])), &now(&["203.0.113.9"])),
            Moved::Elsewhere(..)
        ));
        assert!(matches!(
            compare(Some(&seen(&["10.0.0.1"])), &[]),
            Moved::Gone(_)
        ));
        assert_eq!(compare(None, &now(&["10.0.0.1"])), Moved::Unknown);
    }

    #[test]
    fn the_first_sight_is_kept_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        s.first("eng-1", "app.test", vec!["10.0.0.1".into()], "t1");
        s.first("eng-1", "app.test", vec!["10.9.9.9".into()], "t2");
        s.first("eng-1", "none.test", vec![], "t3");
        let again = Store::open(dir.path()).unwrap();
        assert_eq!(
            again.get("eng-1", "app.test"),
            Some(Seen {
                ips: vec!["10.0.0.1".into()],
                at: "t1".into()
            })
        );
        assert_eq!(again.get("eng-1", "none.test"), None);
        s.first("../x", "a", vec!["1.1.1.1".into()], "t");
        assert!(again.get("../x", "a").is_none());
    }

    #[tokio::test]
    async fn an_ip_literal_resolves_to_itself() {
        assert_eq!(
            System.lookup("203.0.113.7").await,
            vec!["203.0.113.7".to_string()]
        );
    }
}
