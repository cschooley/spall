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
    if !extra.contains(&"--engagements") {
        args.push("--engagements".into());
        args.push(
            token_file
                .with_file_name("engagements")
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

async fn eventually_async<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if f().await {
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
    send_text(addr, "POST", path, body, headers).await
}

async fn send_text(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    headers: Option<&str>,
) -> String {
    let headers = headers.unwrap_or(
        "Content-Type: application/json\r\nOrigin: http://127.0.0.1\r\nSec-Fetch-Site: same-origin\r\n",
    );
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    for (i, scope) in ["other.test", ""].into_iter().enumerate() {
        // Each run gets its own engagement store: --scope only seeds a new one.
        let store = dir.path().join(format!("engagements-{i}"));
        let flags = ["--scope", scope, "--engagements", store.to_str().unwrap()];
        let spall = start_spall_with(Some(api), &token, &flags).await;
        eventually("backfill", || !stub.exchange_queries().is_empty()).await;
        let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
        assert!(res.starts_with("HTTP/1.1 403"), "{scope:?}: {res}");
        assert_eq!(body_json(&res)["decision"], "deny");
    }
    // A Host header edit to an out-of-scope virtual host is denied too.
    let store = dir.path().join("engagements-host");
    let flags = [
        "--scope",
        "example.com",
        "--engagements",
        store.to_str().unwrap(),
    ];
    let spall = start_spall_with(Some(api), &token, &flags).await;
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
        "--engagements",
        dir.path().join("engagements").to_str().unwrap(),
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

#[tokio::test]
async fn an_exclusion_blocks_the_next_replay_and_is_audited_before_and_after() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1"); // http://example.com/e1
    let spall =
        start_spall_with(Some(api), &token, &["--scope", "example.com,*.example.com"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let replay = || post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None);
    assert!(replay().await.starts_with("HTTP/1.1 200"));

    let ex = post_text(
        spall,
        "/api/scope",
        r#"{"op":"exclude","entry":"example.com"}"#,
        None,
    )
    .await;
    assert!(ex.starts_with("HTTP/1.1 200"), "{ex}");
    assert_eq!(
        body_json(&ex)["scope"],
        serde_json::json!(["example.com", "*.example.com", "!example.com"])
    );
    let denied = replay().await;
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    let reason = body_json(&denied)["error"].as_str().unwrap().to_string();
    assert!(
        reason.contains("excluded from scope by !example.com"),
        "{reason}"
    );
    // Every host allowed still doesn't lift an exclusion.
    post_text(spall, "/api/scope", r#"{"op":"all","on":true}"#, None).await;
    assert!(replay().await.starts_with("HTTP/1.1 403"));
    assert_eq!(stub.replays().len(), 1, "nothing reached taphttp");

    // scope check: read-only, explains the decision.
    let check = |t: &str| {
        let path = format!("/api/scope/check?target={t}");
        async move { body_json(&get_text(spall, &path).await) }
    };
    let v = check("http%3A%2F%2Fexample.com%3A8080%2Fx").await;
    assert_eq!(v["host"], "example.com");
    assert_eq!(v["state"], "out");
    assert_eq!(v["by"], "!example.com");
    let v = check("api.EXAMPLE.com").await;
    assert_eq!(
        (v["state"].as_str(), v["by"].as_str()),
        (Some("in"), Some("*.example.com"))
    );
    post_text(spall, "/api/scope", r#"{"op":"all","on":false}"#, None).await;
    let v = check("other.test").await;
    assert_eq!(
        (v["state"].as_str(), v["by"].is_null()),
        (Some("unknown"), true)
    );
    assert!(get_text(spall, "/api/scope/check")
        .await
        .starts_with("HTTP/1.1 400"));

    post_text(
        spall,
        "/api/scope",
        r#"{"op":"remove","entry":"!example.com"}"#,
        None,
    )
    .await;
    assert!(replay().await.starts_with("HTTP/1.1 200"));

    let changes: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "scope")
        .collect();
    assert_eq!(changes[0]["reason"], "exclude example.com");
    assert_eq!(
        changes[0]["scope_before"],
        serde_json::json!(["example.com", "*.example.com"])
    );
    assert_eq!(
        changes[0]["scope_after"],
        serde_json::json!(["example.com", "*.example.com", "!example.com"])
    );
    assert_eq!(changes.last().unwrap()["reason"], "remove !example.com");
}

#[tokio::test]
async fn scope_lives_on_the_engagement_and_survives_a_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1"); // http://example.com/e1
    let spall = start_spall_with(Some(api), &token, &["--scope", "other.test,*"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(
        status["engagement"]["name"], "Engagement",
        "bootstrapped from --scope"
    );
    post_text(
        spall,
        "/api/scope",
        r#"{"op":"add","entry":"example.com"}"#,
        None,
    )
    .await;
    post_text(
        spall,
        "/api/scope",
        r#"{"op":"exclude","entry":"pay.example.com"}"#,
        None,
    )
    .await;

    // A second spall on the same data: the engagement's scope wins over a
    // different --scope, and `*` was never saved.
    let again = start_spall_with(Some(api), &token, &["--scope", "nothing.test"]).await;
    eventually("second backfill", || stub.exchange_queries().len() >= 2).await;
    let status = body_json(&get_text(again, "/api/status").await);
    assert_eq!(
        status["scope"],
        serde_json::json!(["other.test", "example.com", "!pay.example.com"])
    );
    assert_eq!(status["scope_all"], false);
    let res = post_text(again, "/api/replay", r#"{"id":"e1"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let rec = audit_records(&token)
        .into_iter()
        .rfind(|r| r["action"] == "replay")
        .unwrap();
    assert_eq!(
        rec["engagement"], status["engagement"]["id"],
        "every record names the engagement"
    );
}

#[tokio::test]
async fn engagements_are_edited_created_and_switched_through_the_api_and_audited() {
    let (dir, _stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let e = body_json(&get_text(spall, "/api/engagement").await);
    let first = e["id"].as_str().unwrap().to_string();
    assert_eq!(e["scope"], serde_json::json!(["example.com"]));

    let patch = |body: &'static str| send_text(spall, "PATCH", "/api/engagement", body, None);
    let res =
        patch(r#"{"name":"ACME Q4","client":"ACME","starts":"2026-01-05","ends":"2099-12-31"}"#)
            .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(body_json(&res)["engagement"]["name"], "ACME Q4");
    for (bad, want) in [
        (r#"{"ends":"2026-01-01"}"#, "ends: before starts"),
        (r#"{"scope":["x.test"]}"#, "POST /api/scope"),
        (r#"{"starts":"tomorrow"}"#, "YYYY-MM-DD"),
    ] {
        let res = patch(bad).await;
        assert!(res.starts_with("HTTP/1.1 400"), "{bad}: {res}");
        assert!(
            body_json(&res)["error"].as_str().unwrap().contains(want),
            "{bad}: {res}"
        );
    }
    let forged = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"name":"pwned"}"#,
        Some("Content-Type: application/json\r\nOrigin: http://evil.example\r\n"),
    )
    .await;
    assert!(forged.starts_with("HTTP/1.1 403"), "{forged}");

    post_text(spall, "/api/scope", r#"{"op":"all","on":true}"#, None).await;
    patch(r#"{"notes":"every host on"}"#).await;
    let created = post_text(
        spall,
        "/api/engagements",
        r#"{"name":"Beta retest","scope":["beta.test","!sso.beta.test"]}"#,
        None,
    )
    .await;
    assert!(created.starts_with("HTTP/1.1 201"), "{created}");
    let beta = body_json(&created)["id"].as_str().unwrap().to_string();
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["engagement"]["id"], beta.as_str());
    assert_eq!(
        status["scope"],
        serde_json::json!(["beta.test", "!sso.beta.test"])
    );
    assert_eq!(
        status["scope_all"], false,
        "switching engagements turns every-host off"
    );
    let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
    assert!(
        res.starts_with("HTTP/1.1 403"),
        "example.com isn't in the new engagement: {res}"
    );

    let list = body_json(&get_text(spall, "/api/engagements").await);
    let names: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["name"].as_str().unwrap(), x["active"].as_bool().unwrap()))
        .collect();
    assert_eq!(names, [("Beta retest", true), ("ACME Q4", false)]);
    let back = post_text(
        spall,
        "/api/engagements/active",
        r#"{"id":"acme q4"}"#,
        None,
    )
    .await;
    assert!(back.starts_with("HTTP/1.1 200"), "{back}");
    assert!(post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None)
        .await
        .starts_with("HTTP/1.1 200"));
    let missing = post_text(spall, "/api/engagements/active", r#"{"id":"nope"}"#, None).await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");

    let recs: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "engagement")
        .collect();
    let reasons: Vec<_> = recs
        .iter()
        .map(|r| r["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        reasons,
        [
            "set client, ends, name, starts".to_string(),
            "set notes".into(),
            format!("new {beta}"),
            format!("use {first}")
        ]
    );
    assert_eq!(
        recs[0]["changes"]["name"],
        serde_json::json!({ "before": "Engagement", "after": "ACME Q4" })
    );
    assert_eq!(
        (
            recs[0]["scope_all"].as_bool(),
            recs[1]["scope_all"].as_bool()
        ),
        (Some(false), Some(true))
    );
    assert_eq!(
        recs[2]["scope_all"], false,
        "recorded after the switch turned it off"
    );
    assert_eq!(recs[2]["changes"]["scope_all"]["before"], true);
    assert_eq!(
        recs[2]["engagement"],
        first.as_str(),
        "recorded under the engagement it was made from"
    );
}

#[tokio::test]
async fn a_corrupt_engagement_stops_spall_instead_of_resetting_scope() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let store = dir.path().join("engagements");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("active"), "x").unwrap();
    std::fs::write(store.join("x.json"), "{ torn").unwrap();
    let (_stub, api) = start_stub().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = spall::config::Config::try_parse_from([
        "spall",
        "--taphttp-api",
        &format!("http://{api}"),
        "--taphttp-token-file",
        token.to_str().unwrap(),
        "--audit-log",
        dir.path().join("audit.jsonl").to_str().unwrap(),
        "--engagements",
        store.to_str().unwrap(),
        "--scope",
        "*",
    ])
    .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), spall::serve(config, listener))
        .await
        .expect("serve should fail fast");
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("engagements") && err.contains("not JSON"),
        "{err}"
    );
}

#[tokio::test]
async fn rules_of_engagement_gate_replays_and_changes_take_effect_on_the_next_send() {
    let (dir, stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let patch = |body: String| async move {
        send_text(spall, "PATCH", "/api/engagement", &body, None).await
    };
    let replay = |body: &'static str| post_text(spall, "/api/replay", body, None);

    // Methods: DELETE isn't allowed at all (no confirmation can lift it).
    let res = patch(r#"{"rules":{"methods":["GET","post"]}}"#.into()).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let denied = replay(r#"{"id":"e1","method":"DELETE"}"#).await;
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    assert!(body_json(&denied)["error"]
        .as_str()
        .unwrap()
        .contains("DELETE isn't allowed by the rules of engagement (allowed: GET, POST)"));

    // Rate: 1 per second. The first send goes, the next is refused, and
    // switching the ceiling off lets the next one through.
    patch(r#"{"rules":{"rate_limit":1}}"#.into()).await;
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 200"));
    let limited = replay(r#"{"id":"e1"}"#).await;
    assert!(limited.starts_with("HTTP/1.1 429"), "{limited}");
    assert!(
        limited.to_ascii_lowercase().contains("retry-after: 1"),
        "{limited}"
    );
    assert_eq!(body_json(&limited)["decision"], "rate_limited");
    patch(r#"{"rules":{"rate_limit":0}}"#.into()).await;
    assert!(replay(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 200"));

    // A no-test window around now: nothing goes, and status says why.
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let fmt = |z: jiff::Zoned| z.datetime().strftime("%Y-%m-%dT%H:%M").to_string();
    let from = fmt(now.checked_sub(jiff::Span::new().hours(1)).unwrap());
    let to = fmt(now.checked_add(jiff::Span::new().hours(1)).unwrap());
    let res = patch(format!(
        r#"{{"rules":{{"blackouts":[{{"from":"{from}","to":"{to}","note":"change freeze"}}]}}}}"#
    ))
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let blocked = replay(r#"{"id":"e1"}"#).await;
    assert!(blocked.starts_with("HTTP/1.1 403"), "{blocked}");
    assert!(
        body_json(&blocked)["error"]
            .as_str()
            .unwrap()
            .contains("no-test window"),
        "{blocked}"
    );
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["testing"]["allowed"], false);
    assert!(status["testing"]["reason"]
        .as_str()
        .unwrap()
        .ends_with(": change freeze"));
    patch(r#"{"rules":{"blackouts":[]}}"#.into()).await;

    // An expired authorization stops everything.
    let yesterday = now.date().yesterday().unwrap().to_string();
    patch(format!(r#"{{"expires":"{yesterday}"}}"#)).await;
    let expired = replay(r#"{"id":"e1"}"#).await;
    assert!(
        body_json(&expired)["error"]
            .as_str()
            .unwrap()
            .contains("expired"),
        "{expired}"
    );
    let e = body_json(&get_text(spall, "/api/engagement").await);
    assert_eq!(e["testing_now"]["allowed"], false);

    let bad = patch(r#"{"rules":{"time_zone":"Mars/Olympus"}}"#.into()).await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    assert_eq!(
        stub.replays().len(),
        2,
        "only the two allowed sends reached taphttp"
    );
    let outcomes: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "replay")
        .map(|r| r["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        outcomes,
        ["denied", "sent", "rate_limited", "sent", "denied", "denied"]
    );
    let rule_changes: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "engagement")
        .filter_map(|r| r["reason"].as_str().map(String::from))
        .collect();
    assert_eq!(rule_changes[0], "set rules.methods");
}

#[tokio::test]
async fn a_pasted_scope_list_is_previewed_then_applied_in_one_audited_change() {
    let (dir, _stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let text = serde_json::json!({ "text": "In scope:\n- https://www.acme.test/login\n* *.api.acme.test\nexample.com\nOut of scope:\npay.acme.test (vendor)\nnot a host!\n" });
    let res = post_text(spall, "/api/scope/parse", &text.to_string(), None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let preview = body_json(&res);
    assert_eq!(
        preview["add"],
        serde_json::json!(["www.acme.test", "*.api.acme.test"])
    );
    assert_eq!(preview["exclude"], serde_json::json!(["pay.acme.test"]));
    let items = preview["items"].as_array().unwrap();
    assert!(items
        .iter()
        .any(|i| i["source"] == "example.com" && i["present"] == true));
    assert!(
        items.iter().any(|i| i["error"].is_string()),
        "bad lines are reported, not dropped"
    );
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(
        status["scope"],
        serde_json::json!(["example.com"]),
        "a preview changes nothing"
    );

    // A cross-site page can't preview (it would learn the scope) or apply.
    let forged = post_text(
        spall,
        "/api/scope/parse",
        &text.to_string(),
        Some("Content-Type: application/json\r\nOrigin: http://evil.example\r\n"),
    )
    .await;
    assert!(forged.starts_with("HTTP/1.1 403"), "{forged}");

    let apply =
        serde_json::json!({ "op": "apply", "add": preview["add"], "exclude": preview["exclude"] });
    let res = post_text(spall, "/api/scope", &apply.to_string(), None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(
        body_json(&res)["scope"],
        serde_json::json!([
            "example.com",
            "www.acme.test",
            "*.api.acme.test",
            "!pay.acme.test"
        ])
    );
    // All or nothing: one bad entry changes nothing.
    let bad = post_text(
        spall,
        "/api/scope",
        r#"{"op":"apply","add":["ok.test","http://nope/"]}"#,
        None,
    )
    .await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    let status = body_json(&get_text(spall, "/api/status").await);
    assert!(!status["scope"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "ok.test"));

    let rec = audit_records(&token)
        .into_iter()
        .rfind(|r| r["action"] == "scope")
        .unwrap();
    assert_eq!(rec["reason"], "apply 2 in, 1 excluded");
    assert_eq!(rec["scope_after"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn hosts_seen_in_traffic_carry_their_scope_state_live() {
    let (_dir, _stub, spall) = replay_setup(&[], &[]).await;
    eventually_async("e1 counted", || async {
        body_json(&get_text(spall, "/api/hosts").await)["hosts"]
            .as_array()
            .is_some_and(|h| !h.is_empty())
    })
    .await;
    let l = body_json(&get_text(spall, "/api/hosts").await);
    assert_eq!(l["hosts"][0]["host"], "example.com");
    assert_eq!(l["hosts"][0]["state"], "in");
    assert_eq!(l["hosts"][0]["by"], "example.com");
    post_text(
        spall,
        "/api/scope",
        r#"{"op":"exclude","entry":"example.com"}"#,
        None,
    )
    .await;
    let l = body_json(&get_text(spall, "/api/hosts").await);
    assert_eq!(
        (l["hosts"][0]["state"].as_str(), l["counts"]["out"].as_u64()),
        (Some("out"), Some(1))
    );
    post_text(
        spall,
        "/api/scope",
        r#"{"op":"remove","entry":"example.com"}"#,
        None,
    )
    .await;
    let unknown = body_json(&get_text(spall, "/api/hosts?state=unknown").await);
    assert_eq!(unknown["hosts"].as_array().unwrap().len(), 1);
    let none = body_json(&get_text(spall, "/api/hosts?state=in").await);
    assert!(none["hosts"].as_array().unwrap().is_empty());
    assert_eq!(none["counts"]["unknown"], 1, "counts cover every host");
    assert!(get_text(spall, "/api/hosts?state=maybe")
        .await
        .starts_with("HTTP/1.1 400"));
}

#[tokio::test]
async fn hosts_from_a_log_file_include_host_headers_and_are_all_unknown() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let log = token.with_file_name("unused.jsonl");
    std::fs::write(&log, "").unwrap();
    let spall = start_spall(None, &token).await;
    let line = |host: &str, header: &str| {
        format!(
            r#"{{"id":"x","ts":"2026-11-22T10:00:00Z","host":"{host}","method":"GET","url":"http://{host}/","req_headers":[["Host","{header}"]]}}"#
        )
    };
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    use std::io::Write as _;
    writeln!(f, "{}", line("cdn.other.test", "cdn.other.test")).unwrap();
    writeln!(f, "{}", line("app.acme.test", "admin.internal:8080")).unwrap();
    drop(f);
    eventually_async("hosts counted", || async {
        body_json(&get_text(spall, "/api/hosts").await)["hosts"]
            .as_array()
            .is_some_and(|h| h.len() == 3)
    })
    .await;
    let l = body_json(&get_text(spall, "/api/hosts").await);
    let mut names: Vec<_> = l["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["host"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["admin.internal", "app.acme.test", "cdn.other.test"]);
    assert_eq!(l["counts"]["unknown"], 3);
}

#[tokio::test]
async fn an_openapi_file_is_previewed_then_applied_and_kept_on_the_engagement() {
    let (dir, _stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let spec = "openapi: 3.0.3\ninfo: {title: ACME, version: '2'}\nservers:\n  - url: https://api.acme.test/v2\n  - url: https://example.com\npaths:\n  /users: {}\n  /orders/{id}: {}\n";
    let body = serde_json::json!({ "text": spec }).to_string();
    let res = post_text(spall, "/api/openapi/parse", &body, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let i = body_json(&res);
    assert_eq!(
        i["add"],
        serde_json::json!(["api.acme.test"]),
        "example.com is already in scope"
    );
    assert_eq!(
        i["spec"]["paths"],
        serde_json::json!(["/orders/{id}", "/users"]),
        "sorted"
    );
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(
        status["scope"],
        serde_json::json!(["example.com"]),
        "a preview changes nothing"
    );

    let apply = serde_json::json!({ "op": "apply", "add": i["add"], "exclude": i["exclude"] });
    assert!(post_text(spall, "/api/scope", &apply.to_string(), None)
        .await
        .starts_with("HTTP/1.1 200"));
    let keep = serde_json::json!({ "api_specs": [i["spec"]] });
    let res = send_text(spall, "PATCH", "/api/engagement", &keep.to_string(), None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let e = body_json(&get_text(spall, "/api/engagement").await);
    assert_eq!(e["api_specs"][0]["title"], "ACME");
    assert_eq!(
        e["scope"],
        serde_json::json!(["example.com", "api.acme.test"])
    );

    let bad = post_text(
        spall,
        "/api/openapi/parse",
        r#"{"text":"title: nope\n"}"#,
        None,
    )
    .await;
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    let forged = post_text(
        spall,
        "/api/openapi/parse",
        &body,
        Some("Content-Type: application/json\r\nOrigin: http://evil.example\r\n"),
    )
    .await;
    assert!(forged.starts_with("HTTP/1.1 403"), "{forged}");
    let rec = audit_records(&token)
        .into_iter()
        .rfind(|r| r["action"] == "engagement")
        .unwrap();
    assert_eq!(rec["reason"], "set api_specs");
}
