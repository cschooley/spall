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
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub ts: String,
    pub actor: String,
    pub action: &'static str,
    /// The exchange acted on (the original, for a replay).
    pub exchange_id: String,
    pub method: Option<String>,
    pub target: Option<String>,
    pub host_header: Option<String>,
    pub decision: Option<&'static str>,
    pub reason: Option<String>,
    pub confirmed: bool,
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
            action,
            exchange_id: exchange_id.into(),
            method: None,
            target: None,
            host_header: None,
            decision: None,
            reason: None,
            confirmed: false,
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
    fn an_unwritable_location_fails_at_open() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AuditLog::open(&dir.path().join("missing/audit.jsonl")).is_err());
    }
}
