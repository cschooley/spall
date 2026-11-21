//! End to end: real TCP listener, real file on disk, no Docker.
//! Starts spall against a log file that does not exist yet, then plays
//! the part of taphttp by appending lines to it.

use clap::Parser;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const LINE_A: &str = r#"{"id":"e2e-aaaa","ts":"2026-11-20T17:00:00Z","host":"example.com","method":"GET","url":"http://example.com/","req_headers":{"host":"example.com"},"req_body":null,"status":200,"res_headers":{"content-type":"text/html"},"res_body":"<h1>hi</h1>","duration_ms":12,"replay_of":null}"#;
/// v3 (taphttp v0.2.0): repeated Set-Cookie, binary body as base64.
const LINE_B: &str = r#"{"schema_version":3,"id":"e2e-bbbb","ts":"2026-11-20T17:00:01Z","host":"api.example.com","method":"POST","url":"https://api.example.com/login","req_headers":[],"req_body":"{\"u\":\"x\"}","req_body_encoding":"utf8","status":401,"res_headers":[["set-cookie","a=1"],["set-cookie","b=2"]],"res_body":"AAEC/w==","res_body_encoding":"base64","duration_ms":30,"replay_of":null}"#;

fn append(path: &Path, s: &str) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(s.as_bytes()).unwrap();
    f.flush().unwrap();
}

async fn start(log: &Path) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = spall::config::Config::try_parse_from([
        "spall",
        "--log-file",
        log.to_str().unwrap(),
        "--poll-ms",
        "20",
    ])
    .unwrap();
    tokio::spawn(spall::serve(config, listener));
    addr
}

async fn open(addr: SocketAddr, path: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAccept: */*\r\n\r\n",
        addr.port()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    s
}

async fn read_until(s: &mut TcpStream, seen: &mut String, needle: &str) {
    let res = tokio::time::timeout(Duration::from_secs(10), async {
        let mut buf = [0u8; 8192];
        while !seen.contains(needle) {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "connection closed before {needle:?}; got {seen:?}");
            seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    })
    .await;
    assert!(
        res.is_ok(),
        "timed out waiting for {needle:?}; got {seen:?}"
    );
}

#[tokio::test]
async fn appended_log_lines_stream_to_sse_clients() {
    let dir = tempfile::TempDir::new().unwrap();
    let log = dir.path().join("traffic.jsonl");
    let addr = start(&log).await;

    // Healthy even before the log file exists.
    let mut h = open(addr, "/healthz").await;
    let mut seen = String::new();
    read_until(&mut h, &mut seen, "ok").await;
    assert!(seen.starts_with("HTTP/1.1 200"), "{seen}");

    let mut sse = open(addr, "/events").await;
    let mut seen = String::new();
    read_until(&mut sse, &mut seen, "\r\n\r\n").await;
    assert!(seen.starts_with("HTTP/1.1 200"), "{seen}");
    assert!(
        seen.to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "{seen}"
    );

    // taphttp creates the file; a malformed line must not stop the tailer;
    // a line split across writes must be delivered once it is complete.
    append(&log, "this is not json\n");
    append(&log, LINE_A);
    tokio::time::sleep(Duration::from_millis(100)).await;
    append(&log, "\n");
    read_until(&mut sse, &mut seen, "e2e-aaaa").await;

    append(&log, &format!("{LINE_B}\n"));
    read_until(&mut sse, &mut seen, "e2e-bbbb").await;
    assert!(seen.contains("id: e2e-aaaa"));
    assert!(seen.contains("\"status\":401"));
    assert!(seen.contains(r#""res_headers":[["set-cookie","a=1"],["set-cookie","b=2"]]"#));
    assert!(seen.contains(r#""res_body":"AAEC/w==","res_body_encoding":"base64""#));
    assert!(!seen.contains("this is not json"));

    // A browser connecting later gets the backlog.
    let mut late = open(addr, "/events").await;
    let mut late_seen = String::new();
    read_until(&mut late, &mut late_seen, "e2e-bbbb").await;
    assert!(late_seen.contains("e2e-aaaa"));
}

#[tokio::test]
async fn console_page_is_served() {
    let dir = tempfile::TempDir::new().unwrap();
    let addr = start(&dir.path().join("missing.jsonl")).await;
    let mut s = open(addr, "/").await;
    let mut seen = String::new();
    read_until(&mut s, &mut seen, "</html>").await;
    assert!(seen.starts_with("HTTP/1.1 200"));
    assert!(seen.contains("/events") || seen.contains("/app.js"));
}
