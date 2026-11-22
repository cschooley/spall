//! spall in lever-API mode against a stub taphttp API (no Docker).
//!
//! The stub follows taphttp v0.6.0's documented contract: bearer auth,
//! `GET /exchanges` newest first with a `before=<id>` cursor,
//! `GET /exchanges/{id}`, a `/feed` WebSocket that resumes with
//! `?since=<id>` and sends `reset` / `lagged` control frames, and
//! `POST /replay` (200 with the recorded replay, 502 with a recorded
//! failure, 400 for bad input).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path as FsPath;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

const TOKEN: &str = "stub-token-123";

fn entry(id: &str) -> String {
    format!(
        r#"{{"schema_version":3,"id":"{id}","ts":"2026-12-25T10:00:00Z","host":"example.com","method":"GET","url":"http://example.com/{id}","req_headers":[],"req_body":null,"req_body_encoding":"utf8","status":200,"res_headers":[],"res_body":null,"res_body_encoding":"utf8","duration_ms":1,"replay_of":null}}"#
    )
}

#[derive(Default)]
struct StubState {
    /// Recorded order, oldest first: (id, json).
    entries: Mutex<Vec<(String, String)>>,
    /// `since` value of each /feed connection, in order.
    feed_since: Mutex<Vec<Option<String>>>,
    /// Raw query string of each GET /exchanges.
    exchange_queries: Mutex<Vec<String>>,
    /// Close the current feed connection after its next live frame.
    drop_after_next: AtomicBool,
    /// Send `{"type":"reset"}` on the next feed connection.
    reset_next: AtomicBool,
    /// Send `{"type":"lagged"}` on the next feed connection, then close.
    lagged_next: AtomicBool,
    live: Mutex<Option<broadcast::Sender<String>>>,
    /// Body of each POST /replay, parsed.
    replays: Mutex<Vec<serde_json::Value>>,
}

#[derive(Clone)]
struct Stub(Arc<StubState>);

impl Stub {
    fn push(&self, id: &str) {
        let json = entry(id);
        self.0
            .entries
            .lock()
            .unwrap()
            .push((id.to_string(), json.clone()));
        if let Some(tx) = self.0.live.lock().unwrap().as_ref() {
            let _ = tx.send(json);
        }
    }
    fn feed_since(&self) -> Vec<Option<String>> {
        self.0.feed_since.lock().unwrap().clone()
    }
    fn exchange_queries(&self) -> Vec<String> {
        self.0.exchange_queries.lock().unwrap().clone()
    }
    fn replays(&self) -> Vec<serde_json::Value> {
        self.0.replays.lock().unwrap().clone()
    }
}

