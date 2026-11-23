//! Audit log (#22): one JSON line per attempted action against a target,
//! including denials, unconfirmed attempts and failures. Append-only;
//! spall never rewrites or truncates it. M6 adds agent actors and task
//! ids to the same record.
//!
//! The file is opened at startup, so an unwritable audit log stops spall
//! instead of letting actions go unrecorded.

use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What happened to an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// taphttp sent it and recorded the result.
    Sent,
    /// taphttp tried and the target didn't answer (recorded with `error`).
    Failed,
    /// The gate refused it.
    Denied,
    /// The gate wants confirmation; a token was issued, nothing was sent.
    NeedsConfirmation,
    /// A confirmation was given but refused (unknown, expired, mismatched).
    ConfirmationRefused,
    /// Bad input, or taphttp unreachable or refusing; nothing was sent.
    Error,
    /// The scope was changed at runtime (`scope add|exclude|remove|all`).
    ScopeChanged,
    /// Allowed, but over the rules' rate ceiling; nothing was sent.
    RateLimited,
    /// The engagement's details changed, or another one became active.
    EngagementChanged,
    /// A candidate was flagged, triaged or edited (#42).
    CandidateChanged,
    /// Promoted findings were exported, or an export was refused (#51).
    Exported,
    /// A task was made, approved, edited or halted, or that was refused (#54).
    TaskChanged,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub ts: String,
    pub actor: String,
    /// The active engagement's id when this happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engagement: Option<String>,
    pub action: &'static str,
    /// The exchange acted on (the original, for a replay).
    pub exchange_id: String,
    pub method: Option<String>,
    pub target: Option<String>,
    pub host_header: Option<String>,
    pub decision: Option<&'static str>,
    pub reason: Option<String>,
    pub confirmed: bool,
    /// The scope allowed every host when this was decided.
    pub scope_all: bool,
    /// For a scope change: the entries before and after (`Scope::entries`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_before: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_after: Option<Vec<String>>,
    /// For an engagement change: `{field: {before, after}}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<serde_json::Value>,
    pub outcome: Outcome,
    /// taphttp's HTTP status for the forwarded call, if it was made.
    pub upstream_status: Option<u16>,
    /// The exchange taphttp recorded (the replay), if any.
    pub result_id: Option<String>,
    pub error: Option<String>,
}

impl Record {
    /// A record for `action` on `exchange_id` by the human tester, with
    /// everything else empty; fill in what's known.
    pub fn new(action: &'static str, exchange_id: &str, outcome: Outcome) -> Self {
        Record {
            ts: taphttp_types::Timestamp::now().to_string(),
            actor: "human".into(),
            engagement: None,
            action,
            exchange_id: exchange_id.into(),
            method: None,
            target: None,
            host_header: None,
            decision: None,
            reason: None,
            confirmed: false,
            scope_all: false,
            scope_before: None,
            scope_after: None,
            changes: None,
            outcome,
            upstream_status: None,
            result_id: None,
            error: None,
        }
    }
}

pub struct AuditLog {
    path: PathBuf,
    file: Mutex<File>,
}

impl AuditLog {
    /// Open (creating if needed, mode 0600) for appending.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path)?;
        Ok(AuditLog {
            path: path.to_path_buf(),
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The newest `n` records, oldest first, for the console's `audit`.
    /// Lines that don't parse (a torn write, a foreign line) are skipped.
    /// Reads the whole file: fine for an engagement's worth of attempts.
    pub fn tail(&self, n: usize) -> std::io::Result<Vec<serde_json::Value>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let _guard = self.file.lock().unwrap_or_else(|p| p.into_inner());
        let text = std::fs::read_to_string(&self.path)?;
        let mut out: Vec<serde_json::Value> = text
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str(l).ok())
            .take(n)
            .collect();
        out.reverse();
        Ok(out)
    }

    /// Append one record as a single write, so lines never interleave.
    pub fn record(&self, rec: &Record) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(rec)?;
        line.push(b'\n');
        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        f.write_all(&line)?;
        f.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_append_as_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, "{\"earlier\":true}\n").unwrap();
        let log = AuditLog::open(&path).unwrap();
        let mut r = Record::new("replay", "abc", Outcome::Denied);
        r.reason = Some("cdn.other.test is not in scope".into());
        r.decision = Some("deny");
        log.record(&r).unwrap();
        log.record(&Record::new("replay", "def", Outcome::Sent))
            .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "appends, never truncates");
        assert_eq!(lines[1]["exchange_id"], "abc");
        assert_eq!(lines[1]["outcome"], "denied");
        assert_eq!(lines[1]["decision"], "deny");
        assert_eq!(lines[1]["actor"], "human");
        assert_eq!(lines[1]["action"], "replay");
        assert_eq!(lines[2]["outcome"], "sent");
        assert!(lines[2]["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[cfg(unix)]
    #[test]
    fn a_new_audit_log_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        AuditLog::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn tail_returns_the_newest_records_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::open(&path).unwrap();
        assert!(log.tail(5).unwrap().is_empty());
        for id in ["a", "b", "c"] {
            log.record(&Record::new("replay", id, Outcome::Sent))
                .unwrap();
        }
        // A torn or foreign line is skipped, not fatal.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let ids = |v: Vec<serde_json::Value>| -> Vec<String> {
            v.iter()
                .map(|r| r["exchange_id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids(log.tail(2).unwrap()), ["b", "c"]);
        assert_eq!(ids(log.tail(10).unwrap()), ["a", "b", "c"]);
        assert!(log.tail(0).unwrap().is_empty());
    }

    #[test]
    fn an_unwritable_location_fails_at_open() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AuditLog::open(&dir.path().join("missing/audit.jsonl")).is_err());
    }
}
