//! Hosts seen in traffic (#39): every host spall sees (the URL host and,
//! when different, the Host header) with a count and first/last seen, so
//! the tester can decide each one: in scope, excluded, or still unknown.
//! The state comes from the scope engine (`Scope::explain`), never stored
//! here, so a scope change shows up at once.
//!
//! Fed by the hub: taphttp's backfill (the most recent `--history`
//! exchanges) and the live feed, or the log tailer. Older history needs
//! taphttp to aggregate (cschooley/taphttp#21).

use crate::gate::{HostState, Scope};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use taphttp_types::TrafficEntry;

/// Most distinct hosts kept; past this, new hosts are counted as dropped.
pub const MAX_HOSTS: usize = 10_000;

#[derive(Debug, Clone)]
struct Stat {
    count: u64,
    first: String,
    last: String,
}

#[derive(Debug, Default)]
struct Inner {
    hosts: HashMap<String, Stat>,
    dropped: u64,
}

#[derive(Debug, Default)]
pub struct Hosts {
    inner: Mutex<Inner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostRow {
    pub host: String,
    pub count: u64,
    pub first_seen: String,
    pub last_seen: String,
    pub state: HostState,
    /// The scope entry that decided it.
    pub by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub hosts: Vec<HostRow>,
    pub counts: Counts,
    /// Distinct hosts not kept because the table was full.
    pub dropped: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    #[serde(rename = "in")]
    pub in_: usize,
    pub out: usize,
    pub unknown: usize,
}

impl Hosts {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Count an exchange's hosts. Re-seeing an exchange (a resync) counts
    /// it again: counts are "seen", not "distinct exchanges".
    pub fn observe(&self, e: &TrafficEntry) {
        let header = e
            .req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("host"))
            .map(|(_, v)| v.to_string());
        let mut names = vec![Scope::default().explain(&e.host).host];
        if let Some(h) = header {
            let h = Scope::default().explain(&h).host;
            if !names.contains(&h) {
                names.push(h);
            }
        }
        let ts = e.ts.to_string();
        let mut inner = self.lock();
        for name in names {
            if name.is_empty() || name.len() > 255 {
                continue;
            }
            if let Some(s) = inner.hosts.get_mut(&name) {
                s.count += 1;
                if ts > s.last {
                    s.last.clone_from(&ts);
                }
                if ts < s.first {
                    s.first.clone_from(&ts);
                }
            } else if inner.hosts.len() < MAX_HOSTS {
                inner.hosts.insert(
                    name,
                    Stat {
                        count: 1,
                        first: ts.clone(),
                        last: ts.clone(),
                    },
                );
            } else {
                inner.dropped += 1;
            }
        }
    }

    /// Every host with its state under `scope`, most seen first.
    pub fn list(&self, scope: &Scope) -> Listing {
        let inner = self.lock();
        let mut counts = Counts::default();
        let mut hosts: Vec<HostRow> = inner
            .hosts
            .iter()
            .map(|(h, s)| {
                let v = scope.explain(h);
                match v.state {
                    HostState::In => counts.in_ += 1,
                    HostState::Out => counts.out += 1,
                    HostState::Unknown => counts.unknown += 1,
                }
                HostRow {
                    host: h.clone(),
                    count: s.count,
                    first_seen: s.first.clone(),
                    last_seen: s.last.clone(),
                    state: v.state,
                    by: v.by,
                }
            })
            .collect();
        hosts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.host.cmp(&b.host)));
        Listing {
            hosts,
            counts,
            dropped: inner.dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(host: &str, header: Option<&str>, ts: &str) -> TrafficEntry {
        let mut v = serde_json::json!({ "id": "x", "ts": ts, "host": host, "method": "GET", "url": format!("http://{host}/") });
        if let Some(h) = header {
            v["req_headers"] = serde_json::json!([["Host", h]]);
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn hosts_are_normalized_counted_and_stated_by_the_scope() {
        let h = Hosts::default();
        h.observe(&entry("App.Acme.test:8443", None, "2026-11-22T10:00:00Z"));
        h.observe(&entry(
            "app.acme.test",
            Some("app.acme.test"),
            "2026-11-22T09:00:00Z",
        ));
        h.observe(&entry(
            "app.acme.test",
            Some("admin.internal"),
            "2026-11-22T11:00:00Z",
        ));
        h.observe(&entry(
            "www.google-analytics.com",
            None,
            "2026-11-22T10:30:00Z",
        ));
        h.observe(&entry("pay.acme.test", None, "2026-11-22T10:31:00Z"));
        let scope = Scope::parse("*.acme.test, acme.test, !pay.acme.test").unwrap();
        let l = h.list(&scope);
        let rows: Vec<_> = l
            .hosts
            .iter()
            .map(|r| (r.host.as_str(), r.count, r.state))
            .collect();
        assert_eq!(
            rows,
            [
                ("app.acme.test", 3, HostState::In),
                ("admin.internal", 1, HostState::Unknown),
                ("pay.acme.test", 1, HostState::Out),
                ("www.google-analytics.com", 1, HostState::Unknown),
            ]
        );
        let app = &l.hosts[0];
        assert_eq!(
            (app.first_seen.as_str(), app.last_seen.as_str()),
            ("2026-11-22T09:00:00Z", "2026-11-22T11:00:00Z")
        );
        assert_eq!(app.by.as_deref(), Some("*.acme.test"));
        assert_eq!((l.counts.in_, l.counts.out, l.counts.unknown), (1, 1, 2));
        // A scope change is reflected at once: nothing is cached.
        let l = h.list(&Scope::parse("*.acme.test, *.google-analytics.com").unwrap());
        assert_eq!(l.counts.unknown, 1);
    }

    #[test]
    fn the_table_is_bounded() {
        let h = Hosts::default();
        for i in 0..MAX_HOSTS + 5 {
            h.observe(&entry(&format!("h{i}.test"), None, "2026-11-22T10:00:00Z"));
        }
        let l = h.list(&Scope::default());
        assert_eq!(l.hosts.len(), MAX_HOSTS);
        assert_eq!(l.dropped, 5);
        h.observe(&entry("h1.test", None, "2026-11-22T10:00:00Z"));
        assert_eq!(
            h.list(&Scope::default())
                .hosts
                .iter()
                .find(|r| r.host == "h1.test")
                .unwrap()
                .count,
            2,
            "known hosts still count"
        );
    }
}
