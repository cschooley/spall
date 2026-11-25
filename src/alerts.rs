//! Tester alerts (#86): something needs you, or something you started
//! finished. A task waiting on approval sits still until someone looks; an
//! OOB callback (#1) can come hours after its request.
//!
//! **Content-free by construction**, like the quad card: an alert is a kind,
//! ids that match spall's own id shapes (`T-n`, `I-n`, an engagement id),
//! and a time. No field can carry a host, path, payload, finding text or
//! client name, so nothing an alert is shown in (a desktop notification, a
//! webhook later) can leak an engagement's attack surface. The words are
//! the UI's, built from the kind.
//!
//! Kept in memory, newest last, at most [`KEEP`]; the UI asks for those
//! after the last `seq` it saw.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

pub const KEEP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A task paused for your approval.
    Waiting,
    /// A task finished.
    Done,
    Failed,
    Halted,
    /// An out-of-band callback came in (#1).
    OobHit,
    /// Quad wore off or was ended (#66).
    QuadOver,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alert {
    pub seq: u64,
    pub at: String,
    pub kind: Kind,
    pub engagement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// `I-n` for an OOB hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
}

/// `T-n`, `I-n`: a prefix, a dash, digits.
fn id_like(s: &str, prefix: char) -> bool {
    let mut c = s.chars();
    c.next() == Some(prefix)
        && c.next() == Some('-')
        && (2..=12).contains(&s.len())
        && s[2..].bytes().all(|b| b.is_ascii_digit())
}

/// Engagement ids: lowercase letters, digits, `-` (engagement.rs).
fn engagement_like(s: &str) -> bool {
    (1..=48).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Default)]
struct Inner {
    next: u64,
    list: VecDeque<Alert>,
}

#[derive(Default)]
pub struct Alerts {
    inner: Mutex<Inner>,
}

impl Alerts {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Raise an alert. Anything not shaped like spall's own ids is dropped
    /// rather than passed on: an alert carries no free text.
    pub fn raise(&self, kind: Kind, engagement: &str, task: Option<&str>, item: Option<&str>) {
        if !engagement_like(engagement) {
            return;
        }
        let task = task.filter(|t| id_like(t, 'T')).map(String::from);
        let item = item.filter(|i| id_like(i, 'I')).map(String::from);
        let mut g = self.lock();
        g.next += 1;
        let a = Alert {
            seq: g.next,
            at: taphttp_types::Timestamp::now().to_string(),
            kind,
            engagement: engagement.to_string(),
            task,
            item,
        };
        if g.list.len() >= KEEP {
            g.list.pop_front();
        }
        g.list.push_back(a);
    }

    /// Alerts after `seq`, oldest first.
    pub fn after(&self, seq: u64) -> Vec<Alert> {
        self.lock()
            .list
            .iter()
            .filter(|a| a.seq > seq)
            .cloned()
            .collect()
    }

    /// The newest seq (0 if none yet).
    pub fn latest(&self) -> u64 {
        self.lock().next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_carry_ids_and_nothing_else() {
        let a = Alerts::default();
        a.raise(Kind::Waiting, "evilcorp-1a2b", Some("T-3"), None);
        a.raise(Kind::OobHit, "evilcorp-1a2b", Some("T-3"), Some("I-12"));
        // Free text where an id goes is dropped, not passed on.
        a.raise(
            Kind::Done,
            "evilcorp-1a2b",
            Some("T-3 <script>secret.evilcorp.test"),
            Some("/admin"),
        );
        a.raise(
            Kind::Done,
            "Evil Corp: secret.evilcorp.test",
            Some("T-4"),
            None,
        );
        let all = a.after(0);
        assert_eq!(all.len(), 3);
        assert_eq!(
            (all[2].task.as_deref(), all[2].item.as_deref()),
            (None, None)
        );
        let json = serde_json::to_string(&all).unwrap();
        for leak in ["secret", "script", "/admin", "Evil"] {
            assert!(!json.contains(leak), "{leak} in {json}");
        }
        assert_eq!(a.after(2).len(), 1);
        assert_eq!(a.latest(), 3);
    }

    #[test]
    fn only_the_newest_are_kept() {
        let a = Alerts::default();
        for _ in 0..KEEP + 5 {
            a.raise(Kind::Done, "e", Some("T-1"), None);
        }
        let all = a.after(0);
        assert_eq!((all.len(), all[0].seq), (KEEP, 6));
    }
}