/// `POST /replay`: exchange "fails" gets a recorded failure (502), "bad"
/// a 400; anything else a recorded replay `r-<id>`.
async fn replay(State(s): State<Stub>, body: String) -> Response {
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    s.0.replays.lock().unwrap().push(v.clone());
    let id = v["id"].as_str().unwrap().to_string();
    let json = |status, extra: &str| {
        let e = entry(&format!("r-{id}")).replace(
            r#""replay_of":null"#,
            &format!(r#""replay_of":"{id}"{extra}"#),
        );
        (status, [(header::CONTENT_TYPE, "application/json")], e).into_response()
    };
    match id.as_str() {
        "fails" => json(
            StatusCode::BAD_GATEWAY,
            r#","error":"connect error: Connection refused""#,
        ),
        "bad" => (StatusCode::BAD_REQUEST, "invalid header value").into_response(),
        _ => json(StatusCode::OK, ""),
    }
}

async fn auth(req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"));
    if ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

async fn exchanges(
    State(s): State<Stub>,
    RawQuery(raw): RawQuery,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    s.0.exchange_queries
        .lock()
        .unwrap()
        .push(raw.unwrap_or_default());
    let entries = s.0.entries.lock().unwrap().clone();
    let mut newest_first: Vec<_> = entries.into_iter().rev().collect();
    if let Some(before) = q.get("before") {
        match newest_first.iter().position(|(id, _)| id == before) {
            Some(i) => newest_first = newest_first.split_off(i + 1),
            None => return (StatusCode::NOT_FOUND, "unknown id").into_response(),
        }
    }
    let limit: usize = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
    let body: Vec<String> = newest_first
        .into_iter()
        .take(limit)
        .map(|(_, j)| j)
        .collect();
    (
        [(header::CONTENT_TYPE, "application/json")],
        format!("[{}]", body.join(",")),
    )
        .into_response()
}

async fn exchange(State(s): State<Stub>, Path(id): Path<String>) -> Response {
    match s.0.entries.lock().unwrap().iter().find(|(i, _)| *i == id) {
        Some((_, j)) => ([(header::CONTENT_TYPE, "application/json")], j.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, format!("no exchange with id {id}")).into_response(),
    }
}

async fn feed(
    State(s): State<Stub>,
    Query(q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let since = q.get("since").cloned();
    s.0.feed_since.lock().unwrap().push(since.clone());
    let rx = s.0.live.lock().unwrap().as_ref().unwrap().subscribe();
    ws.on_upgrade(move |socket| pump(socket, s, since, rx))
}

async fn pump(
    mut ws: WebSocket,
    s: Stub,
    since: Option<String>,
    mut rx: broadcast::Receiver<String>,
) {
    if s.0.lagged_next.swap(false, Ordering::SeqCst) {
        let _ = ws
            .send(Message::Text(r#"{"type":"lagged","skipped":2}"#.into()))
            .await;
        return;
    }
    if let Some(since) = since {
        let entries = s.0.entries.lock().unwrap().clone();
        let pos = entries.iter().position(|(id, _)| *id == since);
        if s.0.reset_next.swap(false, Ordering::SeqCst) || pos.is_none() {
            let msg = format!(r#"{{"type":"reset","since":"{since}"}}"#);
            let _ = ws.send(Message::Text(msg.into())).await;
        } else if let Some(i) = pos {
            for (_, j) in &entries[i + 1..] {
                if ws.send(Message::Text(j.clone().into())).await.is_err() {
                    return;
                }
            }
        }
    }
    while let Ok(j) = rx.recv().await {
        if ws.send(Message::Text(j.into())).await.is_err() {
            return;
        }
        if s.0.drop_after_next.swap(false, Ordering::SeqCst) {
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
    }
}

async fn start_stub() -> (Stub, SocketAddr) {
    let state = Stub(Arc::new(StubState::default()));
    let (tx, _) = broadcast::channel(64);
    *state.0.live.lock().unwrap() = Some(tx);
    let app = Router::new()
        .route("/exchanges", get(exchanges))
        .route("/exchanges/{id}", get(exchange))
        .route("/feed", get(feed))
        .route("/replay", post(replay))
        .layer(middleware::from_fn(auth))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, addr)
}

fn write_token(path: &FsPath) {
    std::fs::write(path, format!("{TOKEN}\n")).unwrap();
}

async fn start_spall(api: Option<SocketAddr>, token_file: &FsPath) -> SocketAddr {
    start_spall_with(api, token_file, &[]).await
}

/// Like `start_spall`, with extra flags. The audit log goes next to the
/// token file (`audit.jsonl`) unless the flags say otherwise.
async fn start_spall_with(
    api: Option<SocketAddr>,
    token_file: &FsPath,
    extra: &[&str],
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut args = vec![
        "spall".to_string(),
        "--taphttp-token-file".into(),
        token_file.to_str().unwrap().into(),
        "--log-file".into(),
        token_file
            .with_file_name("unused.jsonl")
            .to_str()
            .unwrap()
            .into(),
    ];
    if let Some(api) = api {
        args.push("--taphttp-api".into());
        args.push(format!("http://{api}"));
    }
    if !extra.contains(&"--audit-log") {
        args.push("--audit-log".into());
        args.push(
            token_file
                .with_file_name("audit.jsonl")
                .to_str()
                .unwrap()
                .into(),
        );
    }
    args.extend(extra.iter().map(|s| s.to_string()));
    let config = spall::config::Config::try_parse_from(args).unwrap();
    tokio::spawn(spall::serve(config, listener));
    addr
}

async fn http_get(addr: SocketAddr, path: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    s
}

/// Read a whole (Connection: close) response.
async fn get_text(addr: SocketAddr, path: &str) -> String {
    let mut s = http_get(addr, path).await;
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_string(&mut out))
        .await
        .expect("timed out")
        .unwrap();
    out
}

/// Read the SSE stream until `needle` appears; returns everything read.
async fn sse_until(addr: SocketAddr, needle: &str) -> String {
    let mut s = http_get(addr, "/events").await;
    let mut seen = String::new();
    let res = tokio::time::timeout(Duration::from_secs(10), async {
        let mut buf = [0u8; 8192];
        while !seen.contains(needle) {
            let n = s.read(&mut buf).await.unwrap();
            assert!(n > 0, "closed before {needle:?}: {seen}");
            seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    })
    .await;
    assert!(res.is_ok(), "timed out waiting for {needle:?}; got {seen}");
    seen
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn ids_in_order(sse: &str) -> Vec<String> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("id: "))
        .map(String::from)
        .collect()
}

#[tokio::test]
async fn backfills_history_then_follows_live_feed() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    stub.push("e2");
    let spall = start_spall(Some(api), &token).await;

    let seen = sse_until(spall, "e2").await;
    assert_eq!(ids_in_order(&seen), vec!["e1", "e2"], "oldest first");
    eventually("feed connect", || !stub.feed_since().is_empty()).await;
    assert_eq!(
        stub.feed_since()[0].as_deref(),
        Some("e2"),
        "resumes after backfill"
    );

    stub.push("e3");
    let seen = sse_until(spall, "e3").await;
    assert_eq!(ids_in_order(&seen), vec!["e1", "e2", "e3"]);
}

#[tokio::test]
async fn reconnects_with_since_last_id_and_loses_nothing() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    let spall = start_spall(Some(api), &token).await;
    eventually("first feed connect", || stub.feed_since().len() == 1).await;
    assert_eq!(stub.feed_since()[0], None, "empty history: live from now");

    stub.0.drop_after_next.store(true, Ordering::SeqCst);
    stub.push("e1"); // delivered, then the stub closes the socket
    stub.push("e2"); // recorded while spall is disconnected
    eventually("reconnect", || stub.feed_since().len() >= 2).await;
    assert_eq!(stub.feed_since()[1].as_deref(), Some("e1"));

    let seen = sse_until(spall, "e2").await;
    assert_eq!(
        ids_in_order(&seen),
        vec!["e1", "e2"],
        "no gap, no duplicate"
    );
}

#[tokio::test]
async fn reset_resyncs_from_history_without_duplicates() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    stub.0.reset_next.store(true, Ordering::SeqCst);
    let spall = start_spall(Some(api), &token).await;

    // Backfill e1, connect since=e1, get reset, re-fetch history.
    eventually("re-sync after reset", || stub.exchange_queries().len() >= 2).await;
    stub.push("e2");
    let seen = sse_until(spall, "e2").await;
    assert_eq!(
        ids_in_order(&seen),
        vec!["e1", "e2"],
        "e1 not published twice"
    );
}

#[tokio::test]
async fn lagged_reconnects_from_last_id() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    stub.0.lagged_next.store(true, Ordering::SeqCst);
    let _spall = start_spall(Some(api), &token).await;
    eventually("reconnect after lagged", || stub.feed_since().len() >= 2).await;
    assert_eq!(stub.feed_since()[1].as_deref(), Some("e1"));
}

#[tokio::test]
async fn waits_for_token_file_to_appear() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let spall = start_spall(Some(api), &token).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(stub.feed_since().is_empty(), "nothing sent without a token");
    write_token(&token);
    sse_until(spall, "e1").await;
}

