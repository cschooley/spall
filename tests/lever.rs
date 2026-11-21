//! spall in lever-API mode against a stub taphttp API (no Docker).
//!
//! The stub follows taphttp v0.3.0's documented contract: bearer auth,
//! `GET /exchanges` newest first with a `before=<id>` cursor,
//! `GET /exchanges/{id}`, and a `/feed` WebSocket that resumes with
//! `?since=<id>` and sends `reset` / `lagged` control frames.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
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
}

#[tokio::test]
async fn history_proxy_is_unavailable_without_api() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let spall = start_spall(None, &token).await;
    let res = get_text(spall, "/api/exchanges").await;
    assert!(res.starts_with("HTTP/1.1 503"), "{res}");
}
