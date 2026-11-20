//! taphttp's traffic log entry, as written to its `--log-file` (JSON lines).
//!
//! Mirrors `taphttp::storage::TrafficEntry`, schema version 2. taphttp
//! does not publish this type as a crate, so it is duplicated here; see
//! `docs/taphttp-requests.md`. Unknown fields are ignored so a newer
//! taphttp that adds fields does not break the console.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The taphttp traffic schema version this mirror was written against.
pub const TAPHTTP_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrafficEntry {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub host: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub req_headers: HashMap<String, String>,
    #[serde(default)]
    pub req_body: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub res_headers: HashMap<String, String>,
    #[serde(default)]
    pub res_body: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub replay_of: Option<String>,
}

pub fn parse_line(line: &str) -> Result<TrafficEntry, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line as taphttp 51eb9bc writes it.
    const TAPHTTP_LINE: &str = r#"{"id":"0b6c3e0e-5f53-4c39-9b7a-3f0f5c1a2d4e","ts":"2026-11-20T17:03:04.123456789Z","host":"example.com","method":"GET","url":"http://example.com/","req_headers":{"host":"example.com","user-agent":"curl/8.5.0","accept":"*/*"},"req_body":null,"status":200,"res_headers":{"content-type":"text/html"},"res_body":"<!doctype html>","duration_ms":42,"replay_of":null}"#;

    #[test]
    fn parses_a_taphttp_line() {
        let e = parse_line(TAPHTTP_LINE).unwrap();
        assert_eq!(e.id, "0b6c3e0e-5f53-4c39-9b7a-3f0f5c1a2d4e");
        assert_eq!(e.method, "GET");
        assert_eq!(e.host, "example.com");
        assert_eq!(e.url, "http://example.com/");
        assert_eq!(e.status, Some(200));
        assert_eq!(e.duration_ms, Some(42));
        assert_eq!(
            e.req_headers.get("user-agent").map(String::as_str),
            Some("curl/8.5.0")
        );
        assert_eq!(e.res_body.as_deref(), Some("<!doctype html>"));
        assert_eq!(e.replay_of, None);
    }

    #[test]
    fn schema_v1_line_without_replay_of_parses() {
        let v1 = r#"{"id":"a","ts":"2026-11-20T00:00:00Z","host":"h","method":"POST","url":"/x","req_headers":{},"req_body":"b","status":null,"res_headers":{},"res_body":null,"duration_ms":null}"#;
        let e = parse_line(v1).unwrap();
        assert_eq!(e.replay_of, None);
        assert_eq!(e.status, None);
        assert_eq!(e.req_body.as_deref(), Some("b"));
    }

    #[test]
    fn replay_of_is_carried_through() {
        let line = r#"{"id":"r","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/","req_headers":{},"res_headers":{},"replay_of":"orig"}"#;
        assert_eq!(parse_line(line).unwrap().replay_of.as_deref(), Some("orig"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let line = r#"{"id":"a","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/","req_headers":{},"res_headers":{},"something_new":[1,2,3]}"#;
        assert!(parse_line(line).is_ok());
    }

    #[test]
    fn malformed_lines_are_errors_not_panics() {
        for bad in [
            "",
            "not json",
            "{",
            "[]",
            r#"{"id":"a"}"#,
            r#"{"id":"a","ts":"yesterday","host":"h","method":"GET","url":"/"}"#,
            r#"{"id":1,"ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/"}"#,
            r#"{"id":"a","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/","status":70000}"#,
        ] {
            assert!(parse_line(bad).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn round_trips_through_json() {
        let e = parse_line(TAPHTTP_LINE).unwrap();
        let again = parse_line(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, again);
    }
}