#[tokio::test]
async fn history_proxy_forwards_only_known_params_with_auth() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    for id in ["e1", "e2", "e3"] {
        stub.push(id);
    }
    let spall = start_spall(Some(api), &token).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;

    let res = get_text(
        spall,
        "/api/exchanges?limit=1&before=e3&q=ex+ample&token=evil&evil=1&host=example.com",
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(
        res.to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{res}"
    );
    assert!(
        res.contains(r#""id":"e2""#) && !res.contains(r#""id":"e3""#),
        "{res}"
    );
    let forwarded = stub.exchange_queries().last().unwrap().clone();
    assert!(!forwarded.contains("token"), "{forwarded}");
    assert!(!forwarded.contains("evil"), "{forwarded}");
    for want in ["limit=1", "before=e3", "q=ex+ample", "host=example.com"] {
        assert!(forwarded.contains(want), "{want} missing from {forwarded}");
    }

    let one = get_text(spall, "/api/exchanges/e1").await;
    assert!(
        one.starts_with("HTTP/1.1 200") && one.contains(r#""id":"e1""#),
        "{one}"
    );
    let missing = get_text(spall, "/api/exchanges/nope").await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    let bad = get_text(spall, "/api/exchanges/..%2Fexchanges").await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
}

#[tokio::test]
async fn status_reports_lever_source_and_connection() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let (stub, api) = start_stub().await;
    let spall = start_spall(Some(api), &token).await;
    let before = get_text(spall, "/api/status").await;
    assert!(before.contains(r#""source":"lever""#), "{before}");
    assert!(
        before.contains(r#""connected":false"#),
        "no token yet: {before}"
    );
    write_token(&token);
    eventually("feed connect", || !stub.feed_since().is_empty()).await;
    for _ in 0..100 {
        if get_text(spall, "/api/status")
            .await
            .contains(r#""connected":true"#)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("status never reported connected");
}

#[tokio::test]
async fn status_reports_log_source_without_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let spall = start_spall(None, &dir.path().join("api-token")).await;
    let res = get_text(spall, "/api/status").await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(res.contains(r#""source":"log""#), "{res}");
    assert!(res.contains(r#""connected":null"#), "{res}");
    assert!(
        res.contains(r#""proxy":"127.0.0.1:8080""#),
        "default proxy address: {res}"
    );
}

#[tokio::test]
async fn history_proxy_is_unavailable_without_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let spall = start_spall(None, &token).await;
    let res = get_text(spall, "/api/exchanges").await;
    assert!(res.starts_with("HTTP/1.1 503"), "{res}");
}

/// POST `body` to spall as its own page would (JSON, same origin), or with
/// the given headers instead.
async fn post_text(addr: SocketAddr, path: &str, body: &str, headers: Option<&str>) -> String {
    let headers = headers.unwrap_or(
        "Content-Type: application/json\r\nOrigin: http://127.0.0.1\r\nSec-Fetch-Site: same-origin\r\n",
    );
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_string(&mut out))
        .await
        .expect("timed out")
        .unwrap();
    out
}

fn body_json(res: &str) -> serde_json::Value {
    let (_, body) = res.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {res}"))
}

fn audit_records(token_file: &FsPath) -> Vec<serde_json::Value> {
    std::fs::read_to_string(token_file.with_file_name("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A stub with e1 (and the given ids), and spall following it with
/// `example.com` in scope.
async fn replay_setup(ids: &[&str], extra: &[&str]) -> (tempfile::TempDir, Stub, SocketAddr) {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    for id in ids {
        stub.push(id);
    }
    let mut flags = vec!["--scope", "example.com"];
    flags.extend_from_slice(extra);
    let spall = start_spall_with(Some(api), &token, &flags).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    (dir, stub, spall)
}

#[tokio::test]
async fn replay_in_scope_is_forwarded_with_spalls_attribution() {
    let (dir, stub, spall) = replay_setup(&[], &[]).await;
    let res = post_text(
        spall,
        "/api/replay",
        r#"{"id":"e1","meta":{"actor":"agent"},"evil":1}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let entry = body_json(&res);
    assert_eq!(entry["replay_of"], "e1");
    assert_eq!(entry["id"], "r-e1");
    assert_eq!(
        stub.replays(),
        vec![serde_json::json!({"id": "e1", "meta": {"actor": "human"}})],
        "token added by spall (the stub checks it); browser meta and unknown fields dropped"
    );
    let audit = audit_records(&dir.path().join("api-token"));
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["outcome"], "sent");
    assert_eq!(audit[0]["decision"], "allow");
    assert_eq!(audit[0]["result_id"], "r-e1");
    assert_eq!(audit[0]["target"], "http://example.com/e1");
    assert_eq!(audit[0]["upstream_status"], 200);
}

#[tokio::test]
async fn out_of_scope_replays_never_reach_taphttp() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    for scope in ["other.test", ""] {
        let spall = start_spall_with(Some(api), &token, &["--scope", scope]).await;
        eventually("backfill", || !stub.exchange_queries().is_empty()).await;
        let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
        assert!(res.starts_with("HTTP/1.1 403"), "{scope:?}: {res}");
        assert_eq!(body_json(&res)["decision"], "deny");
    }
    // A Host header edit to an out-of-scope virtual host is denied too.
    let spall = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    let res = post_text(
        spall,
        "/api/replay",
        r#"{"id":"e1","req_headers":[["host","admin.internal"]]}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    assert!(body_json(&res)["error"]
        .as_str()
        .unwrap()
        .contains("Host header"));

    assert!(stub.replays().is_empty(), "{:?}", stub.replays());
    let audit = audit_records(&token);
    assert_eq!(audit.len(), 3, "{audit:?}");
    assert!(audit.iter().all(|r| r["outcome"] == "denied"), "{audit:?}");
}

#[tokio::test]
async fn unsafe_methods_need_a_single_use_confirmation_for_the_same_request() {
    let (dir, stub, spall) = replay_setup(&[], &[]).await;
    let first = post_text(
        spall,
        "/api/replay",
        r#"{"id":"e1","method":"DELETE"}"#,
        None,
    )
    .await;
    assert!(first.starts_with("HTTP/1.1 428"), "{first}");
    let token = body_json(&first)["confirm"].as_str().unwrap().to_string();
    assert!(
        stub.replays().is_empty(),
        "nothing sent before confirmation"
    );

    // The token doesn't confirm a different request.
    let other = post_text(
        spall,
        "/api/replay",
        &format!(r#"{{"id":"e1","method":"PUT","confirm":"{token}"}}"#),
        None,
    )
    .await;
    assert!(other.starts_with("HTTP/1.1 409"), "{other}");

    let again = post_text(
        spall,
        "/api/replay",
        r#"{"id":"e1","method":"DELETE"}"#,
        None,
    )
    .await;
    let token = body_json(&again)["confirm"].as_str().unwrap().to_string();
    let confirmed = format!(r#"{{"id":"e1","method":"DELETE","confirm":"{token}"}}"#);
    let sent = post_text(spall, "/api/replay", &confirmed, None).await;
    assert!(sent.starts_with("HTTP/1.1 200"), "{sent}");
    let reused = post_text(spall, "/api/replay", &confirmed, None).await;
    assert!(reused.starts_with("HTTP/1.1 409"), "{reused}");

    assert_eq!(stub.replays().len(), 1, "sent exactly once");
    assert_eq!(stub.replays()[0]["method"], "DELETE");
    let outcomes: Vec<_> = audit_records(&dir.path().join("api-token"))
        .iter()
        .map(|r| {
            (
                r["outcome"].as_str().unwrap().to_string(),
                r["confirmed"].as_bool().unwrap(),
            )
        })
        .collect();
    let want = [
        ("needs_confirmation", false),
        ("confirmation_refused", false),
        ("needs_confirmation", false),
        ("sent", true),
        ("confirmation_refused", false),
    ];
    assert_eq!(
        outcomes,
        want.map(|(o, c)| (o.to_string(), c)).to_vec(),
        "one record per attempt"
    );
}

#[tokio::test]
async fn failed_bad_and_missing_replays_are_reported_and_audited() {
    let (dir, stub, spall) = replay_setup(&["fails", "bad"], &[]).await;
    let failed = post_text(spall, "/api/replay", r#"{"id":"fails"}"#, None).await;
    assert!(failed.starts_with("HTTP/1.1 502"), "{failed}");
    let entry = body_json(&failed);
    assert_eq!(entry["replay_of"], "fails", "the recorded failed attempt");
    assert_eq!(entry["error"], "connect error: Connection refused");

    let bad = post_text(spall, "/api/replay", r#"{"id":"bad"}"#, None).await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    assert_eq!(body_json(&bad)["error"], "invalid header value");

    let missing = post_text(spall, "/api/replay", r#"{"id":"nope"}"#, None).await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    let invalid = post_text(spall, "/api/replay", r#"{"id":"../x"}"#, None).await;
    assert!(invalid.starts_with("HTTP/1.1 400"), "{invalid}");
    let junk = post_text(spall, "/api/replay", "not json", None).await;
    assert!(junk.starts_with("HTTP/1.1 400"), "{junk}");

    assert_eq!(stub.replays().len(), 2, "fails and bad reached taphttp");
    let audit = audit_records(&dir.path().join("api-token"));
    let outcomes: Vec<_> = audit
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["failed", "error", "error"], "{audit:?}");
    assert_eq!(audit[0]["result_id"], "r-fails");
    assert_eq!(audit[2]["exchange_id"], "nope");
}

#[tokio::test]
async fn cross_site_replays_are_refused_before_anything_else() {
    let (dir, stub, spall) = replay_setup(&[], &[]).await;
    let lookups = stub.exchange_queries().len();
    for headers in [
        "Content-Type: text/plain\r\nOrigin: http://evil.example\r\n",
        "Content-Type: application/json\r\nSec-Fetch-Site: cross-site\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
    ] {
        let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, Some(headers)).await;
        assert!(res.starts_with("HTTP/1.1 403"), "{headers}: {res}");
    }
    assert!(stub.replays().is_empty());
    assert_eq!(stub.exchange_queries().len(), lookups);
    assert!(audit_records(&dir.path().join("api-token")).is_empty());
}

#[tokio::test]
async fn replay_needs_the_lever_api_and_status_says_so() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let spall = start_spall(None, &token).await;
    let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 503"), "{res}");
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["replay"], false);

    let (_dir, _stub, spall) = replay_setup(&[], &[]).await;
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["replay"], true);
    assert_eq!(status["scope"], serde_json::json!(["example.com"]));
}

#[tokio::test]
async fn an_unwritable_audit_log_stops_spall_in_api_mode() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (_stub, api) = start_stub().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let audit = dir.path().join("missing/audit.jsonl");
    let config = spall::config::Config::try_parse_from([
        "spall",
        "--taphttp-api",
        &format!("http://{api}"),
        "--taphttp-token-file",
        token.to_str().unwrap(),
        "--audit-log",
        audit.to_str().unwrap(),
    ])
    .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), spall::serve(config, listener))
        .await
        .expect("serve should fail fast");
    let err = format!("{:#}", r.unwrap_err());
    assert!(err.contains("audit log"), "{err}");
}

#[tokio::test]
async fn audit_endpoint_returns_the_newest_attempts() {
    let (_dir, _stub, spall) = replay_setup(&[], &[]).await;
    for body in [r#"{"id":"e1"}"#, r#"{"id":"e1","method":"DELETE"}"#] {
        post_text(spall, "/api/replay", body, None).await;
    }
    let res = get_text(spall, "/api/audit?last=1").await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let recs = body_json(&res);
    assert_eq!(recs.as_array().unwrap().len(), 1);
    assert_eq!(recs[0]["outcome"], "needs_confirmation");
    let all = body_json(&get_text(spall, "/api/audit").await);
    let outcomes: Vec<_> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].clone())
        .collect();
    assert_eq!(outcomes, ["sent", "needs_confirmation"]);
    let bad = get_text(spall, "/api/audit?last=nope").await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");

    let dir = tempfile::TempDir::new().unwrap();
    let log_mode = start_spall(None, &dir.path().join("api-token")).await;
    let res = get_text(log_mode, "/api/audit").await;
    assert!(res.starts_with("HTTP/1.1 503"), "{res}");
}

#[tokio::test]
async fn scope_changes_at_runtime_are_gated_audited_and_take_effect() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1"); // http://example.com/e1
    let spall = start_spall_with(Some(api), &token, &["--scope", "other.test"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let replay = |body: &'static str| post_text(spall, "/api/replay", body, None);

    let denied = replay(r#"{"id":"e1"}"#).await;
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    let reason = body_json(&denied)["error"].as_str().unwrap().to_string();
    assert!(
        reason.contains("scope: other.test") && reason.contains("scope all on"),
        "{reason}"
    );

    // A cross-site page can't change the scope.
    let forged = post_text(
        spall,
        "/api/scope",
        r#"{"op":"all","on":true}"#,
        Some("Content-Type: text/plain\r\nOrigin: http://evil.example\r\n"),
    )
    .await;
    assert!(forged.starts_with("HTTP/1.1 403"), "{forged}");

    let added = post_text(
        spall,
        "/api/scope",
        r#"{"op":"add","entry":"*.example.com"}"#,
        None,
    )
    .await;
    assert!(added.starts_with("HTTP/1.1 200"), "{added}");
    assert_eq!(
        body_json(&added)["scope"],
        serde_json::json!(["other.test", "*.example.com"])
    );
    assert!(
        replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 403"),
        "*. is subdomains only"
    );
    post_text(
        spall,
        "/api/scope",
        r#"{"op":"add","entry":"example.com"}"#,
        None,
    )
    .await;
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 200"));

    let removed = post_text(
        spall,
        "/api/scope",
        r#"{"op":"remove","entry":"example.com"}"#,
        None,
    )
    .await;
    assert_eq!(body_json(&removed)["changed"], true);
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 403"));

    let on = post_text(spall, "/api/scope", r#"{"op":"all","on":true}"#, None).await;
    assert_eq!(body_json(&on)["all"], true);
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["scope_all"], true);
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 200"));
    let unsafe_ = replay(r#"{"id":"e1","method":"DELETE"}"#).await;
    assert!(
        unsafe_.starts_with("HTTP/1.1 428"),
        "all hosts doesn't lift confirmation: {unsafe_}"
    );
    post_text(spall, "/api/scope", r#"{"op":"all","on":false}"#, None).await;
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 403"));

    for bad in [
        r#"{"op":"add","entry":"http://x/"}"#,
        r#"{"op":"add","entry":"*"}"#,
        r#"{"op":"nope"}"#,
        r#"{"op":"all"}"#,
        "junk",
    ] {
        let res = post_text(spall, "/api/scope", bad, None).await;
        assert!(res.starts_with("HTTP/1.1 400"), "{bad}: {res}");
    }

    let audit = audit_records(&token);
    let scope_changes: Vec<_> = audit
        .iter()
        .filter(|r| r["action"] == "scope")
        .map(|r| r["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        scope_changes,
        [
            "add *.example.com",
            "add example.com",
            "remove example.com",
            "all hosts on",
            "all hosts off"
        ]
    );
    let under_all: Vec<_> = audit
        .iter()
        .filter(|r| r["action"] == "replay" && r["scope_all"] == true)
        .map(|r| r["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        under_all,
        ["sent", "needs_confirmation"],
        "replays under all-hosts are marked"
    );
}

#[tokio::test]
async fn star_in_spall_scope_allows_every_host_from_the_start() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let spall = start_spall_with(Some(api), &token, &["--scope", "*"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["scope_all"], true);
    let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
}
