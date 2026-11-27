//! spall in lever-API mode against a stub taphttp API (no Docker).
//!
//! The stub follows taphttp v0.6.0's documented contract: bearer auth,
//! `GET /exchanges` newest first with a `before=<id>` cursor,
//! `GET /exchanges/{id}`, a `/feed` WebSocket that resumes with
//! `?since=<id>` and sends `reset` / `lagged` control frames, and
//! `POST /replay` (200 with the recorded replay, 502 with a recorded
//! failure, 400 for bad input), and v0.7.0's `POST /send` (the same
//! answers, for a new request).

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
    /// Issued proxy credentials (taphttp#25 `/clients`): each POST body.
    clients: Mutex<Vec<serde_json::Value>>,
    /// Credentials revoked (DELETE /clients/{id}), in order.
    revoked: Mutex<Vec<String>>,
    /// What GET /clients says each credential sent: as if taphttp counted.
    client_sent: Mutex<u32>,
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
    /// Body of each POST /send, parsed.
    sends: Mutex<Vec<serde_json::Value>>,
    /// Where it listens, for a restart against the same stub.
    addr: Mutex<Option<SocketAddr>>,
}

#[derive(Clone)]
struct Stub(Arc<StubState>);

impl Stub {
    fn push(&self, id: &str) {
        self.push_json(id, entry(id));
    }
    fn push_json(&self, id: &str, json: String) {
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
    fn sends(&self) -> Vec<serde_json::Value> {
        self.0.sends.lock().unwrap().clone()
    }
    fn clients(&self) -> Vec<serde_json::Value> {
        self.0.clients.lock().unwrap().clone()
    }
    fn revoked(&self) -> Vec<String> {
        self.0.revoked.lock().unwrap().clone()
    }
}

/// `POST /clients`: a credential `c-<n>` with a password that needs
/// escaping in a URL. `GET /clients` lists them with `client_sent` as the
/// count and one refusal each. `DELETE /clients/{id}` revokes.
async fn clients_post(State(s): State<Stub>, body: String) -> Response {
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let mut list = s.0.clients.lock().unwrap();
    list.push(v.clone());
    let id = format!("c-{}", list.len());
    (
        StatusCode::CREATED,
        axum::Json(serde_json::json!({
            "id": id, "username": id, "password": "p@ss/1",
            "allow": v["allow"], "budget": v["budget"], "meta": v["meta"],
            "sent": 0, "refused": 0, "revoked": false,
        })),
    )
        .into_response()
}

async fn clients_get(State(s): State<Stub>) -> Response {
    let sent = *s.0.client_sent.lock().unwrap();
    let revoked = s.0.revoked.lock().unwrap().clone();
    let n = s.0.clients.lock().unwrap().len();
    let list: Vec<serde_json::Value> = (1..=n)
        .map(|i| {
            let id = format!("c-{i}");
            serde_json::json!({"id": id, "sent": sent, "refused": 1, "revoked": revoked.contains(&id)})
        })
        .collect();
    axum::Json(list).into_response()
}

async fn clients_delete(State(s): State<Stub>, Path(id): Path<String>) -> Response {
    s.0.revoked.lock().unwrap().push(id);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /send`: a URL with `fails` in it gets a recorded failure (502),
/// one with `bad` a 400; anything else a recorded exchange `s-<n>`.
async fn send(State(s): State<Stub>, body: String) -> Response {
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let n = {
        let mut sends = s.0.sends.lock().unwrap();
        sends.push(v.clone());
        sends.len()
    };
    let url = v["url"].as_str().unwrap().to_string();
    let method = v["method"].as_str().unwrap_or("GET").to_string();
    let id = format!("s-{n}");
    // A small site for sweeps (#75), under /sweep-site/ only: a soft-404
    // page for everything, a robots.txt, and a login wall on /admin.
    let site = |e: String| -> String {
        if !url.contains("/sweep-site/") {
            return e;
        }
        let (status, body) = if url.ends_with("/robots.txt") {
            (200, "User-agent: *\\nDisallow: /secret\\n".to_string())
        } else if url.ends_with("/admin") {
            (401, "login required".to_string())
        } else {
            (200, "x".repeat(5000))
        };
        e.replace(r#""status":200"#, &format!(r#""status":{status}"#))
            .replace(r#""res_body":null"#, &format!(r#""res_body":"{body}""#))
    };
    let json = |status, extra: &str| {
        let e = site(
            entry(&id)
                .replace(&format!("http://example.com/{id}"), &url)
                .replace(r#""method":"GET""#, &format!(r#""method":"{method}""#))
                .replace(
                    r#""replay_of":null"#,
                    &format!(r#""replay_of":null,"origin":"send"{extra}"#),
                ),
        );
        // Recorded, like taphttp: in history and on the feed.
        s.push_json(&id, e.clone());
        (status, [(header::CONTENT_TYPE, "application/json")], e).into_response()
    };
    if url.contains("fails") {
        json(
            StatusCode::BAD_GATEWAY,
            r#","error":"connect error: Connection refused""#,
        )
    } else if url.contains("bad") {
        (StatusCode::BAD_REQUEST, "invalid request").into_response()
    } else {
        json(StatusCode::OK, "")
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
        .route("/send", post(send))
        .route("/clients", post(clients_post).get(clients_get))
        .route("/clients/{id}", axum::routing::delete(clients_delete))
        .route("/info", get(|| async { axum::Json(serde_json::json!({
            "taphttp_version": "0.6.1",
            "ca": { "subject": "CN=taphttp CA", "sha256": "EB:6F:71:3A", "not_before": "2026-11-20T00:00:00Z", "not_after": "2036-11-20T00:00:00Z" },
            "api": { "listen": "0.0.0.0:8081", "tls": false }
        })) }))
        .route("/ca.pem", get(|| async { ([("content-type", "application/x-pem-file")], STUB_PEM) }))
        .route("/ca.der", get(|| async { ([("content-type", "application/x-x509-ca-cert")], vec![0x30u8, 0x82, 0x01, 0x0a]) }))
        .layer(middleware::from_fn(auth))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    *state.0.addr.lock().unwrap() = Some(addr);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, addr)
}

const STUB_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIBstub\n-----END CERTIFICATE-----\n";

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
    let config = spall_config(api, token_file, extra);
    tokio::spawn(spall::serve(config, listener));
    addr
}

/// Like `start_spall_with`, keeping hold of the replayer: the agent side
/// (`agent_replay`) has no HTTP route, so tests call it directly.
async fn start_agent_spall(
    api: SocketAddr,
    token_file: &FsPath,
    extra: &[&str],
) -> (SocketAddr, Arc<spall::replay::Replayer>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = spall::build(spall_config(Some(api), token_file, extra)).unwrap();
    let replayer = app.replayer.clone().unwrap();
    tokio::spawn(app.run(listener));
    (addr, replayer)
}

fn spall_config(
    api: Option<SocketAddr>,
    token_file: &FsPath,
    extra: &[&str],
) -> spall::config::Config {
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
    if !extra.contains(&"--mcp-token-file") {
        args.push("--mcp-token-file".into());
        args.push(
            token_file
                .with_file_name("mcp-token")
                .to_str()
                .unwrap()
                .into(),
        );
    }
    args.extend(extra.iter().map(|s| s.to_string()));
    spall::config::Config::try_parse_from(args).unwrap()
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

/// Like `get_text`, for a binary body (non-UTF-8 bytes replaced).
async fn get_lossy(addr: SocketAddr, path: &str) -> String {
    let mut s = http_get(addr, path).await;
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out))
        .await
        .expect("timed out")
        .unwrap();
    String::from_utf8_lossy(&out).into_owned()
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

#[tokio::test]
async fn the_public_ca_certificate_is_offered_for_download_with_its_fingerprint() {
    let (_dir, _stub, spall) = replay_setup(&[], &[]).await;
    let info = body_json(&get_text(spall, "/api/ca").await);
    assert_eq!(info["sha256"], "EB:6F:71:3A");
    assert_eq!(info["subject"], "CN=taphttp CA");
    assert_eq!(info["not_after"], "2036-11-20T00:00:00Z");
    assert_eq!(info["taphttp_version"], "0.6.1");
    assert!(info.get("api").is_none(), "only the CA's public details");

    let pem = get_text(spall, "/api/ca.pem").await;
    assert!(pem.starts_with("HTTP/1.1 200"), "{pem}");
    let lower = pem.to_ascii_lowercase();
    assert!(
        lower.contains("content-type: application/x-pem-file"),
        "{pem}"
    );
    assert!(
        lower.contains("content-disposition: attachment; filename=\"taphttp-ca.pem\""),
        "{pem}"
    );
    assert!(lower.contains("x-content-type-options: nosniff"), "{pem}");
    assert!(pem.ends_with(STUB_PEM), "{pem}");
    assert!(
        !pem.contains(TOKEN),
        "the API token never reaches the browser"
    );
    let der = get_lossy(spall, "/api/ca.der").await;
    assert!(
        der.to_ascii_lowercase()
            .contains("content-type: application/x-x509-ca-cert"),
        "{der}"
    );
    assert!(
        der.to_ascii_lowercase()
            .contains("filename=\"taphttp-ca.der\""),
        "{der}"
    );

    let dir = tempfile::TempDir::new().unwrap();
    let log_mode = start_spall(None, &dir.path().join("api-token")).await;
    assert!(get_text(log_mode, "/api/ca.pem")
        .await
        .starts_with("HTTP/1.1 503"));
}

#[tokio::test]
async fn candidates_are_flagged_triaged_audited_and_kept_across_a_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    for id in ["e1", "e2", "e3"] {
        stub.push(id);
    }
    let spall = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;

    // Flag e1: rule from the catalog, the exchange as evidence.
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e1","rule":"WSTG-INPV-05","parameter":"id","level":"error","replays":["e2"]}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let c = body_json(&res);
    let id = c["id"].as_str().unwrap().to_string();
    let f = &c["finding"];
    assert_eq!(f["ruleId"], "WSTG-INPV-05");
    assert_eq!(f["level"], "error");
    assert_eq!(f["properties"]["flux/status"], "candidate");
    assert_eq!(
        f["properties"]["flux/replayIds"],
        serde_json::json!(["e1", "e2"])
    );
    assert_eq!(f["taxa"][0]["id"], "89");
    assert_eq!(
        f["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "http://example.com/e1"
    );
    assert_eq!(
        f["fingerprints"]["flux/dast/v1"].as_str().unwrap().len(),
        64
    );
    assert_eq!(c["flagged_by"], "human");

    // The same endpoint, parameter and rule again: 409 with the existing one.
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e1","rule":"WSTG-INPV-05","parameter":"id"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    assert_eq!(body_json(&res)["existing"]["id"], id.as_str());

    // Bad input: unknown rule, unknown or malformed exchange, unknown field.
    for (body, want) in [
        (r#"{"exchange":"e1","rule":"WSTG-NOPE-01"}"#, "400"),
        (r#"{"exchange":"missing","rule":"WSTG-INPV-05"}"#, "404"),
        (r#"{"exchange":"../x","rule":"WSTG-INPV-05"}"#, "400"),
        (
            r#"{"exchange":"e1","rule":"WSTG-INPV-05","replays":["missing"]}"#,
            "404",
        ),
        (
            r#"{"exchange":"e1","rule":"WSTG-INPV-05","sneaky":true}"#,
            "400",
        ),
    ] {
        let res = post_text(spall, "/api/candidates", body, None).await;
        assert!(
            res.starts_with(&format!("HTTP/1.1 {want}")),
            "{body}: {res}"
        );
    }

    // A second one, then triage.
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e3","rule":"WSTG-INPV-01","parameter":"q"}"#,
        None,
    )
    .await;
    let id2 = body_json(&res)["id"].as_str().unwrap().to_string();
    let patch = |cid: &str, body: &str| {
        let (cid, body) = (cid.to_string(), body.to_string());
        async move {
            send_text(
                spall,
                "PATCH",
                &format!("/api/candidates/{cid}"),
                &body,
                None,
            )
            .await
        }
    };
    let res = patch(&id, r#"{"op":"confirm"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let guid = body_json(&res)["finding"]["correlationGuid"]
        .as_str()
        .unwrap()
        .to_string();
    let res = patch(
        &id2,
        &format!(r#"{{"op":"duplicate","of":"{id}","justification":"same handler"}}"#),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let dup = body_json(&res);
    assert_eq!(
        dup["finding"]["suppressions"][0]["properties"]["flux/duplicateOf"],
        guid.as_str()
    );
    let res = patch(&id2, r#"{"op":"reject","justification":" "}"#).await;
    assert!(res.starts_with("HTTP/1.1 400"), "{res}");
    let res = patch("c-00000000", r#"{"op":"confirm"}"#).await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");
    let res = patch(&id, r#"{"op":"evidence","exchanges":["e3"]}"#).await;
    assert_eq!(
        body_json(&res)["finding"]["properties"]["flux/replayIds"],
        serde_json::json!(["e1", "e2", "e3"])
    );

    // Listing: newest first; by exchange.
    let list = body_json(&get_text(spall, "/api/candidates").await);
    let ids: Vec<&str> = list["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [id2.as_str(), id.as_str()]);
    let by = body_json(&get_text(spall, "/api/candidates?exchange=e2").await);
    assert_eq!(by["candidates"].as_array().unwrap().len(), 1);

    // Audited: who, what, which candidate.
    let recs: Vec<serde_json::Value> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "candidate")
        .collect();
    let reasons: Vec<&str> = recs.iter().map(|r| r["reason"].as_str().unwrap()).collect();
    assert!(
        reasons.contains(&format!("flag {id} as WSTG-INPV-05").as_str()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&format!("duplicate {id2}: same handler").as_str()),
        "{reasons:?}"
    );
    let conf = recs
        .iter()
        .find(|r| r["reason"] == format!("confirm {id}"))
        .unwrap();
    assert_eq!(conf["outcome"], "candidate_changed");
    assert_eq!(
        conf["changes"]["status"],
        serde_json::json!({"before": "candidate", "after": "confirmed"})
    );
    assert_eq!(conf["exchange_id"], "e1");
    assert!(
        recs.iter().any(|r| r["error"].is_string()),
        "refused steps are audited too"
    );

    // id is confirmed, id2 a duplicate: nothing waits for triage.
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["to_triage"],
        0
    );

    // A restart on the same data keeps them.
    let again = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("second backfill", || stub.exchange_queries().len() >= 2).await;
    let list2 = body_json(&get_text(again, "/api/candidates").await);
    assert_eq!(list2, list);
}

#[tokio::test]
async fn tester_notes_on_a_candidate_reach_the_preview_only_when_ticked() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let spall = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e1","rule":"WSTG-INFO-02"}"#,
        None,
    )
    .await;
    let id = body_json(&res)["id"].as_str().unwrap().to_string();
    let path = format!("/api/candidates/{id}");
    let patch = |body: &'static str| {
        let path = path.clone();
        async move { send_text(spall, "PATCH", &path, body, None).await }
    };
    let preview = || {
        let id = id.clone();
        async move {
            let p = body_json(&get_text(spall, &format!("/api/candidates/{id}/redaction")).await);
            p["finding"]["message"]["text"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };

    let res = patch(r#"{"op":"note","text":"looks like an ad network"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let c = body_json(&res);
    assert_eq!(c["notes"][0]["text"], "looks like an ad network");
    assert_eq!(c["notes"][0]["by"], "human");
    assert!(c.get("notes_in_export").is_none(), "off by default: {c}");
    assert!(!preview().await.contains("Tester notes"));

    let res = patch(r#"{"op":"notes_in_export","on":true}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(preview()
        .await
        .ends_with("Tester notes:\n- looks like an ad network"));

    let at = c["notes"][0]["at"].as_str().unwrap().to_string();
    let res = send_text(
        spall,
        "PATCH",
        &path,
        &format!(r#"{{"op":"unnote","at":"{at}"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(body_json(&res).get("notes").is_none());
    assert!(patch(r#"{"op":"note","text":""}"#)
        .await
        .starts_with("HTTP/1.1 400"));

    let reasons: Vec<String> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "candidate")
        .filter_map(|r| r["reason"].as_str().map(String::from))
        .collect();
    for op in ["note", "notes_in_export", "unnote"] {
        assert!(
            reasons.iter().any(|r| r == &format!("{op} {id}")),
            "{op} audited: {reasons:?}"
        );
    }
}

#[tokio::test]
async fn promoted_candidates_export_as_redacted_flux_and_unredacted_needs_the_opt_in() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    // A captured login: credentials in headers, the URL and the body.
    stub.push_json(
        "login",
        serde_json::json!({
            "schema_version": 3, "id": "login", "ts": "2026-12-25T10:00:00Z", "host": "example.com",
            "method": "POST", "url": "http://example.com/login?next=/&access_token=tok-123456",
            "req_headers": [["cookie", "sid=8f3a91c2d4e5"], ["authorization", "Bearer abcdefghijklmnop"], ["content-type", "application/json"]],
            "req_body": "{\"user\":\"bob\",\"password\":\"hunter2\"}", "req_body_encoding": "utf8",
            "status": 500, "res_headers": [["set-cookie", "sid=new; HttpOnly"]],
            "res_body": "ERROR near \"'\"", "res_body_encoding": "utf8", "duration_ms": 1, "replay_of": null
        })
        .to_string(),
    );
    let spall = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;

    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"login","rule":"WSTG-INPV-05","parameter":"user"}"#,
        None,
    )
    .await;
    let id = body_json(&res)["id"].as_str().unwrap().to_string();
    let patch = |body: &'static str| {
        let id = id.clone();
        async move { send_text(spall, "PATCH", &format!("/api/candidates/{id}"), body, None).await }
    };

    // The preview says what an export would strip, and stores nothing.
    let prev = body_json(&get_text(spall, &format!("/api/candidates/{id}/redaction")).await);
    let kinds: Vec<&str> = prev["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    for k in ["token", "authorization", "cookie", "password", "set-cookie"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    assert_eq!(
        prev["finding"]["webRequest"]["headers"]["cookie"],
        "[REDACTED:cookie]"
    );

    // Nothing promoted, nothing to export; an unconfirmed one can't be promoted.
    let res = post_text(spall, "/api/export", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    assert!(patch(r#"{"op":"promote"}"#)
        .await
        .starts_with("HTTP/1.1 400"));
    assert!(patch(r#"{"op":"confirm"}"#)
        .await
        .starts_with("HTTP/1.1 200"));
    let res = patch(r#"{"op":"promote"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(body_json(&res)["promoted"].is_string());
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["export"]["promoted"],
        1
    );

    // The export: valid flux, redacted, no captured secret left in it.
    let res = post_text(spall, "/api/export", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(res.contains("application/sarif+json"));
    assert!(res.contains("attachment; filename=\"spall-"));
    let (_, body) = res.split_once("\r\n\r\n").unwrap();
    for secret in [
        "8f3a91c2d4e5",
        "abcdefghijklmnop",
        "tok-123456",
        "hunter2",
        "sid=new",
    ] {
        assert!(!body.contains(secret), "{secret} leaked");
    }
    let report = flux::Report::from_json(body).unwrap();
    assert_eq!(flux::validate::validate(&report), vec![]);
    let f = report.findings().next().unwrap();
    assert_eq!(f.properties.redacted, Some(true));
    assert_eq!(f.properties.status, Some(flux::Status::Confirmed));
    assert_eq!(report.runs[0].tool.driver.rules[0].id, "WSTG-INPV-05");
    assert_eq!(report.runs[0].tool.driver.rules[0].cwes(), vec![89]);

    // Unredacted: refused without the opt-in, and audited both times.
    let res = post_text(spall, "/api/export", r#"{"unredacted":true}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    let recs: Vec<serde_json::Value> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "export")
        .collect();
    assert!(recs.iter().any(|r| r["reason"]
        .as_str()
        .unwrap_or("")
        .starts_with("exported 1 finding (redacted,")));
    assert!(recs.iter().any(|r| r["decision"] == "deny"));

    // A reject takes it out of the export.
    patch(r#"{"op":"reject","justification":"input is parameterized"}"#).await;
    assert!(post_text(spall, "/api/export", "{}", None)
        .await
        .starts_with("HTTP/1.1 409"));

    // With the opt-in: unredacted, and marked so.
    let again = start_spall_with(
        Some(api),
        &token,
        &["--scope", "example.com", "--allow-unredacted-export"],
    )
    .await;
    eventually("second backfill", || stub.exchange_queries().len() >= 2).await;
    let reopen = send_text(
        again,
        "PATCH",
        &format!("/api/candidates/{id}"),
        r#"{"op":"reopen"}"#,
        None,
    )
    .await;
    assert!(reopen.starts_with("HTTP/1.1 200"), "{reopen}");
    send_text(
        again,
        "PATCH",
        &format!("/api/candidates/{id}"),
        r#"{"op":"promote"}"#,
        None,
    )
    .await;
    let res = post_text(again, "/api/export", r#"{"unredacted":true}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let (_, body) = res.split_once("\r\n\r\n").unwrap();
    assert!(body.contains("hunter2"));
    let report = flux::Report::from_json(body).unwrap();
    assert_eq!(
        report.findings().next().unwrap().properties.redacted,
        Some(false)
    );
    assert!(res.contains("-UNREDACTED.sarif"));
}

#[tokio::test]
async fn the_redaction_list_comes_from_flux() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let spall = start_spall(None, &token).await;
    let v = body_json(&get_text(spall, "/api/redaction").await);
    let headers: Vec<&str> = v["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    for h in [
        "authorization",
        "cookie",
        "set-cookie",
        "proxy-authorization",
        "x-api-key",
    ] {
        assert!(headers.contains(&h), "{h}");
    }
}

#[tokio::test]
async fn the_command_registry_is_served_from_the_server() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    let spall = start_spall(None, &token).await;
    let v = body_json(&get_text(spall, "/api/commands").await);
    assert_eq!(v, spall::commands::specs());
    let cmds = v["commands"].as_array().unwrap();
    let replay = cmds.iter().find(|c| c["name"] == "replay").unwrap();
    assert_eq!(replay["risk"], "send");
    assert_eq!(replay["mcp"], true);
    assert_eq!(replay["args"]["required"], serde_json::json!(["id"]));
    let roe = cmds.iter().find(|c| c["name"] == "roe").unwrap();
    assert_eq!(
        (roe["risk"].as_str(), roe["mcp"].as_bool()),
        (Some("policy"), Some(false))
    );
}

#[tokio::test]
async fn a_draft_with_no_hosts_offers_the_ones_its_goal_names() {
    let (dir, _stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let agents = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(agents.starts_with("HTTP/1.1 200"), "{agents}");
    let new = r#"{"goal":"crawl https://example.com/ and evil.test","role":"mapper","constraints":{"budget":20}}"#;
    assert!(post_text(spall, "/api/tasks", new, None)
        .await
        .starts_with("HTTP/1.1 201"));

    // Offered on the list and on the task; only what's in scope.
    let list = body_json(&get_text(spall, "/api/tasks").await);
    assert_eq!(
        list["tasks"][0]["goal_hosts"],
        serde_json::json!(["example.com"])
    );
    let one = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert_eq!(one["goal_hosts"], serde_json::json!(["example.com"]));

    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_on","hosts":["example.com"]}"#,
        None,
    )
    .await;
    let t = body_json(&res);
    assert_eq!(t["status"], "approved", "{res}");
    assert_eq!(
        t["constraints"]["hosts"],
        serde_json::json!(["example.com"])
    );
    assert_eq!(t["constraints"]["budget"], 20);
    let one = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert!(one.get("goal_hosts").is_none(), "only drafts are offered");

    let rec = audit_records(&token)
        .into_iter()
        .find(|r| r["reason"] == "approve T-1 on example.com")
        .expect("audited");
    assert_eq!(rec["changes"]["status"]["after"], "approved");
}

#[tokio::test]
async fn tasks_are_made_approved_halted_audited_and_kept_per_engagement() {
    let (dir, _stub, spall) = replay_setup(&[], &[]).await;
    let token = dir.path().join("api-token");
    let new = r#"{"goal":"map example.com","role":"mapper","plan":["crawl"],"constraints":{"methods":["GET"],"budget":20}}"#;

    // Not while the RoE keeps agents out.
    let res = post_text(spall, "/api/tasks", new, None).await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    assert!(res.contains("don't allow agents"), "{res}");
    let agents = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(agents.starts_with("HTTP/1.1 200"), "{agents}");

    // Wider than the RoE: refused, nothing made.
    let wide = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"g","role":"tester","constraints":{"rate_limit":500}}"#,
        None,
    )
    .await;
    assert!(
        wide.starts_with("HTTP/1.1 400") && wide.contains("rate_limit"),
        "{wide}"
    );
    let outside = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"g","role":"tester","constraints":{"hosts":["evil.test"]}}"#,
        None,
    )
    .await;
    assert!(
        outside.starts_with("HTTP/1.1 400") && outside.contains("scope"),
        "{outside}"
    );
    let forged = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"g","role":"mapper","status":"running"}"#,
        None,
    )
    .await;
    assert!(forged.starts_with("HTTP/1.1 400"), "{forged}");

    let res = post_text(spall, "/api/tasks", new, None).await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let t = body_json(&res);
    assert_eq!(
        (t["id"].as_str(), t["status"].as_str()),
        (Some("T-1"), Some("draft"))
    );
    assert_eq!(t["constraints"]["methods"], serde_json::json!(["GET"]));
    assert_eq!(t["constraints"]["rate_limit"], 10.0, "the RoE's, left out");
    assert_eq!(t["asked_by"], "human");

    let patch = |body: &'static str| send_text(spall, "PATCH", "/api/tasks/T-1", body, None);
    let start = send_text(spall, "PATCH", "/api/tasks/T-1", r#"{"op":"start"}"#, None).await;
    assert!(
        start.starts_with("HTTP/1.1 400"),
        "a human can't run it by hand: {start}"
    );
    let res = patch(r#"{"op":"edit","note":"my boss said"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = patch(r#"{"op":"approve"}"#).await;
    assert_eq!(body_json(&res)["status"], "approved");
    let res = patch(r#"{"op":"edit","goal":"something else"}"#).await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("only a draft"),
        "{res}"
    );
    let res = patch(r#"{"op":"halt","reason":"wrong host"}"#).await;
    let halted = body_json(&res);
    assert_eq!(
        (halted["status"].as_str(), halted["reason"].as_str()),
        (Some("halted"), Some("wrong host"))
    );
    let missing = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-9",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");

    let list = body_json(&get_text(spall, "/api/tasks").await);
    let tasks = list["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(
        tasks[0].get("log").is_none() && tasks[0]["log_len"] == 2,
        "lists leave the log out"
    );
    let one = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert_eq!(one["log"][1]["to"], "halted");
    assert_eq!(one["note"], "my boss said");

    let recs: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "task")
        .collect();
    let reasons: Vec<&str> = recs.iter().map(|r| r["reason"].as_str().unwrap()).collect();
    assert!(
        reasons[0].starts_with("create:") && recs[0]["error"].as_str().unwrap().contains("agents"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"create T-1")
            && reasons.contains(&"approve T-1")
            && reasons.contains(&"halt T-1: wrong host"),
        "{reasons:?}"
    );
    assert!(recs.iter().all(|r| r["outcome"] == "task_changed"));
    let approve = recs.iter().find(|r| r["reason"] == "approve T-1").unwrap();
    assert_eq!(
        approve["changes"]["status"],
        serde_json::json!({"before": "draft", "after": "approved"})
    );

    // Another engagement's tasks never show.
    let other = post_text(spall, "/api/engagements", r#"{"name":"Other"}"#, None).await;
    assert!(other.starts_with("HTTP/1.1 201"), "{other}");
    let list = body_json(&get_text(spall, "/api/tasks").await);
    assert_eq!(list["tasks"], serde_json::json!([]));
    let res = get_text(spall, "/api/tasks/T-1").await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");
}

#[tokio::test]
async fn tasks_survive_a_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let spall = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"kept","role":"reporter"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;

    let again = start_spall_with(Some(api), &token, &["--scope", "example.com"]).await;
    eventually("second backfill", || stub.exchange_queries().len() >= 2).await;
    let t = body_json(&get_text(again, "/api/tasks/T-1").await);
    assert_eq!(
        (t["goal"].as_str(), t["status"].as_str()),
        (Some("kept"), Some("approved"))
    );
    let next = post_text(
        again,
        "/api/tasks",
        r#"{"goal":"next","role":"mapper"}"#,
        None,
    )
    .await;
    assert_eq!(body_json(&next)["id"], "T-2");
}

/// spall following a stub with e1, `example.com` in scope, agents allowed,
/// and task T-1 (made from `constraints`) approved and running.
async fn agent_setup(
    constraints: &str,
) -> (
    tempfile::TempDir,
    Stub,
    SocketAddr,
    Arc<spall::replay::Replayer>,
) {
    agent_setup_with(constraints, &["--scope", "example.com"], &[]).await
}

/// [`agent_setup`] with spall's flags and extra exchanges `(id, url)`.
async fn agent_setup_with(
    constraints: &str,
    flags: &[&str],
    urls: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    Stub,
    SocketAddr,
    Arc<spall::replay::Replayer>,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    stub.push_json(
        "offsite",
        entry("offsite").replace("http://example.com/", "http://evil.test/"),
    );
    for (id, url) in urls {
        let host = url
            .split("://")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap();
        stub.push_json(
            id,
            entry(id)
                .replace(&format!("http://example.com/{id}"), url)
                .replace(r#""host":"example.com""#, &format!(r#""host":"{host}""#)),
        );
    }
    let (spall, r) = start_agent_spall(api, &token, flags).await;
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/tasks",
        &format!(r#"{{"goal":"map","role":"tester","constraints":{constraints}}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let eng = r.engagements.active().id;
    r.tasks
        .transition(&eng, "T-1", spall::tasks::Status::Running, None, &now())
        .unwrap();
    (dir, stub, spall, r)
}

fn agent_req(json: &str) -> spall::replay::ReplayRequest {
    serde_json::from_str(json).unwrap()
}

async fn agent_send(
    r: &spall::replay::Replayer,
    task: &str,
    json: &str,
) -> (u16, serde_json::Value) {
    let res = r.agent_replay(task, agent_req(json)).await;
    let status = res.status().as_u16();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

fn task_of(r: &spall::replay::Replayer, id: &str) -> spall::tasks::Task {
    r.tasks.get(&r.engagements.active().id, id).unwrap()
}

#[tokio::test]
async fn agents_never_target_the_stack_whatever_the_scope() {
    // Everything in scope, the task allowing every host: only the stack
    // guard (#73) stands between an agent and spall's or taphttp's own API.
    let flags = [
        "--scope",
        "example.com,*",
        "--listen",
        "127.0.0.1:3000",
        "--proxy-address",
        "127.0.0.1:18080",
        "--self-address",
        "127.0.0.1:13000",
    ];
    let stack = [
        ("s1", "http://spall:3000/api/status"),
        ("s2", "http://taphttp:8081/info"),
        ("s3", "http://SPALL./api/engagement"),
        ("s4", "http://127.0.0.1:3000/api/estop/clear"),
        ("s5", "http://localhost:13000/api/tasks"),
        ("s6", "http://host.docker.internal:18080/"),
        ("s7", "http://[::1]:3000/"),
        ("s8", "http://0.0.0.0:3000/"),
        ("s9", "http://app.localhost:13000/"),
    ];
    let mut urls = stack.to_vec();
    // A dev app on this machine is a normal target.
    urls.push(("dev", "http://localhost:5173/"));
    let (dir, stub, _spall, r) = agent_setup_with(r#"{"budget":50}"#, &flags, &urls).await;
    // The lever API's own address (the stub, here) is the stack too.
    let lever = stub_addr(&stub);
    stub.push_json(
        "s10",
        entry("s10")
            .replace(
                "http://example.com/s10",
                &format!("http://{lever}/exchanges"),
            )
            .replace(r#""host":"example.com""#, &format!(r#""host":"{lever}""#)),
    );
    let mut ids: Vec<&str> = stack.iter().map(|(id, _)| *id).collect();
    ids.push("s10");
    for id in &ids {
        let (status, body) = agent_send(&r, "T-1", &format!(r#"{{"id":"{id}"}}"#)).await;
        assert_eq!(status, 403, "{id}: {body}");
        assert_eq!(body["decision"], "deny", "{id}: {body}");
        let err = body["error"].as_str().unwrap();
        assert!(err.contains("spall's own stack"), "{id}: {err}");
        assert!(err.contains("can't be approved"), "{id}: {err}");
    }
    // A POST to the stack is refused too, not paused for approval.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"s4","method":"POST","body":"{}"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(task_of(&r, "T-1").status, spall::tasks::Status::Running);
    assert!(task_of(&r, "T-1").pending.is_none());
    assert!(stub.replays().is_empty(), "{:?}", stub.replays());

    let (status, body) = agent_send(&r, "T-1", r#"{"id":"dev"}"#).await;
    assert_eq!(status, 200, "a local dev app isn't the stack: {body}");
    assert_eq!(stub.replays().len(), 1);

    let token = dir.path().join("api-token");
    let refused: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|a| a["outcome"] == "denied")
        .collect();
    assert_eq!(refused.len(), ids.len() + 1, "{refused:?}");
    assert!(refused
        .iter()
        .all(|a| a["task"] == "T-1" && a["actor"] == "agent"));
}

#[tokio::test]
async fn an_agent_sends_under_its_task_attributed_charged_and_logged() {
    let (dir, stub, _spall, r) = agent_setup(r#"{"budget":5}"#).await;
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1","meta":{"actor":"human"}}"#).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["id"], "r-e1");
    let sent = stub.replays();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["meta"],
        serde_json::json!({"actor": "agent", "task": "T-1"}),
        "spall's attribution, never the caller's"
    );

    let t = task_of(&r, "T-1");
    assert_eq!(t.spent, 1);
    let last = serde_json::to_value(t.log.last().unwrap()).unwrap();
    assert_eq!(last["kind"], "request");
    assert_eq!(
        (
            last["outcome"].as_str(),
            last["exchange"].as_str(),
            last["status"].as_u64()
        ),
        (Some("sent"), Some("r-e1"), Some(200))
    );

    let recs = audit_records(&dir.path().join("api-token"));
    let rec = recs.iter().rev().find(|r| r["action"] == "replay").unwrap();
    assert_eq!(
        (
            rec["actor"].as_str(),
            rec["task"].as_str(),
            rec["outcome"].as_str()
        ),
        (Some("agent"), Some("T-1"), Some("sent"))
    );

    // A human's replay is still the human's.
    let (status, _) = (r.handle(br#"{"id":"e1"}"#).await.status().as_u16(), ());
    assert_eq!(status, 200);
    assert_eq!(
        stub.replays()[1]["meta"],
        serde_json::json!({"actor": "human"})
    );
    assert_eq!(task_of(&r, "T-1").spent, 1, "not charged to the task");
}

#[tokio::test]
async fn outside_the_rules_or_scope_denies_outside_the_task_pauses() {
    let (_dir, stub, spall, r) = agent_setup(r#"{"methods":["GET","POST"]}"#).await;

    // Outside scope: a plain denial; the task keeps running.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"offsite"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("not in scope"),
        "{body}"
    );
    // Outside the RoE: the same.
    send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"methods":["GET","POST"]}}"#,
        None,
    )
    .await;
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1","method":"DELETE"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(task_of(&r, "T-1").status, spall::tasks::Status::Running);

    // An agent can't confirm, even with a token.
    let (status, body) =
        agent_send(&r, "T-1", r#"{"id":"e1","method":"POST","confirm":"abc"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("don't confirm"),
        "{body}"
    );

    // Inside the RoE and scope, but a send the task must ask for: pause.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1","method":"POST","body":"x=1"}"#).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["decision"], "paused");
    assert!(body.get("confirm").is_none(), "no token to the agent");
    let t = task_of(&r, "T-1");
    assert_eq!(t.status, spall::tasks::Status::WaitingOnApproval);
    let p = t.pending.expect("the exact request is kept");
    assert_eq!(
        (p.method.as_str(), p.target.as_str()),
        ("POST", "http://example.com/e1")
    );
    assert_eq!(
        p.request,
        serde_json::json!({"id": "e1", "method": "POST", "body": "x=1", "meta": {"actor": "agent", "task": "T-1"}})
    );
    assert!(stub.replays().is_empty(), "nothing was sent");

    // Waiting: nothing more goes out until a human decides.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("waiting_on_approval"),
        "{body}"
    );
    assert!(stub.replays().is_empty());
}

/// A role caps its task (#64): each role against each kind of send and
/// tool, through the gate, as the table in docs/ux.md (Roles) says. A send
/// the role can't make is refused, never paused: no approval widens a role.
#[tokio::test]
async fn each_role_is_refused_or_allowed_through_the_gate_as_the_table_says() {
    let (_dir, stub, spall, r) = agent_setup("{}").await;
    let eng = r.engagements.active().id;
    // T-1 is the tester; one more task per other role, running.
    for role in ["scoper", "mapper", "reporter"] {
        let res = post_text(
            spall,
            "/api/tasks",
            &format!(r#"{{"goal":"{role} work","role":"{role}"}}"#),
            None,
        )
        .await;
        let id = body_json(&res)["id"].as_str().unwrap().to_string();
        let res = send_text(
            spall,
            "PATCH",
            &format!("/api/tasks/{id}"),
            r#"{"op":"approve"}"#,
            None,
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 200"), "{res}");
        r.tasks
            .transition(&eng, &id, spall::tasks::Status::Running, None, &now())
            .unwrap();
    }
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    // (task, role, safe GET, unsafe POST)
    let table = [
        ("T-1", "tester", 200, 202),
        ("T-2", "scoper", 403, 403),
        ("T-3", "mapper", 200, 403),
        ("T-4", "reporter", 403, 403),
    ];
    for (id, role, get, post) in table {
        let before = stub.replays().len();
        let (status, body) = agent_send(&r, id, r#"{"id":"e1"}"#).await;
        assert_eq!(status, get, "{role} GET: {body}");
        let (status, body) = agent_send(&r, id, r#"{"id":"e1","method":"POST"}"#).await;
        assert_eq!(status, post, "{role} POST: {body}");
        if post == 403 {
            assert!(body["error"].as_str().unwrap().contains(role), "{body}");
            assert_eq!(
                task_of(&r, id).status,
                spall::tasks::Status::Running,
                "{role}: refused, not paused"
            );
        }
        let sent = stub.replays().len() - before;
        assert_eq!(sent, usize::from(get == 200), "{role}");
        // Tools: what the role may run, refused (and audited) otherwise.
        let out = tools.call(id, "filter", &serde_json::json!({})).await;
        assert!(!out.is_error, "{role} filter: {}", out.text);
        let out = tools.call(id, "skill", &serde_json::json!({})).await;
        assert!(
            !out.is_error && out.text.contains(role),
            "{role} skill: {}",
            out.text
        );
        let out = tools
            .call(
                id,
                "fetch",
                &serde_json::json!({"url": "http://example.com/x"}),
            )
            .await;
        let may_fetch = matches!(role, "tester" | "mapper");
        assert_eq!(
            out.text.contains("can't run fetch"),
            !may_fetch,
            "{role} fetch: {}",
            out.text
        );
        let out = tools
            .call(
                id,
                "propose_finding",
                &serde_json::json!({"exchange_id": "e1", "rule": "WSTG-INFO-02", "message": "m"}),
            )
            .await;
        assert_eq!(
            out.text.contains("doesn't propose findings"),
            role == "scoper",
            "{role}: {}",
            out.text
        );
        // Policy stays refused for every role, by admit before the role.
        let out = tools.call(id, "scope", &serde_json::json!({})).await;
        assert!(out.is_error, "{role} scope");
    }
    let refusals: Vec<_> = audit_records(&_dir.path().join("api-token"))
        .into_iter()
        .filter(|a| a["outcome"] == "denied" && a["actor"] == "agent")
        .collect();
    assert!(
        refusals
            .iter()
            .any(|a| a["task"] == "T-2" && a["reason"].as_str().unwrap_or("").contains("fetch")),
        "{refusals:?}"
    );
}

/// Verbs on objects (#67): a button drafts a pre-filled task with its cost,
/// for a human or an agent alike, and nothing runs until it's approved. A
/// retest's verdict lands on the candidate as flux's retest flow has it.
#[tokio::test]
async fn verbs_draft_prefilled_tasks_and_a_retest_marks_the_candidate() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e1","rule":"WSTG-INPV-05"}"#,
        None,
    )
    .await;
    let cid = body_json(&res)["id"].as_str().unwrap().to_string();

    // A human's button: the draft, its role and its cost.
    let res = post_text(
        spall,
        "/api/verbs/retest",
        &format!(r#"{{"id":"{cid}"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let v = body_json(&res);
    assert_eq!(v["task"]["status"], "draft");
    assert_eq!(v["task"]["role"], "tester");
    assert_eq!(v["task"]["asked_by"], "human");
    assert_eq!(
        v["task"]["constraints"]["hosts"],
        serde_json::json!(["example.com"])
    );
    assert_eq!(
        v["task"]["constraints"]["methods"],
        serde_json::json!(["GET"])
    );
    assert_eq!(v["cost"], "~5 requests, safe methods");
    assert!(v["task"]["note"].as_str().unwrap().contains("Retest on"));
    let res = post_text(
        spall,
        "/api/verbs/test_for",
        r#"{"id":"e1","category":"nope"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 400") && res.contains("ATHZ"),
        "{res}"
    );
    let res = post_text(spall, "/api/verbs/poc", r#"{"id":"e1"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 404"), "not built: {res}");
    let res = post_text(spall, "/api/verbs/why", r#"{"id":"nope"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");
    assert!(stub.replays().is_empty(), "a verb sends nothing");

    // An agent's: the same command, drafted for the tester, capped.
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let out = tools
        .call("T-1", "why", &serde_json::json!({"id": "e1"}))
        .await;
    assert!(
        !out.is_error && out.text.contains("drafted T-3"),
        "{}",
        out.text
    );
    let t3 = task_of(&r, "T-3");
    assert_eq!(
        (t3.status, t3.role, t3.asked_by.as_str()),
        (
            spall::tasks::Status::Draft,
            spall::tasks::Role::Reporter,
            "agent:T-1"
        )
    );
    for _ in 0..4 {
        assert!(
            !tools
                .call("T-1", "why", &serde_json::json!({"id": "e1"}))
                .await
                .is_error
        );
    }
    let out = tools
        .call("T-1", "why", &serde_json::json!({"id": "e1"}))
        .await;
    assert!(
        out.is_error && out.text.contains("leave the rest"),
        "{}",
        out.text
    );
    assert!(stub.replays().is_empty());

    // A retest's verdict needs a replay this task sent.
    let rr = |exchange: &str| serde_json::json!({"candidate": cid, "verdict": "fixed", "exchange_id": exchange, "note": "no error any more"});
    let out = tools.call("T-1", "retest_result", &rr("e1")).await;
    assert!(
        out.is_error && out.text.contains("isn't a request"),
        "{}",
        out.text
    );
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 200, "{body}");
    let replay = body["id"].as_str().unwrap().to_string();
    let out = tools.call("T-1", "retest_result", &rr(&replay)).await;
    assert!(!out.is_error, "{}", out.text);
    let all = body_json(&get_text(spall, "/api/candidates").await);
    let c = all["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == cid.as_str())
        .unwrap()
        .clone();
    assert_eq!(c["finding"]["properties"]["flux/status"], "fixed", "{c}");
    assert_eq!(c["finding"]["baselineState"], "absent");
    assert_eq!(c["retests"][0]["by"], "agent:T-1");
    assert_eq!(c["retests"][0]["exchange"], replay.as_str());
    assert!(c["finding"]["properties"]["flux/replayIds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x == replay.as_str()));
    let audit = audit_records(&dir.path().join("api-token"));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("retest ") && s.contains("fixed"))
        && a["task"] == "T-1"));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("retest on ") && s.contains("create T-2"))));
}

#[tokio::test]
async fn a_method_or_host_outside_the_task_pauses() {
    let (_dir, stub, _spall, r) = agent_setup(r#"{"methods":["GET"]}"#).await;
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1","method":"PUT"}"#).await;
    assert_eq!(status, 202, "{body}");
    assert!(
        body["reason"]
            .as_str()
            .unwrap()
            .contains("the task's constraints"),
        "{body}"
    );
    assert!(stub.replays().is_empty());

    let (_dir, stub, _spall, r) = agent_setup(r#"{"hosts":["example.com"]}"#).await;
    let (status, body) = agent_send(
        &r,
        "T-1",
        r#"{"id":"e1","req_headers":[["host","other.example.com"]]}"#,
    )
    .await;
    // other.example.com isn't in scope at all: the engagement denies first.
    assert_eq!(status, 403, "{body}");
    assert!(stub.replays().is_empty());
}

#[tokio::test]
async fn a_halted_finished_or_unapproved_task_cannot_send() {
    let (_dir, stub, spall, r) = agent_setup("{}").await;
    let eng = r.engagements.active().id;
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"draft","role":"mapper"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let (status, body) = agent_send(&r, "T-2", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);
    assert!(
        body["error"].as_str().unwrap().contains("T-2 is draft"),
        "{body}"
    );
    let (status, _) = agent_send(&r, "T-9", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);

    r.tasks
        .transition(
            &eng,
            "T-1",
            spall::tasks::Status::Done,
            None,
            "2026-11-23T16:00:00Z",
        )
        .unwrap();
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);
    assert!(
        body["error"].as_str().unwrap().contains("T-1 is done"),
        "{body}"
    );

    send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"halt","reason":"stop"}"#,
        None,
    )
    .await;
    let (status, body) = agent_send(&r, "T-2", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("never sends again"),
        "{body}"
    );
    assert!(stub.replays().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_budget_is_exact_under_racing_sends() {
    let (_dir, stub, spall, r) = agent_setup(r#"{"budget":5}"#).await;
    send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"rate_limit":0}}"#,
        None,
    )
    .await;
    let sends: Vec<_> = (0..20)
        .map(|_| {
            let r = r.clone();
            tokio::spawn(async move { agent_send(&r, "T-1", r#"{"id":"e1"}"#).await.0 })
        })
        .collect();
    let mut codes = Vec::new();
    for s in sends {
        codes.push(s.await.unwrap());
    }
    assert_eq!(codes.iter().filter(|c| **c == 200).count(), 5, "{codes:?}");
    assert_eq!(stub.replays().len(), 5, "exactly the budget went out");
    let t = task_of(&r, "T-1");
    assert_eq!(t.spent, 5);
    assert_eq!(
        t.status,
        spall::tasks::Status::WaitingOnApproval,
        "it asks for more"
    );
    assert!(t.reason.unwrap().contains("budget of 5"));
}

fn now() -> String {
    jiff::Timestamp::now().to_string()
}

#[tokio::test]
async fn a_task_past_its_time_limit_pauses() {
    let (_dir, stub, spall, r) = agent_setup(r#"{"time_limit_minutes":1}"#).await;
    let eng = r.engagements.active().id;
    // A second task with the same limit, started two minutes ago.
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"late","role":"mapper","constraints":{"time_limit_minutes":1}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    let past = (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(2)).to_string();
    r.tasks
        .transition(&eng, "T-2", spall::tasks::Status::Running, None, &past)
        .unwrap();

    let (status, body) = agent_send(&r, "T-2", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 202, "{body}");
    assert!(
        body["reason"]
            .as_str()
            .unwrap()
            .contains("used its time limit (1 min)"),
        "{body}"
    );
    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 200, "T-1 is inside its minute");
    assert_eq!(stub.replays().len(), 1);
}

#[tokio::test]
async fn the_e_stop_halts_every_task_revokes_tokens_and_survives_a_restart() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let token = dir.path().join("api-token");
    // A human's pending confirmation, issued before the stop.
    let res = post_text(
        spall,
        "/api/replay",
        r#"{"id":"e1","method":"DELETE"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 428"), "{res}");
    let confirm = body_json(&res)["confirm"].as_str().unwrap().to_string();

    let res = post_text(spall, "/api/estop", r#"{"reason":"weird responses"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let st = body_json(&res);
    assert_eq!(st["halted"].as_array().unwrap().len(), 1);
    assert_eq!(st["estop"]["reason"], "weird responses");
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["estop"]["by"],
        "human"
    );

    // Agents: refused, saying why; nothing sent.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("e-stop is engaged"),
        "{body}"
    );
    assert!(stub.replays().is_empty());
    let t = task_of(&r, "T-1");
    assert_eq!(
        (t.status, t.reason.as_deref()),
        (
            spall::tasks::Status::Halted,
            Some("e-stop: weird responses")
        )
    );

    // The token issued before the stop is gone; a human still replays.
    let res = post_text(
        spall,
        "/api/replay",
        &format!(r#"{{"id":"e1","method":"DELETE","confirm":"{confirm}"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "revoked: {res}");
    let res = post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await;
    assert!(
        res.starts_with("HTTP/1.1 200"),
        "human traffic is unaffected: {res}"
    );

    // No new task may run while stopped.
    post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"next","role":"mapper"}"#,
        None,
    )
    .await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("e-stop"),
        "{res}"
    );

    let recs: Vec<_> = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "estop")
        .collect();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["outcome"], "e_stop");
    assert_eq!(recs[0]["changes"]["tokens_revoked"], 1);

    // Still stopped after a restart (same data, same stub).
    let api = stub_addr(&stub);
    let (again, r2) = start_agent_spall(api, &token, &["--scope", "example.com"]).await;
    assert_eq!(
        body_json(&get_text(again, "/api/status").await)["estop"]["reason"],
        "weird responses"
    );
    let (status, _) = agent_send(&r2, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);

    // Cleared by a human: new work runs; the halted task never does.
    let res = post_text(again, "/api/estop/clear", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(body_json(&get_text(again, "/api/status").await)["estop"].is_null());
    let res = send_text(
        again,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 200"),
        "a draft made while stopped may run once a human clears it: {res}"
    );
    let (status, body) = agent_send(&r2, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("never sends again"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_uncharged_is_sent_after_the_e_stop() {
    let (_dir, stub, spall, r) = agent_setup(r#"{"budget":1000}"#).await;
    send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"rate_limit":0}}"#,
        None,
    )
    .await;
    let sends: Vec<_> = (0..40)
        .map(|_| {
            let r = r.clone();
            tokio::spawn(async move { agent_send(&r, "T-1", r#"{"id":"e1"}"#).await.0 })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let res = post_text(spall, "/api/estop", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    for s in sends {
        s.await.unwrap();
    }
    let spent = task_of(&r, "T-1").spent as usize;
    assert_eq!(
        stub.replays().len(),
        spent,
        "every send that went out was charged before the stop"
    );
    let after = stub.replays().len();
    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403);
    assert_eq!(stub.replays().len(), after);
}

async fn agent_fetch(
    r: &spall::replay::Replayer,
    task: &str,
    json: &str,
) -> (u16, serde_json::Value) {
    let res = r
        .agent_fetch(task, serde_json::from_str(json).unwrap())
        .await;
    let status = res.status().as_u16();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn an_agent_fetches_a_new_url_through_the_same_gate_as_a_replay() {
    let (dir, stub, spall, r) = agent_setup(r#"{"budget":5}"#).await;

    // In scope: sent through taphttp's /send, attributed, charged, logged.
    let (status, body) = agent_fetch(
        &r,
        "T-1",
        r#"{"url":"http://example.com/assets/","meta":{"actor":"human"}}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["id"], "s-1");
    assert_eq!(body["origin"], "send");
    assert_eq!(
        stub.sends(),
        vec![serde_json::json!({
            "method": "GET",
            "url": "http://example.com/assets/",
            "meta": {"actor": "agent", "task": "T-1"},
        })],
        "spall's attribution, never the caller's"
    );
    assert!(stub.replays().is_empty());
    let t = task_of(&r, "T-1");
    assert_eq!(t.spent, 1);
    let last = serde_json::to_value(t.log.last().unwrap()).unwrap();
    assert_eq!(
        (
            last["kind"].as_str(),
            last["outcome"].as_str(),
            last["exchange"].as_str(),
            last["target"].as_str()
        ),
        (
            Some("request"),
            Some("sent"),
            Some("s-1"),
            Some("http://example.com/assets/")
        )
    );
    let recs = audit_records(&dir.path().join("api-token"));
    let rec = recs.iter().rev().find(|a| a["action"] == "fetch").unwrap();
    assert_eq!(
        (
            rec["actor"].as_str(),
            rec["task"].as_str(),
            rec["outcome"].as_str(),
            rec["result_id"].as_str(),
            rec["target"].as_str()
        ),
        (
            Some("agent"),
            Some("T-1"),
            Some("sent"),
            Some("s-1"),
            Some("http://example.com/assets/")
        )
    );

    // HEAD too, with the agent's own headers.
    let (status, body) = agent_fetch(
        &r,
        "T-1",
        r#"{"url":"http://example.com/robots.txt","method":"HEAD","req_headers":[["accept","text/plain"]]}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        stub.sends()[1],
        serde_json::json!({
            "method": "HEAD",
            "url": "http://example.com/robots.txt",
            "req_headers": [["accept", "text/plain"]],
            "meta": {"actor": "agent", "task": "T-1"},
        })
    );

    // Only GET and HEAD, absolute http(s) URLs, valid headers: else 400,
    // nothing sent, nothing charged.
    for bad in [
        r#"{"url":"http://example.com/","method":"POST"}"#,
        r#"{"url":"http://example.com/","method":"get"}"#,
        r#"{"url":"/relative"}"#,
        r#"{"url":"ftp://example.com/"}"#,
        r#"{"url":"http://example.com/a\r\nb"}"#,
        r#"{"url":"http://example.com/","req_headers":[["bad name","x"]]}"#,
    ] {
        let (status, body) = agent_fetch(&r, "T-1", bad).await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
    assert_eq!(stub.sends().len(), 2);
    assert_eq!(task_of(&r, "T-1").spent, 2);

    // A failed attempt is recorded; both are charged (the charge comes
    // before the send, as for a replay, so the budget is exact).
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://example.com/fails"}"#).await;
    assert_eq!(status, 502, "{body}");
    assert_eq!(body["id"], "s-3");
    let (status, _) = agent_fetch(&r, "T-1", r#"{"url":"http://example.com/bad"}"#).await;
    assert_eq!(status, 400);

    // Out of scope: denied, the task keeps running.
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://evil.test/"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("not in scope"),
        "{body}"
    );
    // The stack: refused whatever the scope, not approvable.
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://spall:3000/api/status"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("can't be approved"),
        "{body}"
    );
    assert_eq!(task_of(&r, "T-1").status, spall::tasks::Status::Running);
    assert_eq!(stub.sends().len(), 4, "neither reached taphttp");

    // A human fetches from the console: the human's, not charged.
    let res = post_text(
        spall,
        "/api/fetch",
        r#"{"url":"http://example.com/x"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(
        stub.sends()[4]["meta"],
        serde_json::json!({"actor": "human"})
    );
    let res = post_text(spall, "/api/fetch", r#"{"url":"http://evil.test/"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    let res = post_text(
        spall,
        "/api/fetch",
        r#"{"url":"http://example.com/"}"#,
        Some("Content-Type: application/json\r\nOrigin: http://evil.test\r\n"),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 403"), "cross-site: {res}");
    assert_eq!(stub.sends().len(), 5);
    assert_eq!(task_of(&r, "T-1").spent, 4);
}

#[tokio::test]
async fn a_fetch_outside_the_task_pauses_and_the_e_stop_stops_it() {
    let (_dir, stub, spall, r) = agent_setup_with(
        r#"{"hosts":["example.com"]}"#,
        &["--scope", "example.com,other.test"],
        &[],
    )
    .await;
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://other.test/"}"#).await;
    assert_eq!(status, 202, "{body}");
    let p = task_of(&r, "T-1")
        .pending
        .expect("the exact request is kept");
    assert_eq!(
        (p.method.as_str(), p.target.as_str()),
        ("GET", "http://other.test/")
    );
    assert_eq!(
        p.request,
        serde_json::json!({"method": "GET", "url": "http://other.test/", "meta": {"actor": "agent", "task": "T-1"}})
    );
    assert!(stub.sends().is_empty());

    // Approved once: that exact fetch goes, once.
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://other.test/"}"#).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = agent_fetch(&r, "T-1", r#"{"url":"http://other.test/"}"#).await;
    assert_eq!(status, 202, "used up");
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"deny","reason":"no"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");

    let res = post_text(spall, "/api/estop", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let (status, body) = agent_fetch(&r, "T-1", r#"{"url":"http://example.com/"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("e-stop is engaged"),
        "{body}"
    );
    assert_eq!(stub.sends().len(), 1);
}

fn stub_addr(stub: &Stub) -> SocketAddr {
    stub.0.addr.lock().unwrap().unwrap()
}

#[tokio::test]
async fn approve_once_lets_that_exact_request_through_once() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let post = r#"{"id":"e1","method":"POST","body":"a=1"}"#;
    let (status, body) = agent_send(&r, "T-1", post).await;
    assert_eq!(status, 202, "the RoE wants POST confirmed: {body}");
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["waiting_approval"],
        1
    );
    let t = task_of(&r, "T-1");
    assert_eq!(
        serde_json::to_value(&t.pending.unwrap().ask).unwrap(),
        serde_json::json!({"kind": "confirm"})
    );

    // Only once can lift an RoE confirmation.
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_for_task"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 400") && res.contains("approve it once"),
        "{res}"
    );
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let after = body_json(&res);
    assert_eq!(after["status"], "running");
    assert!(after["grant"]["expires"].is_string());
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["waiting_approval"],
        0
    );

    // A different request isn't approved: it pauses again, nothing sent.
    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1","method":"POST","body":"a=2"}"#).await;
    assert_eq!(status, 202);
    assert!(stub.replays().is_empty());
    send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"deny","reason":"not that one"}"#,
        None,
    )
    .await;

    // The approved one goes, attributed to the agent and charged.
    let (status, body) = agent_send(&r, "T-1", post).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(stub.replays().len(), 1);
    assert_eq!(stub.replays()[0]["meta"]["task"], "T-1");
    let t = task_of(&r, "T-1");
    assert_eq!(t.spent, 1);
    assert!(t.grant.is_none(), "used up");

    // Once means once.
    let (status, _) = agent_send(&r, "T-1", post).await;
    assert_eq!(status, 202);
    assert_eq!(stub.replays().len(), 1);

    let reasons: Vec<String> = audit_records(&dir.path().join("api-token"))
        .iter()
        .filter(|r| r["action"] == "task")
        .map(|r| r["reason"].as_str().unwrap().to_string())
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r == "approve_once T-1 POST http://example.com/e1"),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r == "deny T-1 POST http://example.com/e1: not that one"),
        "{reasons:?}"
    );
    let sent = audit_records(&dir.path().join("api-token"))
        .into_iter()
        .find(|r| r["action"] == "replay" && r["outcome"] == "sent")
        .unwrap();
    assert_eq!(sent["confirmed"], true, "sent on a human's approval");
}

#[tokio::test]
async fn approve_for_the_task_widens_it_and_deny_lets_it_try_another_way() {
    let (_dir, stub, spall, r) = agent_setup(r#"{"methods":["GET"]}"#).await;
    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1","method":"HEAD"}"#).await;
    assert_eq!(status, 202);
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_for_task"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(
        body_json(&res)["constraints"]["methods"],
        serde_json::json!(["GET", "HEAD"])
    );
    for _ in 0..2 {
        let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1","method":"HEAD"}"#).await;
        assert_eq!(status, 200, "HEAD is the task's now");
    }

    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1","method":"OPTIONS"}"#).await;
    assert_eq!(status, 202);
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"deny","reason":"use GET"}"#,
        None,
    )
    .await;
    let t = body_json(&res);
    assert_eq!(
        (t["status"].as_str(), t["reason"].as_str()),
        (
            Some("running"),
            Some("OPTIONS http://example.com/e1 was denied: use GET")
        )
    );
    let (status, _) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 200, "another way works");
    assert_eq!(stub.replays().len(), 3);

    // Nothing to decide now; and never after a halt.
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    agent_send(&r, "T-1", r#"{"id":"e1","method":"OPTIONS"}"#).await;
    send_text(spall, "PATCH", "/api/tasks/T-1", r#"{"op":"halt"}"#, None).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
}

/// An MCP client over spall's streamable HTTP (#58).
struct Mcp {
    addr: SocketAddr,
    token: String,
    session: Option<String>,
    next: u64,
}

impl Mcp {
    fn new(addr: SocketAddr, token_file: &FsPath) -> Self {
        let token = std::fs::read_to_string(token_file.with_file_name("mcp-token"))
            .unwrap()
            .trim()
            .to_string();
        Mcp {
            addr,
            token,
            session: None,
            next: 0,
        }
    }

    fn headers(&self) -> String {
        let mut h = format!("Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAuthorization: Bearer {}\r\n", self.token);
        if let Some(s) = &self.session {
            h.push_str(&format!("Mcp-Session-Id: {s}\r\n"));
        }
        h
    }

    async fn raw(&self, path: &str, msg: &serde_json::Value) -> String {
        send_text(
            self.addr,
            "POST",
            path,
            &msg.to_string(),
            Some(&self.headers()),
        )
        .await
    }

    async fn initialize(&mut self, query: &str) -> String {
        let res = self
            .raw(
                &format!("/mcp{query}"),
                &serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "1"}}}),
            )
            .await;
        self.session = res.lines().find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("mcp-session-id: ")
                .map(|s| s.trim().to_string())
        });
        res
    }

    async fn rpc(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next += 1;
        let res = self
            .raw("/mcp", &serde_json::json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}))
            .await;
        assert!(res.starts_with("HTTP/1.1 200"), "{res}");
        body_json(&res)
    }

    /// A tool call: (text, is_error).
    async fn call(&mut self, name: &str, args: serde_json::Value) -> (String, bool) {
        let v = self
            .rpc(
                "tools/call",
                serde_json::json!({"name": name, "arguments": args}),
            )
            .await;
        let r = &v["result"];
        (
            r["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            r["isError"] == true,
        )
    }
}

#[tokio::test]
async fn mcp_over_http_needs_the_token_and_works_under_its_task() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let token = dir.path().join("api-token");
    let mut m = Mcp::new(spall, &token);

    // No token, a wrong one, or no session: refused.
    let init = serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}});
    let res = send_text(
        spall,
        "POST",
        "/mcp?task=T-1",
        &init.to_string(),
        Some("Content-Type: application/json\r\n"),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 401"), "{res}");
    let res = send_text(
        spall,
        "POST",
        "/mcp?task=T-1",
        &init.to_string(),
        Some("Content-Type: application/json\r\nAuthorization: Bearer nope\r\n"),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 401"), "{res}");
    let res = m
        .raw(
            "/mcp",
            &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");

    let res = m.initialize("?task=T-1").await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let v = body_json(&res);
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert!(v["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("task T-1 (now running)"));
    assert!(m.session.is_some());

    let tools = m.rpc("tools/list", serde_json::json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "filter",
            "open",
            "replay",
            "fetch",
            "sweep",
            "checks",
            "oob_payload",
            "oob_hits",
            "surface",
            "record_coverage",
            "run_in_sandbox",
            "hosts",
            "rules",
            "task",
            "propose_finding",
            "report",
            "skill",
            "why",
            "explain",
            "map_auth",
            "test_for",
            "retest",
            "variants",
            "escalate",
            "poc",
            "save_poc",
            "retest_result"
        ]
    );

    // Reads come back wrapped as untrusted, and are logged as reads.
    let (text, is_err) = m.call("open", serde_json::json!({"id": "e1"})).await;
    assert!(!is_err, "{text}");
    assert!(
        text.contains("<untrusted-") && text.contains("\"id\": \"e1\""),
        "{text}"
    );
    let (text, _) = m
        .call(
            "filter",
            serde_json::json!({"query": "host:example.com status:2xx"}),
        )
        .await;
    assert!(text.contains("exchange(s)"), "{text}");
    assert!(
        stub.exchange_queries()
            .iter()
            .any(|q| q.contains("host=example.com") && q.contains("status=2xx")),
        "{:?}",
        stub.exchange_queries()
    );

    // A safe replay inside scope goes; an unsafe one stops at approval.
    let (text, is_err) = m.call("replay", serde_json::json!({"id": "e1"})).await;
    assert!(
        !is_err && text.starts_with("sent: replay r-e1 of e1"),
        "{text}"
    );
    assert_eq!(
        stub.replays()[0]["meta"],
        serde_json::json!({"actor": "agent", "task": "T-1"})
    );
    let (text, is_err) = m
        .call(
            "replay",
            serde_json::json!({"id": "e1", "method": "POST", "yes": true}),
        )
        .await;
    assert!(
        is_err && text.starts_with("paused (yes is ignored"),
        "{text}"
    );
    assert!(text.contains("Don't retry it"), "{text}");
    assert_eq!(stub.replays().len(), 1);
    let (text, _) = m.call("task", serde_json::json!({})).await;
    assert!(
        text.starts_with("Waiting on the tester's approval"),
        "{text}"
    );
    let (text, is_err) = m.call("task", serde_json::json!({"id": "T-9"})).await;
    assert!(is_err && text.contains("only its own task"), "{text}");

    // What an agent never gets, whatever it asks.
    for name in ["scope", "promote", "export", "roe", "estop"] {
        let (text, is_err) = m.call(name, serde_json::json!({})).await;
        assert!(is_err && text.contains("Ask the tester"), "{name}: {text}");
    }
    let (text, is_err) = m.call("curl", serde_json::json!({})).await;
    assert!(is_err && text.contains("not offered to agents"), "{text}");
    let refused = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "command" && r["actor"] == "agent")
        .count();
    assert_eq!(refused, 6, "every refused tool call is audited");
    let started = audit_records(&token)
        .into_iter()
        .filter(|r| r["action"] == "mcp")
        .count();
    assert_eq!(started, 1);

    // Unknown methods, notifications and ping.
    let v = m.rpc("resources/list", serde_json::json!({})).await;
    assert_eq!(v["error"]["code"], -32601);
    let res = m
        .raw(
            "/mcp",
            &serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await;
    assert!(res.starts_with("HTTP/1.1 202"), "{res}");
    assert_eq!(
        m.rpc("ping", serde_json::json!({})).await["result"],
        serde_json::json!({})
    );

    // Ending the session.
    let res = send_text(spall, "DELETE", "/mcp", "", Some(&m.headers())).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = m
        .raw(
            "/mcp",
            &serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}),
        )
        .await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");
    let _ = r;
}

#[tokio::test]
async fn mcp_gets_the_same_host_and_origin_checks_as_the_ui() {
    let (dir, _stub, spall, _r) = agent_setup("{}").await;
    let m = Mcp::new(spall, &dir.path().join("api-token"));
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}})
        .to_string();
    // DNS rebinding: a page on evil.test resolving to 127.0.0.1.
    let res = send_text(spall, "POST", "/mcp?task=T-1", &body, Some(&m.headers())).await;
    assert!(res.starts_with("HTTP/1.1 200"), "baseline: {res}");
    let mut s = TcpStream::connect(spall).await.unwrap();
    let req = format!(
        "POST /mcp?task=T-1 HTTP/1.1\r\nHost: evil.test\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        m.headers(),
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
    // A cross-site page, even with the token somehow.
    let res = send_text(
        spall,
        "POST",
        "/mcp?task=T-1",
        &body,
        Some(&format!("{}Origin: http://evil.test\r\n", m.headers())),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    let res = send_text(
        spall,
        "POST",
        "/mcp?task=T-1",
        &body,
        Some(&format!("{}Sec-Fetch-Site: cross-site\r\n", m.headers())),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 403"), "{res}");
    let res = send_text(spall, "GET", "/mcp", "", Some(&m.headers())).await;
    assert!(res.starts_with("HTTP/1.1 405"), "{res}");
}

#[tokio::test]
async fn an_mcp_session_without_a_task_gets_a_draft_that_sends_nothing() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    let res = m.initialize("?goal=map%20the%20API&role=mapper").await;
    assert!(
        body_json(&res)["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("task T-2 (now draft)"),
        "{res}"
    );
    let t = task_of(&r, "T-2");
    assert_eq!(
        (t.goal.as_str(), t.asked_by.as_str()),
        ("map the API", "agent (claude-code)")
    );
    let (text, is_err) = m.call("replay", serde_json::json!({"id": "e1"})).await;
    assert!(is_err && text.contains("T-2 is draft"), "{text}");
    assert!(stub.replays().is_empty());
    let (text, _) = m.call("task", serde_json::json!({})).await;
    assert!(
        text.starts_with("A draft: nothing sends until the tester approves it."),
        "{text}"
    );

    // A finished task can't be bound; nor any task once agents are off.
    send_text(spall, "PATCH", "/api/tasks/T-1", r#"{"op":"halt"}"#, None).await;
    let mut m2 = Mcp::new(spall, &dir.path().join("api-token"));
    let v = body_json(&m2.initialize("?task=T-1").await);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("never runs again"),
        "{v}"
    );
    send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":false}}"#,
        None,
    )
    .await;
    let v = body_json(&m2.initialize("").await);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("don't allow agents"),
        "{v}"
    );
}

/// A session with no `?task` follows the queue: the oldest unfinished task
/// for an outside client that no other live session holds. A finished task
/// hands over to the next one without a reconnect, and the call that finds
/// the switch runs nothing.
#[tokio::test]
async fn an_mcp_session_without_a_task_follows_the_queue() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let tok = dir.path().join("api-token");
    let mut m = Mcp::new(spall, &tok);
    let v = body_json(&m.initialize("").await);
    let ins = v["result"]["instructions"].as_str().unwrap().to_string();
    assert!(ins.contains("follows the tester's queue"), "{ins}");
    assert!(ins.contains("now working T-1"), "{ins}");

    // Tools are every role's (the role can change between tasks); each
    // call is still checked against the task's role.
    let tools = m.rpc("tools/list", serde_json::json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"replay") && names.contains(&"propose_finding"));
    let mut dedup = names.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), names.len(), "no tool twice");

    // A second follower doesn't take a task another session holds.
    let mut m2 = Mcp::new(spall, &tok);
    let v = body_json(&m2.initialize("").await);
    assert!(
        v["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("no task for you yet"),
        "{v}"
    );

    // T-1 ends; a draft isn't work. The next call sends nothing.
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"second","role":"mapper"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    send_text(spall, "PATCH", "/api/tasks/T-1", r#"{"op":"halt"}"#, None).await;
    let before = stub.replays().len();
    let (text, is_err) = m.call("replay", serde_json::json!({"id": "e1"})).await;
    assert!(is_err && text.contains("T-1 is halted"), "{text}");
    assert!(text.contains("waiting for the tester"), "{text}");
    let (text, is_err) = m.call("task", serde_json::json!({})).await;
    assert!(!is_err && text.contains("waiting for the tester"), "{text}");
    assert!(!text.contains("ran nothing"), "{text}");

    // Approved: the next call introduces T-2 and still sends nothing.
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let (text, is_err) = m.call("replay", serde_json::json!({"id": "e1"})).await;
    assert!(is_err && text.contains("next task is T-2"), "{text}");
    assert!(text.contains("mapper") && text.contains("second"), "{text}");
    assert_eq!(stub.replays().len(), before, "the switching call ran");
    // From then on, calls are T-2's.
    let (text, _) = m.call("task", serde_json::json!({})).await;
    assert!(text.contains("\"id\": \"T-2\""), "{text}");
    // Asking `task` right after a task ends isn't a failed call: no error,
    // and it says the task ended rather than that the call ran nothing.
    send_text(spall, "PATCH", "/api/tasks/T-2", r#"{"op":"halt"}"#, None).await;
    let (text, is_err) = m.call("task", serde_json::json!({})).await;
    assert!(!is_err, "{text}");
    assert!(
        text.starts_with("T-2 is halted.") && !text.contains("ran nothing"),
        "{text}"
    );
    assert!(task_of(&r, "T-2")
        .log
        .iter()
        .any(|e| matches!(e.event, spall::tasks::Event::Command { .. })));
    // The other follower still has nothing: T-2 is held.
    let (text, _) = m2.call("task", serde_json::json!({})).await;
    assert!(text.contains("waiting for the tester"), "{text}");

    // A pinned session keeps working the old way.
    let mut m3 = Mcp::new(spall, &tok);
    let v = body_json(&m3.initialize("?task=T-1").await);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("never runs again"),
        "{v}"
    );
}

#[tokio::test]
async fn an_agent_fetches_over_mcp_and_what_it_fetched_is_evidence() {
    let (dir, stub, spall, _r) = agent_setup("{}").await;
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    m.initialize("?task=T-1").await;
    let (text, is_err) = m
        .call(
            "fetch",
            serde_json::json!({"url": "http://example.com/assets/", "headers": [{"name": "accept", "value": "*/*"}]}),
        )
        .await;
    assert!(!is_err, "{text}");
    assert!(
        text.starts_with("sent: fetch s-1 (GET http://example.com/assets/)"),
        "{text}"
    );
    assert!(
        text.contains("untrusted"),
        "traffic comes back wrapped: {text}"
    );
    assert_eq!(
        stub.sends()[0]["req_headers"],
        serde_json::json!([["accept", "*/*"]])
    );

    // What it fetched counts as evidence it sent.
    let (text, is_err) = m
        .call(
            "propose_finding",
            serde_json::json!({"exchange_id": "s-1", "rule": "WSTG-CONF-04", "message": "a directory listing"}),
        )
        .await;
    assert!(!is_err && text.starts_with("proposed c-"), "{text}");

    let (text, is_err) = m
        .call("fetch", serde_json::json!({"url": "http://evil.test/"}))
        .await;
    assert!(is_err && text.starts_with("refused: "), "{text}");
    let (text, is_err) = m
        .call(
            "fetch",
            serde_json::json!({"url": "http://example.com/", "method": "POST"}),
        )
        .await;
    assert!(is_err && text.contains("GET or HEAD"), "{text}");
    assert_eq!(stub.sends().len(), 1);
}

#[tokio::test]
async fn the_same_issue_in_other_words_is_seen_again_not_proposed_again() {
    let (dir, stub, spall, _r) = agent_setup("{}").await;
    stub.push("e2");
    let token = dir.path().join("api-token");
    let flag = |body: &'static str| post_text(spall, "/api/candidates", body, None);
    let list = || async {
        body_json(&get_text(spall, "/api/candidates").await)["candidates"]
            .as_array()
            .unwrap()
            .clone()
    };

    // T-3's finding (#78's fixture): a server banner, described in prose,
    // confirmed by the tester.
    let res = flag(r#"{"exchange":"e1","rule":"WSTG-INFO-02","parameter":"Server response header / error-page footer"}"#).await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let orig = body_json(&res)["id"].as_str().unwrap().to_string();
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/candidates/{orig}"),
        r#"{"op":"confirm"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");

    let mut m = Mcp::new(spall, &token);
    m.initialize("?task=T-1").await;
    m.call("open", serde_json::json!({"id": "e1"})).await;
    m.call("open", serde_json::json!({"id": "e2"})).await;

    // An agent's parameter is a name; prose goes in the message.
    let (text, is_err) = m
        .call("propose_finding", serde_json::json!({"exchange_id": "e2", "rule": "WSTG-INFO-02", "message": "nginx version", "parameter": "Server header"}))
        .await;
    assert!(is_err && text.contains("the name of"), "{text}");

    // T-4: the same banner on another path of the same host.
    let (text, is_err) = m
        .call("propose_finding", serde_json::json!({"exchange_id": "e2", "rule": "WSTG-INFO-02", "message": "nginx/1.25 in the Server header", "parameter": "Server"}))
        .await;
    assert!(
        is_err && text.starts_with(&format!("duplicate: {orig} (confirmed)")),
        "{text}"
    );
    assert!(text.contains("your evidence was added to it"), "{text}");
    assert!(text.contains("doesn't count against your cap"), "{text}");
    let l = list().await;
    assert_eq!(l.len(), 1, "no new candidate: {l:?}");
    assert_eq!(
        l[0]["finding"]["properties"]["flux/replayIds"],
        serde_json::json!(["e1", "e2"])
    );
    assert_eq!(l[0]["seen_again"][0]["by"], "agent:T-1");
    assert_eq!(
        l[0]["seen_again"][0]["exchanges"],
        serde_json::json!(["e2"])
    );
    assert_eq!(l[0]["finding"]["occurrenceCount"], 2);
    let rec = audit_records(&token)
        .into_iter()
        .rev()
        .find(|a| a["action"] == "candidate")
        .unwrap();
    assert!(
        rec["reason"]
            .as_str()
            .unwrap()
            .starts_with(&format!("seen again {orig}")),
        "{rec}"
    );
    assert_eq!(rec["task"], "T-1");
    // Again with nothing new: still the original, nothing more recorded.
    let (text, _) = m
        .call("propose_finding", serde_json::json!({"exchange_id": "e2", "rule": "WSTG-INFO-02", "message": "again", "parameter": "server"}))
        .await;
    assert!(
        text.starts_with(&format!("duplicate: {orig}"))
            && !text.contains("your evidence was added"),
        "{text}"
    );
    assert_eq!(list().await[0]["seen_again"].as_array().unwrap().len(), 1);

    // A rejected one: the agent hears why, and its sighting is kept.
    let res = flag(r#"{"exchange":"e1","rule":"WSTG-CONF-07"}"#).await;
    let hsts = body_json(&res)["id"].as_str().unwrap().to_string();
    send_text(
        spall,
        "PATCH",
        &format!("/api/candidates/{hsts}"),
        r#"{"op":"reject","justification":"HSTS is set at the CDN"}"#,
        None,
    )
    .await;
    let (text, is_err) = m
        .call(
            "propose_finding",
            serde_json::json!({"exchange_id": "e2", "rule": "WSTG-CONF-07", "message": "no HSTS"}),
        )
        .await;
    assert!(
        is_err && text.contains("set it aside") && text.contains("HSTS is set at the CDN"),
        "{text}"
    );
    assert_eq!(list().await.len(), 2);

    // Parameter rules: another path or parameter is another finding.
    for (ex, param) in [("e1", "q"), ("e2", "q"), ("e1", "id")] {
        let (text, is_err) = m
            .call("propose_finding", serde_json::json!({"exchange_id": ex, "rule": "WSTG-INPV-05", "message": "injectable", "parameter": param}))
            .await;
        assert!(
            !is_err && text.starts_with("proposed c-"),
            "{ex} {param}: {text}"
        );
    }
    assert_eq!(list().await.len(), 5);

    // A human's near-duplicate: warned, then flagged anyway, marked.
    let res = flag(r#"{"exchange":"e2","rule":"WSTG-INFO-02","parameter":"X-Powered-By"}"#).await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    let b = body_json(&res);
    assert_eq!(b["match"], "similar");
    assert_eq!(b["existing"]["id"], orig.as_str());
    assert!(
        b["error"]
            .as_str()
            .unwrap()
            .contains("possibly a duplicate"),
        "{b}"
    );
    let res =
        flag(r#"{"exchange":"e2","rule":"WSTG-INFO-02","parameter":"X-Powered-By","force":true}"#)
            .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    assert_eq!(body_json(&res)["possible_duplicate_of"], orig.as_str());
    // The exact same one is still refused, force or not.
    let res = flag(r#"{"exchange":"e1","rule":"WSTG-INFO-02","parameter":"Server response header / error-page footer","force":true}"#).await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    assert_eq!(body_json(&res)["match"], "exact");
}

#[tokio::test]
async fn an_agent_files_a_report_the_tester_keeps() {
    let (dir, _stub, spall, _r) = agent_setup("{}").await;
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    let init = body_json(&m.initialize("?task=T-1").await);
    assert!(
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.contains("report")),
        "the agent is asked for one: {init}"
    );

    let (text, is_err) = m
        .call(
            "report",
            serde_json::json!({"text": "Stack: Apache behind Cloudflare.\nNext: /cgi-bin/."}),
        )
        .await;
    assert!(!is_err && text.contains("T-1"), "{text}");
    let t = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert_eq!(
        t["report"]["text"],
        "Stack: Apache behind Cloudflare.\nNext: /cgi-bin/."
    );
    let list = body_json(&get_text(spall, "/api/tasks").await);
    assert!(
        list["tasks"][0]["report"]["text"].is_string(),
        "lists carry it"
    );
    assert!(t["log"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "command" && e["name"] == "report" && e["outcome"] == "ok"));
    let rec = audit_records(&dir.path().join("api-token"))
        .into_iter()
        .find(|r| {
            r["action"] == "task"
                && r["reason"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("report T-1"))
        })
        .expect("audited");
    assert_eq!(
        (rec["actor"].as_str(), rec["task"].as_str()),
        (Some("agent"), Some("T-1"))
    );

    let (text, is_err) = m.call("report", serde_json::json!({"text": "  "})).await;
    assert!(is_err && text.contains("empty"), "{text}");
    let (text, is_err) = m.call("report", serde_json::json!({})).await;
    assert!(is_err, "{text}");

    // Finished: the final report ends the task as done, and nothing
    // more runs under it.
    let (text, is_err) = m
        .call(
            "report",
            serde_json::json!({"text": "Final: Apache behind Cloudflare.", "done": true}),
        )
        .await;
    assert!(!is_err && text.contains("T-1 is done"), "{text}");
    let t = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert_eq!(t["status"], "done");
    assert_eq!(t["report"]["text"], "Final: Apache behind Cloudflare.");
    let (text, is_err) = m.call("report", serde_json::json!({"text": "late"})).await;
    assert!(is_err && text.contains("done"), "{text}");
    let (_, is_err) = m.call("open", serde_json::json!({"id": "e1"})).await;
    assert!(is_err, "nothing more runs under it");
    let t = body_json(&get_text(spall, "/api/tasks/T-1").await);
    assert_eq!(t["report"]["text"], "Final: Apache behind Cloudflare.");
}

#[tokio::test]
async fn an_agent_proposes_findings_only_with_real_evidence_and_a_human_decides() {
    let (dir, stub, spall, _r) = agent_setup("{}").await;
    stub.push("e2");
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    m.initialize("?task=T-1").await;
    let propose = |ex: &str, param: &str| serde_json::json!({"exchange_id": ex, "rule": "WSTG-INPV-05", "message": "id looks injectable", "parameter": param, "level": "warning"});

    // Evidence must be traffic this task sent or read.
    let (text, is_err) = m.call("propose_finding", propose("e1", "id")).await;
    assert!(
        is_err && text.contains("sent or read") && text.contains("e1"),
        "{text}"
    );
    m.call("open", serde_json::json!({"id": "e1"})).await;
    let (text, is_err) = m.call("propose_finding", serde_json::json!({"exchange_id": "e1", "rule": "WSTG-INPV-05", "message": "m", "evidence": ["e2"]})).await;
    assert!(is_err && text.contains(": e2"), "{text}");

    let (text, is_err) = m.call("propose_finding", propose("e1", "id")).await;
    assert!(!is_err && text.starts_with("proposed c-"), "{text}");
    let list = body_json(&get_text(spall, "/api/candidates").await);
    let c = &list["candidates"][0];
    assert_eq!(c["flagged_by"], "agent:T-1");
    assert_eq!(
        c["finding"]["properties"]["flux/status"], "candidate",
        "a candidate, for a human"
    );
    let cid = c["id"].as_str().unwrap().to_string();
    let rec = audit_records(&dir.path().join("api-token"))
        .into_iter()
        .find(|r| r["action"] == "candidate")
        .unwrap();
    assert_eq!(
        (rec["actor"].as_str(), rec["task"].as_str()),
        (Some("agent"), Some("T-1"))
    );
    assert!(rec["reason"].as_str().unwrap().starts_with("propose c-"));

    // The same thing again: the agent learns it's a duplicate.
    let (text, is_err) = m.call("propose_finding", propose("e1", "id")).await;
    assert!(
        is_err && text.starts_with(&format!("duplicate: {cid}")),
        "{text}"
    );
    // Rejected by the tester: the agent is told not to raise it again.
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/candidates/{cid}"),
        r#"{"op":"reject","justification":"parameterized query"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let (text, is_err) = m.call("propose_finding", propose("e1", "id")).await;
    assert!(
        is_err && text.contains("set it aside") && text.contains("parameterized query"),
        "{text}"
    );

    // Triaged like a flagged one; promote and export stay a human's.
    let other = {
        let (t, _) = m.call("propose_finding", propose("e1", "q")).await;
        t
    };
    let cid2 = other.split_whitespace().nth(1).unwrap().to_string();
    for name in ["promote", "export"] {
        let (text, is_err) = m
            .call(name, serde_json::json!({"candidate_id": cid2}))
            .await;
        assert!(
            is_err && text.contains("agents may never run export commands"),
            "{text}"
        );
    }
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/candidates/{cid2}"),
        r#"{"op":"confirm"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");

    // A per-task cap on proposals.
    for i in 2..spall::agent::MAX_PROPOSALS {
        let (text, is_err) = m
            .call("propose_finding", propose("e1", &format!("p{i}")))
            .await;
        assert!(!is_err, "{i}: {text}");
    }
    let (text, is_err) = m.call("propose_finding", propose("e1", "one-more")).await;
    assert!(is_err && text.contains("cap per task"), "{text}");
}

#[tokio::test]
async fn injected_instructions_in_traffic_stay_data_and_change_nothing() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let corpus: Vec<String> = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/injection-corpus.txt"
    ))
    .unwrap()
    .lines()
    .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    .map(String::from)
    .collect();
    assert!(corpus.len() >= 10);
    for (i, p) in corpus.iter().enumerate() {
        let id = format!("inj{i}");
        let mut e: serde_json::Value = serde_json::from_str(&entry(&id)).unwrap();
        e["res_body"] = p.clone().into();
        e["res_headers"] = serde_json::json!([["x-note", p], ["content-type", "text/html"]]);
        e["url"] = format!("http://example.com/{id}?next={}", p.replace(' ', "+")).into();
        stub.push_json(&id, e.to_string());
    }
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    m.initialize("?task=T-1").await;
    let scope_before = r.engagements.scope().entries();
    for (i, p) in corpus.iter().enumerate() {
        let (text, is_err) = m
            .call("open", serde_json::json!({"id": format!("inj{i}")}))
            .await;
        assert!(!is_err, "{text}");
        // Exactly one wrapper, with one nonce, and the payload inside it.
        let opens: Vec<&str> = text
            .match_indices("<untrusted-")
            .map(|(k, _)| &text[k + 11..k + 27])
            .collect();
        let closes: Vec<&str> = text
            .match_indices("</untrusted-")
            .map(|(k, _)| &text[k + 12..k + 28])
            .collect();
        assert_eq!((opens.len(), closes.len()), (1, 1), "{p}\n{text}");
        assert_eq!(opens[0], closes[0]);
        let start = text.find("<untrusted-").unwrap();
        let end = text.find("</untrusted-").unwrap();
        assert!(text.starts_with(spall::untrusted::PREAMBLE));
        let probe = p
            .split_whitespace()
            .last()
            .unwrap()
            .replace("untrusted-", "untrusted_");
        let inside = &text[start..end];
        assert!(
            inside.contains(&probe.replace('"', "\\\"")) || inside.contains(&probe),
            "{probe} kept as data: {inside}"
        );
        assert!(
            !text[end..].contains(&probe) || probe.len() < 3,
            "nothing after the wrapper"
        );
    }
    // And whatever the payloads ask for, the server refuses.
    for (name, args) in [
        ("scope", serde_json::json!({"op": "all", "on": true})),
        (
            "roe",
            serde_json::json!({"op": "set", "field": "agents", "value": "yes"}),
        ),
        ("estop", serde_json::json!({"clear": true})),
        ("promote", serde_json::json!({"candidate_id": "c-00000000"})),
        ("export", serde_json::json!({"unredacted": true})),
    ] {
        let (text, is_err) = m.call(name, args).await;
        assert!(is_err, "{name}: {text}");
    }
    let (text, is_err) = m.call("replay", serde_json::json!({"id": "offsite"})).await;
    assert!(
        is_err && text.starts_with("refused"),
        "evil.test stays out of scope: {text}"
    );
    let (text, is_err) = m
        .call(
            "replay",
            serde_json::json!({"id": "e1", "yes": true, "method": "DELETE"}),
        )
        .await;
    assert!(
        is_err && text.starts_with("paused"),
        "yes confirms nothing: {text}"
    );
    assert_eq!(
        r.engagements.scope().entries(),
        scope_before,
        "scope unchanged"
    );
    assert!(stub.replays().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn claude_code_over_stdio_reads_replays_and_proposes_and_an_unsafe_send_waits() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (dir, stub, spall, _r) = agent_setup("{}").await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_spall"))
        .args([
            "mcp",
            "--url",
            &format!("http://127.0.0.1:{}", spall.port()),
            "--task",
            "T-1",
        ])
        .env("SPALL_MCP_TOKEN_FILE", dir.path().join("mcp-token"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    async fn rpc(
        stdin: &mut tokio::process::ChildStdin,
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        msg: serde_json::Value,
        answer: bool,
    ) -> serde_json::Value {
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
        if !answer {
            return serde_json::Value::Null;
        }
        let got = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&got).unwrap()
    }
    let init = rpc(&mut stdin, &mut lines, serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2"}}}), true).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    rpc(
        &mut stdin,
        &mut lines,
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        false,
    )
    .await;
    let call = |id: u64, name: &str, args: serde_json::Value| serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let text = |v: &serde_json::Value| {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let v = rpc(
        &mut stdin,
        &mut lines,
        call(2, "filter", serde_json::json!({"query": "example.com"})),
        true,
    )
    .await;
    assert!(text(&v).contains("e1"), "{v}");
    let v = rpc(
        &mut stdin,
        &mut lines,
        call(3, "open", serde_json::json!({"id": "e1"})),
        true,
    )
    .await;
    assert!(text(&v).contains("<untrusted-"), "{v}");
    let v = rpc(
        &mut stdin,
        &mut lines,
        call(4, "replay", serde_json::json!({"id": "e1"})),
        true,
    )
    .await;
    assert!(text(&v).starts_with("sent"), "{v}");
    let v = rpc(&mut stdin, &mut lines, call(5, "propose_finding", serde_json::json!({"exchange_id": "e1", "rule": "WSTG-INFO-05", "message": "comments leak paths"})), true).await;
    assert!(text(&v).starts_with("proposed c-"), "{v}");
    let v = rpc(
        &mut stdin,
        &mut lines,
        call(
            6,
            "replay",
            serde_json::json!({"id": "e1", "method": "PUT"}),
        ),
        true,
    )
    .await;
    assert_eq!(v["result"]["isError"], true);
    assert!(text(&v).starts_with("paused"), "{v}");
    assert_eq!(stub.replays().len(), 1, "the unsafe one waits for a human");

    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let ended = audit_records(&dir.path().join("api-token"))
        .into_iter()
        .any(|r| {
            r["action"] == "mcp"
                && r["reason"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("session ended")
        });
    assert!(ended, "the bridge ends its session on EOF");
}

#[tokio::test]
async fn an_approved_task_starts_with_its_agent_and_steering_reaches_it() {
    let (dir, _stub, spall, r) = agent_setup("{}").await;
    post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"second","role":"mapper"}"#,
        None,
    )
    .await;
    send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    let mut m = Mcp::new(spall, &dir.path().join("api-token"));
    m.initialize("?task=T-2").await;
    let (text, _) = m.call("task", serde_json::json!({})).await;
    assert!(
        text.starts_with("Approved: it starts with your first action"),
        "reading its task doesn't start it, nor say it's waiting on the tester: {text}"
    );
    assert_eq!(task_of(&r, "T-2").status, spall::tasks::Status::Approved);
    let (_, is_err) = m.call("open", serde_json::json!({"id": "e1"})).await;
    assert!(!is_err);
    let t = task_of(&r, "T-2");
    assert_eq!(t.status, spall::tasks::Status::Running);
    assert!(t.started.is_some());

    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"note","text":"skip password reset; out of scope this week"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-2",
        r#"{"op":"note","text":"  "}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 400"), "{res}");
    let (text, _) = m.call("task", serde_json::json!({})).await;
    assert!(
        text.contains("skip password reset; out of scope this week"),
        "{text}"
    );
}

// ---- spall as agent host (#63): a scripted model drives a task ----

use spall::model::{Call, Conversation, Model, ModelError, Stop, ToolDef, Turn, Usage};

enum Step {
    Turn(Turn),
    /// A model call that never comes back (until something stops it).
    Hang,
}

/// A model that plays back recorded turns and keeps what it was shown.
#[derive(Default)]
struct Scripted {
    steps: Mutex<std::collections::VecDeque<Step>>,
    /// The conversation as it stood at each turn.
    seen: Mutex<Vec<Conversation>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Arc<Scripted> {
        Arc::new(Scripted {
            steps: Mutex::new(steps.into()),
            seen: Mutex::default(),
        })
    }
    fn seen(&self) -> Vec<Conversation> {
        self.seen.lock().unwrap().clone()
    }
}

impl Model for Scripted {
    fn describe(&self) -> String {
        "scripted".into()
    }
    fn start(&self, text: &str) -> Conversation {
        Conversation {
            messages: vec![serde_json::json!({"role": "user", "content": text})],
        }
    }
    fn turn<'a>(
        &'a self,
        _system: &'a str,
        tools: &'a [ToolDef],
        c: &'a mut Conversation,
    ) -> spall::model::BoxFuture<'a, Result<Turn, ModelError>> {
        // An agent's tools, or the intake's (#65).
        assert!(tools
            .iter()
            .any(|t| t.name == "replay" || t.name == "plan_retest"));
        assert!(
            !tools
                .iter()
                .any(|t| t.name == "scope" || t.name == "promote"),
            "policy and export are never offered"
        );
        self.seen.lock().unwrap().push(c.clone());
        let step = self.steps.lock().unwrap().pop_front();
        Box::pin(async move {
            match step {
                Some(Step::Turn(t)) => {
                    c.messages.push(serde_json::json!({
                        "role": "assistant",
                        "text": t.text,
                        "calls": t.calls.iter().map(|x| &x.name).collect::<Vec<_>>(),
                    }));
                    Ok(t)
                }
                Some(Step::Hang) | None => std::future::pending().await,
            }
        })
    }
    fn answer(
        &self,
        c: &mut Conversation,
        results: &[(String, spall::agent::Output)],
        note: Option<&str>,
    ) {
        c.messages.push(serde_json::json!({
            "role": "user",
            "results": results.iter().map(|(id, o)| serde_json::json!({"id": id, "text": o.text, "error": o.is_error})).collect::<Vec<_>>(),
            "note": note,
        }));
    }
}

fn call(id: &str, name: &str, input: serde_json::Value) -> Call {
    Call {
        id: id.into(),
        name: name.into(),
        input: Ok(input),
    }
}

fn turn(text: &str, calls: Vec<Call>) -> Step {
    let stop = if calls.is_empty() {
        Stop::EndTurn
    } else {
        Stop::ToolUse
    };
    Step::Turn(Turn {
        text: text.into(),
        calls,
        stop,
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
        },
        note: None,
    })
}

/// spall with `model` as its agent host, and T-1 made for it (run by
/// spall) and approved: the host starts it.
async fn host_setup(
    model: Arc<Scripted>,
    constraints: &str,
) -> (
    tempfile::TempDir,
    Stub,
    SocketAddr,
    Arc<spall::replay::Replayer>,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let spall = listener.local_addr().unwrap();
    let app = spall::build(spall_config(Some(api), &token, &["--scope", "example.com"]))
        .unwrap()
        .with_model(model);
    let r = app.replayer.clone().unwrap();
    tokio::spawn(app.run(listener));
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/tasks",
        &format!(r#"{{"goal":"replay e1","role":"tester","runner":"spall","constraints":{constraints},"note":"start with e1"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    assert_eq!(body_json(&res)["runner"], "spall");
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    (dir, stub, spall, r)
}

fn task_now(r: &spall::replay::Replayer) -> spall::tasks::Task {
    r.tasks.get(&r.engagements.active().id, "T-1").unwrap()
}

#[tokio::test]
async fn a_spall_run_task_keeps_its_report_or_its_closing_answer() {
    // Filed: kept, not replaced by the closing line.
    let model = Scripted::new(vec![
        turn(
            "",
            vec![call(
                "c1",
                "report",
                serde_json::json!({"text": "Stack: nginx.\nLeft: /admin."}),
            )],
        ),
        turn("Filed my report.", vec![]),
    ]);
    let (_dir, _stub, _spall, r) = host_setup(model.clone(), "{}").await;
    eventually_async("done", || async {
        task_now(&r).status == spall::tasks::Status::Done
    })
    .await;
    assert_eq!(
        task_now(&r).report.unwrap().text,
        "Stack: nginx.\nLeft: /admin."
    );
    let told = serde_json::to_string(&model.seen()[1]).unwrap();
    assert!(told.contains("filed: the tester sees it on T-1"), "{told}");

    // Forgotten: the closing answer becomes the report.
    let model = Scripted::new(vec![turn("  Done: e1 answers 200 from nginx.  ", vec![])]);
    let (_dir, _stub, _spall, r) = host_setup(model, "{}").await;
    eventually_async("done", || async {
        task_now(&r).status == spall::tasks::Status::Done
    })
    .await;
    assert_eq!(
        task_now(&r).report.unwrap().text,
        "Done: e1 answers 200 from nginx."
    );
}

#[tokio::test]
async fn spall_runs_a_task_end_to_end_through_the_gate_and_waits_for_approval() {
    let model = Scripted::new(vec![
        turn(
            "Looking at e1.",
            vec![
                call("c1", "task", serde_json::json!({})),
                call("c2", "open", serde_json::json!({"id": "e1"})),
            ],
        ),
        turn(
            "",
            vec![call("c3", "replay", serde_json::json!({"id": "e1"}))],
        ),
        turn(
            "",
            vec![call(
                "c4",
                "replay",
                serde_json::json!({"id": "e1", "method": "POST"}),
            )],
        ),
        turn(
            "",
            vec![call(
                "c5",
                "replay",
                serde_json::json!({"id": "e1", "method": "POST"}),
            )],
        ),
        turn("Done: e1 replayed, and as a POST once approved.", vec![]),
    ]);
    let (dir, stub, spall, r) = host_setup(model.clone(), "{}").await;
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["agent_host"]["model"],
        "scripted"
    );

    // The POST is outside the task (GET only without asking): it pauses,
    // and the model isn't called again while it waits.
    eventually_async("paused", || async {
        task_now(&r).status == spall::tasks::Status::WaitingOnApproval
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(model.seen().len(), 3, "no model call while waiting");
    assert_eq!(stub.replays().len(), 1);
    assert_eq!(
        stub.replays()[0]["meta"],
        serde_json::json!({"actor": "agent", "task": "T-1"})
    );

    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    eventually_async("done", || async {
        task_now(&r).status == spall::tasks::Status::Done
    })
    .await;
    let replays = stub.replays();
    assert_eq!(replays.len(), 2);
    assert_eq!(replays[1]["method"], "POST");

    // What the model was shown: the task, the untrusted exchange, the
    // pause, then the tester's decision before it tried again.
    let seen = model.seen();
    let first = seen[0].messages[0]["content"].as_str().unwrap();
    assert!(
        first.starts_with("Task T-1: replay e1") && first.contains("start with e1"),
        "{first}"
    );
    let opened = serde_json::to_string(&seen[1].messages).unwrap();
    assert!(opened.contains("untrusted"), "{opened}");
    let paused = serde_json::to_string(&seen[3].messages).unwrap();
    assert!(paused.contains("paused"), "{paused}");
    assert!(
        paused.contains("approved once"),
        "the decision is news: {paused}"
    );

    let t = task_now(&r);
    assert_eq!(t.spent, 2);
    assert_eq!(
        t.usage,
        spall::tasks::ModelUsage {
            calls: 5,
            input_tokens: 500,
            output_tokens: 50,
            model_ms: t.usage.model_ms
        }
    );
    let turns: Vec<String> = t
        .log
        .iter()
        .filter_map(|e| match &e.event {
            spall::tasks::Event::Turn { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns,
        [
            "Looking at e1.",
            "Done: e1 replayed, and as a POST once approved."
        ]
    );
    let transcript = dir
        .path()
        .join("engagements/tasks/transcripts")
        .join(&t.engagement)
        .join("T-1.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&transcript).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    let saved: Conversation = serde_json::from_slice(&std::fs::read(&transcript).unwrap()).unwrap();
    assert!(saved.messages.len() > seen[4].messages.len());
}

#[tokio::test]
async fn the_e_stop_ends_a_model_call_in_flight() {
    let model = Scripted::new(vec![
        turn(
            "",
            vec![call("c1", "replay", serde_json::json!({"id": "e1"}))],
        ),
        Step::Hang,
    ]);
    let (_dir, stub, spall, r) = host_setup(model.clone(), "{}").await;
    eventually("second model call", || model.seen().len() == 2).await;
    let res = post_text(spall, "/api/estop", r#"{"reason":"stop it"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let t = task_now(&r);
    assert_eq!(t.status, spall::tasks::Status::Halted);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(model.seen().len(), 2);
    assert_eq!(stub.replays().len(), 1);
    assert_eq!(
        task_now(&r).usage.calls,
        1,
        "the dropped call was never answered"
    );
}

#[tokio::test]
async fn a_task_halts_at_its_model_token_budget() {
    let model = Scripted::new(vec![
        turn("", vec![call("c1", "task", serde_json::json!({}))]),
        turn("", vec![call("c2", "task", serde_json::json!({}))]),
        turn(
            "",
            vec![call("c3", "replay", serde_json::json!({"id": "e1"}))],
        ),
    ]);
    let (_dir, stub, _spall, r) = host_setup(model.clone(), r#"{"tokens":150}"#).await;
    eventually_async("halted", || async {
        task_now(&r).status == spall::tasks::Status::Halted
    })
    .await;
    let t = task_now(&r);
    assert!(t.reason.unwrap().contains("token budget"));
    assert_eq!(t.usage.calls, 2);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(model.seen().len(), 2);
    assert!(stub.replays().is_empty());
}

#[tokio::test]
async fn without_a_model_spall_wont_take_a_task_itself() {
    let (_dir, _stub, spall, _r) = agent_setup("{}").await;
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["agent_host"],
        serde_json::Value::Null
    );
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"g","role":"tester","runner":"spall"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "{res}");
    assert!(res.contains("--model-provider"), "{res}");
    let res = post_text(spall, "/api/tasks", r#"{"goal":"g","role":"tester"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    assert_eq!(body_json(&res)["runner"], "external");
}

/// Quad (#66): 4× the task's rate, never past the RoE's; logged on each
/// live task with its workers; the e-stop ends it; and the scorecard and
/// its shareable card carry numbers only, never an engagement's words.
#[tokio::test]
async fn quad_boosts_inside_the_engagement_and_its_card_is_numbers_only() {
    let (dir, _stub, spall, r) = agent_setup(r#"{"rate_limit":1}"#).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"name":"EvilCorp secret retest","client":"Evil Corp Ltd","rules":{"rate_limit":10}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let eng = r.engagements.active().id;
    let new_task = |rate: &str| {
        let (r, eng) = (r.clone(), eng.clone());
        let body =
            format!(r#"{{"goal":"go","role":"tester","constraints":{{"rate_limit":{rate}}}}}"#);
        async move {
            let res = post_text(spall, "/api/tasks", &body, None).await;
            let id = body_json(&res)["id"].as_str().unwrap().to_string();
            let res = send_text(
                spall,
                "PATCH",
                &format!("/api/tasks/{id}"),
                r#"{"op":"approve"}"#,
                None,
            )
            .await;
            assert!(res.starts_with("HTTP/1.1 200"), "{res}");
            r.tasks
                .transition(&eng, &id, spall::tasks::Status::Running, None, &now())
                .unwrap();
            id
        }
    };
    let quick = |id: String, n: usize| {
        let r = r.clone();
        async move {
            let mut codes = Vec::new();
            for _ in 0..n {
                codes.push(agent_send(&r, &id, r#"{"id":"e1"}"#).await.0);
            }
            codes
        }
    };

    // Bad asks are refused, and audited.
    let res = post_text(spall, "/api/quad", r#"{"seconds":601}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 400"), "{res}");

    let t2 = new_task("1").await;
    let res = post_text(spall, "/api/quad", r#"{"seconds":60}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let q = body_json(&res)["quad"].clone();
    assert_eq!(q["workers"], 8);
    assert!(q["capped"].is_null(), "1/s ×4 is under the RoE's 10: {q}");
    let res = post_text(spall, "/api/quad", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 409"), "one at a time: {res}");
    let status = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(status["quad"]["workers"], 8);
    assert!(status["quad"]["remaining_secs"].as_i64().unwrap() > 50);
    // T-2 (1/s) sends 4 at once in quad, and no more.
    assert_eq!(quick(t2.clone(), 5).await, [200, 200, 200, 200, 429]);
    // The start is on every live task's log, with its workers.
    let log_has = |id: &str, phase: &str| {
        task_of(&r, id).log.iter().any(|e| {
            matches!(&e.event, spall::tasks::Event::Quad { phase: p, workers: 8, .. } if p == phase)
        })
    };
    assert!(log_has("T-1", "start") && log_has(&t2, "start"));

    let res = post_text(spall, "/api/quad", r#"{"stop":true}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let card = body_json(&res)["scorecard"].clone();
    assert_eq!(card["requests"], 4, "{card}");
    assert_eq!(card["ended"], "stopped");
    assert!(log_has("T-1", "end") && log_has(&t2, "end"));
    // Out of quad, a fresh 1/s task sends one, then waits.
    let t3 = new_task("1").await;
    assert_eq!(quick(t3, 2).await, [200, 429]);

    // Capped: 5/s ×4 would pass the RoE's 10, and the start says so.
    new_task("5").await;
    let res = post_text(spall, "/api/quad", "{}", None).await;
    let q = body_json(&res)["quad"].clone();
    assert!(
        q["capped"]
            .as_str()
            .unwrap()
            .contains("10 sends per second"),
        "{q}"
    );
    assert_eq!(q["seconds"], 120, "the default");

    // The e-stop ends it at once, and no quad starts while it's engaged.
    let res = post_text(spall, "/api/estop", r#"{"reason":"stop"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let status = body_json(&get_text(spall, "/api/status").await);
    assert!(status["quad"].is_null(), "{status}");
    assert_eq!(status["last_quad"]["ended"], "estop");
    let res = post_text(spall, "/api/quad", "{}", None).await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("e-stop"),
        "{res}"
    );
    let res = send_text(spall, "POST", "/api/estop/clear", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    // Every task is halted now: halted agents can't go quad.
    let res = post_text(spall, "/api/quad", "{}", None).await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("never goes quad"),
        "{res}"
    );

    // Nothing of the engagement reaches the card.
    let text = status["last_quad"]["card"].as_str().unwrap().to_string();
    let e = r.engagements.active();
    for secret in [
        e.id.as_str(),
        e.name.as_str(),
        "Evil",
        "evil",
        "example.com",
        "/e1",
        "e1",
    ] {
        assert!(!text.contains(secret), "{secret:?} in {text}");
    }
    assert!(
        text.starts_with("QUAD ") && text.contains("e-stopped"),
        "{text}"
    );
    let audit = audit_records(&dir.path().join("api-token"));
    let quads: Vec<_> = audit.iter().filter(|a| a["outcome"] == "quad").collect();
    assert!(
        quads.iter().any(|a| a["reason"]
            .as_str()
            .unwrap()
            .starts_with("start: 60s, 8 workers")),
        "{quads:?}"
    );
    assert!(quads.iter().any(|a| a["error"]
        .as_str()
        .is_some_and(|s| s.contains("600 seconds"))));
}

/// Quad's workers (#66): spall's host drives 2 tasks at once, 8 in quad.
#[tokio::test]
async fn quad_gives_spalls_host_four_times_the_workers() {
    let (_dir, _stub, spall, r) = host_setup(Scripted::new(vec![]), "{}").await;
    for _ in 0..4 {
        let res = post_text(
            spall,
            "/api/tasks",
            r#"{"goal":"more","role":"tester","runner":"spall"}"#,
            None,
        )
        .await;
        let id = body_json(&res)["id"].as_str().unwrap().to_string();
        let res = send_text(
            spall,
            "PATCH",
            &format!("/api/tasks/{id}"),
            r#"{"op":"approve"}"#,
            None,
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    }
    let running = || {
        r.tasks
            .list(&r.engagements.active().id)
            .iter()
            .filter(|t| t.status == spall::tasks::Status::Running)
            .count()
    };
    eventually("two workers", || running() == 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(running(), 2, "no more than two outside quad");
    let res = post_text(spall, "/api/quad", r#"{"seconds":30}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    eventually("all five in quad", || running() == 5).await;
}

/// A resolver whose answers a test sets (#65).
#[derive(Default)]
struct FakeDns(Mutex<std::collections::HashMap<String, Vec<String>>>);

impl FakeDns {
    fn set(&self, host: &str, ips: &[&str]) {
        self.0
            .lock()
            .unwrap()
            .insert(host.into(), ips.iter().map(|s| s.to_string()).collect());
    }
}

impl spall::addresses::Resolve for FakeDns {
    fn lookup<'a>(&'a self, host: &'a str) -> spall::model::BoxFuture<'a, Vec<String>> {
        let ips = self
            .0
            .lock()
            .unwrap()
            .get(host)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { ips })
    }
}

/// The acceptance test for the intake (#65, docs/ux.md): "My boss said to
/// re-scan the findings from last week's evilcorp.com test." Resolve the
/// engagement; refuse while its window is closed and draft an extension
/// instead; ask when it's ambiguous; flag a host that resolves elsewhere
/// now; plan one retest per finding; nothing runs until approved; then
/// through the gate, a non-reproducing one is marked fixed with the retest
/// exchange as evidence, and who asked is on the task.
#[tokio::test]
async fn my_boss_said_to_re_scan_last_weeks_findings() {
    const ASK: &str = "My boss said to re-scan the findings from last week's evilcorp.com test.";
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    stub.push("e2");
    let dns = Arc::new(FakeDns::default());
    dns.set("example.com", &["93.184.215.14"]);
    let model = Scripted::new(vec![]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let spall = listener.local_addr().unwrap();
    let app = spall::build(spall_config(Some(api), &token, &["--scope", "example.com"]))
        .unwrap()
        .with_resolver(dns.clone())
        .with_model(model.clone());
    let r = app.replayer.clone().unwrap();
    tokio::spawn(app.run(listener));
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"name":"evilcorp.com web test","client":"EvilCorp","rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let eng = r.engagements.active().id;
    let mut cands = Vec::new();
    for (x, rule) in [("e1", "WSTG-INPV-05"), ("e2", "WSTG-INPV-01")] {
        let res = post_text(
            spall,
            "/api/candidates",
            &format!(r#"{{"exchange":"{x}","rule":"{rule}"}}"#),
            None,
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 201"), "{res}");
        cands.push(body_json(&res)["id"].as_str().unwrap().to_string());
    }
    // Where the host resolved when its findings were made.
    eventually("the address baseline", || {
        r.addresses.get(&eng, "example.com").is_some()
    })
    .await;
    // Last week's test: its window closed yesterday.
    let today = jiff::Zoned::now().date();
    let day = |n: i64| {
        today
            .checked_add(jiff::Span::new().days(n))
            .unwrap()
            .to_string()
    };
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        &format!(r#"{{"starts":"{}","ends":"{}"}}"#, day(-8), day(-1)),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");

    let intake = |body: String| async move {
        let res = post_text(spall, "/api/intake", &body, None).await;
        assert!(res.starts_with("HTTP/1.1 200"), "{res}");
        body_json(&res)
    };
    let push = |steps: Vec<Step>| model.steps.lock().unwrap().extend(steps);

    // 1. Resolve, then check: the window has closed, so nothing is planned
    // and an extension is drafted for a human.
    push(vec![
        turn("", vec![call("1", "engagements", serde_json::json!({}))]),
        turn(
            "",
            vec![call(
                "2",
                "plan_retest",
                serde_json::json!({"engagement": eng}),
            )],
        ),
        turn(
            "",
            vec![call(
                "3",
                "done",
                serde_json::json!({"summary": "Its window has closed: extend it first."}),
            )],
        ),
    ]);
    let a = intake(serde_json::json!({ "text": ASK }).to_string()).await;
    assert_eq!(a["kind"], "plan", "{a}");
    let p = &a["plan"];
    assert_eq!(p["tasks"].as_array().unwrap().len(), 0, "{p}");
    assert_eq!(p["proposal"]["kind"], "extension");
    assert_eq!(p["proposal"]["engagement"], eng.as_str());
    let w = p["warnings"][0].as_str().unwrap();
    assert!(
        w.contains("isn't authorized now") && w.contains("context, not authorization"),
        "{w}"
    );
    // The model saw what the engagement said, and the refusal.
    let seen = serde_json::to_string(&model.seen().last().unwrap().messages).unwrap();
    assert!(
        seen.contains("evilcorp.com web test") && seen.contains("refused"),
        "{seen}"
    );
    let id = p["id"].as_str().unwrap().to_string();
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/plans/{id}"),
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("no tasks"),
        "{res}"
    );

    // A human extends it (policy: theirs). Meanwhile the host moved.
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        &format!(r#"{{"ends":"{}"}}"#, day(6)),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    dns.set("example.com", &["203.0.113.50"]);

    // 2. Ambiguity is a pick list, never a guess.
    push(vec![turn(
        "",
        vec![call(
            "1",
            "ask",
            serde_json::json!({"question": "Retest which findings?", "options": ["all open findings", "only the SQL injection"]}),
        )],
    )]);
    let a = intake(serde_json::json!({ "text": ASK }).to_string()).await;
    assert_eq!(
        a["plan"]["choice"]["options"][0], "all open findings",
        "{a}"
    );
    assert!(a["plan"]["tasks"].as_array().unwrap().is_empty());

    // 3. Picked: one retest task per finding, the moved host flagged.
    push(vec![
        turn(
            "",
            vec![call(
                "1",
                "plan_retest",
                serde_json::json!({"engagement": eng}),
            )],
        ),
        turn(
            "",
            vec![call(
                "2",
                "done",
                serde_json::json!({"summary": "Two retests."}),
            )],
        ),
    ]);
    let a = intake(
        serde_json::json!({ "text": ASK, "pick": {"question": "Retest which findings?", "answer": "all open findings"} }).to_string(),
    )
    .await;
    let opening = serde_json::to_string(&model.seen().last().unwrap().messages[0]).unwrap();
    assert!(
        opening.contains("The tester picked: all open findings"),
        "{opening}"
    );
    let p = a["plan"].clone();
    let tasks = p["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{p}");
    let planned: Vec<&str> = tasks
        .iter()
        .map(|t| t["candidate"].as_str().unwrap())
        .collect();
    assert_eq!(
        planned,
        cands.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for t in tasks {
        assert_eq!(t["task"]["role"], "tester");
        assert_eq!(
            t["task"]["constraints"]["methods"],
            serde_json::json!(["GET"])
        );
        assert!(t["task"]["note"].as_str().unwrap().contains(ASK));
    }
    assert!(
        p["warnings"].as_array().unwrap().iter().any(|w| {
            let w = w.as_str().unwrap();
            w.contains("example.com resolves to 203.0.113.50 now, not 93.184.215.14")
        }),
        "{p}"
    );
    let plans = body_json(&get_text(spall, "/api/plans").await);
    let listed = plans["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == p["id"])
        .unwrap()
        .clone();
    assert_eq!(
        listed["estimate"],
        "2 tasks, ~10 requests, safe methods only"
    );
    // Nothing runs before it's approved.
    assert!(stub.replays().is_empty());
    assert!(r.tasks.list(&eng).is_empty());

    // Off-topic: no.
    push(vec![turn("", vec![call("1", "no", serde_json::json!({}))])]);
    let a = intake(serde_json::json!({ "text": "what's the meaning of life?" }).to_string()).await;
    assert_eq!(a["kind"], "no");

    // 4. Approved: the tasks are made and approved, and who asked is on them.
    let id = p["id"].as_str().unwrap().to_string();
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/plans/{id}"),
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let made: Vec<String> = body_json(&res)["made"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert_eq!(made.len(), 2);
    let t = task_of(&r, &made[0]);
    assert_eq!(t.asked_by, format!("human (plan {id})"));
    assert!(t.note.contains("My boss said"), "{}", t.note);
    let res = send_text(
        spall,
        "PATCH",
        &format!("/api/plans/{id}"),
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 409"), "once: {res}");

    // 5. Run through the gate; the first no longer reproduces: fixed, with
    // the retest exchange as evidence.
    eventually("started", || {
        task_of(&r, &made[0]).status == spall::tasks::Status::Running
    })
    .await;
    let (status, body) = agent_send(&r, &made[0], r#"{"id":"e1"}"#).await;
    assert_eq!(status, 200, "{body}");
    let replay = body["id"].as_str().unwrap().to_string();
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let out = tools
        .call(&made[0], "retest_result", &serde_json::json!({"candidate": cands[0], "verdict": "fixed", "exchange_id": replay, "note": "no SQL error now"}))
        .await;
    assert!(!out.is_error, "{}", out.text);
    let c = r.candidates.get(&eng, &cands[0]).unwrap();
    assert_eq!(c.finding.status(), Some(flux::Status::Fixed));
    assert!(c.exchanges().contains(&replay));
    let audit = audit_records(&token);
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with(&format!("approve {id}: made ")))));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("intake: P-"))));
}

/// With no authorized engagement, the only thing the intake can draft is
/// one (#65); and without a model there's no intake at all.
#[tokio::test]
async fn without_an_authorized_engagement_the_intake_drafts_only_one() {
    let model = Scripted::new(vec![]);
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push("e1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let spall = listener.local_addr().unwrap();
    let app = spall::build(spall_config(Some(api), &token, &["--scope", "example.com"]))
        .unwrap()
        .with_model(model.clone());
    tokio::spawn(app.run(listener));
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    // Agents aren't allowed (the default): not authorized for agent work.
    model.steps.lock().unwrap().extend(vec![
        turn("", vec![call("1", "plan_task", serde_json::json!({"role": "mapper", "goal": "map the API"}))]),
        turn("", vec![call("2", "propose_engagement", serde_json::json!({"name": "ACME API", "scope": ["api.acme.test"], "why": "no engagement allows agents"}))]),
        turn("", vec![call("3", "done", serde_json::json!({"summary": "Create an engagement first."}))]),
    ]);
    let res = post_text(spall, "/api/intake", r#"{"text":"map the API"}"#, None).await;
    let a = body_json(&res);
    assert!(a["plan"]["tasks"].as_array().unwrap().is_empty(), "{a}");
    assert_eq!(a["plan"]["proposal"]["kind"], "engagement");
    assert_eq!(
        a["plan"]["proposal"]["scope"],
        serde_json::json!(["api.acme.test"])
    );
    let seen = serde_json::to_string(&model.seen().last().unwrap().messages).unwrap();
    assert!(
        seen.contains("the only thing you can draft is an engagement"),
        "{seen}"
    );

    // No model, no intake.
    let (_dir, _stub, plain, _r) = agent_setup("{}").await;
    let res = post_text(plain, "/api/intake", r#"{"text":"map the API"}"#, None).await;
    assert!(
        res.starts_with("HTTP/1.1 409") && res.contains("needs spall's model"),
        "{res}"
    );
}

/// Since taphttp 0.8.0 (schema 7) evidence records where taphttp actually
/// connected (`peer`): a retest plan compares that with where the host
/// resolves now, ahead of spall's own lookup at flag time (#65, taphttp#24).
#[tokio::test]
async fn a_retest_plan_compares_the_evidences_peer_with_where_the_host_resolves_now() {
    let dir = tempfile::TempDir::new().unwrap();
    let token = dir.path().join("api-token");
    write_token(&token);
    let (stub, api) = start_stub().await;
    stub.push_json(
        "p1",
        entry("p1")
            .replace("\"schema_version\":3", "\"schema_version\":7")
            .replace(
                "\"replay_of\":null",
                "\"replay_of\":null,\"peer\":\"198.51.100.7:80\"",
            ),
    );
    let dns = Arc::new(FakeDns::default());
    // spall's own lookup at flag time says one thing; taphttp's peer wins.
    dns.set("example.com", &["192.0.2.1"]);
    let model = Scripted::new(vec![]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let spall = listener.local_addr().unwrap();
    let app = spall::build(spall_config(Some(api), &token, &["--scope", "example.com"]))
        .unwrap()
        .with_resolver(dns.clone())
        .with_model(model.clone());
    let r = app.replayer.clone().unwrap();
    tokio::spawn(app.run(listener));
    eventually("backfill", || !stub.exchange_queries().is_empty()).await;
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"agents_allowed":true}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"p1","rule":"WSTG-INPV-05"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    let eng = r.engagements.active().id;
    eventually("the lookup baseline", || {
        r.addresses.get(&eng, "example.com").is_some()
    })
    .await;
    dns.set("example.com", &["203.0.113.50"]);
    let plan = |steps: Vec<Step>| {
        model.steps.lock().unwrap().extend(steps);
        async move { body_json(&post_text(spall, "/api/intake", r#"{"text":"retest it"}"#, None).await) }
    };
    let steps = || {
        vec![
            turn(
                "",
                vec![call(
                    "1",
                    "plan_retest",
                    serde_json::json!({"engagement": eng}),
                )],
            ),
            turn(
                "",
                vec![call(
                    "2",
                    "done",
                    serde_json::json!({"summary": "one retest"}),
                )],
            ),
        ]
    };
    let a = plan(steps()).await;
    let w = a["plan"]["warnings"].to_string();
    assert!(
        w.contains("resolves to 203.0.113.50 now, not 198.51.100.7 (where taphttp connected for its findings)"),
        "{a}"
    );
    // Resolving to the peer again: no warning, whatever spall's lookup said.
    dns.set("example.com", &["198.51.100.7"]);
    let a = plan(steps()).await;
    assert_eq!(a["plan"]["warnings"], serde_json::json!([]), "{a}");
}

/// Too-wide scope entries (#83) are refused over the API unless the change
/// carries the override, which the audit record names; a pasted list needs
/// it too, all or nothing.
#[tokio::test]
async fn too_wide_scope_entries_need_the_audited_override() {
    let (dir, _stub, spall) = replay_setup(&["e1"], &[]).await;
    let scope = || async { body_json(&get_text(spall, "/api/status").await)["scope"].clone() };
    let before = scope().await;
    for body in [
        r#"{"op":"add","entry":"*.com"}"#,
        r#"{"op":"add","entry":"10.0.0.0/8"}"#,
        r#"{"op":"apply","add":["app.test","*.co.uk"],"exclude":[]}"#,
    ] {
        let res = post_text(spall, "/api/scope", body, None).await;
        assert!(
            res.starts_with("HTTP/1.1 400") && res.contains("too wide") && res.contains("--wide"),
            "{body}: {res}"
        );
    }
    assert_eq!(scope().await, before, "nothing changed, the apply included");
    let res = post_text(
        spall,
        "/api/scope",
        r#"{"op":"add","entry":"10.0.0.0/8","wide":true}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/scope",
        r#"{"op":"apply","add":["app.test","*.co.uk"],"exclude":[],"wide":true}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let s = scope().await.to_string();
    assert!(
        s.contains("10.0.0.0/8") && s.contains("*.co.uk") && s.contains("app.test"),
        "{s}"
    );
    let audit = audit_records(&dir.path().join("api-token"));
    let reasons: Vec<&str> = audit.iter().filter_map(|a| a["reason"].as_str()).collect();
    assert!(
        reasons.contains(&"add 10.0.0.0/8 (too wide: added with --wide)"),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("added with --wide: *.co.uk")),
        "{reasons:?}"
    );
}

/// The global never-test list (#84): after scope, for humans and agents,
/// past `scope all on`, never approvable; an engagement can be exempted
/// from an entry, explicitly and audited; the list survives a restart.
#[tokio::test]
async fn the_never_test_list_refuses_whatever_scope_says_unless_exempted() {
    let (dir, stub, spall, r) = agent_setup("{}").await;
    let token = dir.path().join("api-token");
    let never =
        |body: &'static str| async move { post_text(spall, "/api/never-test", body, None).await };
    let human = || async { post_text(spall, "/api/replay", r#"{"id":"e1"}"#, None).await };
    assert!(
        human().await.starts_with("HTTP/1.1 200"),
        "in scope, not listed yet"
    );
    let res = never(r#"{"op":"add","entry":"example.com"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(
        body_json(&res)["entries"],
        serde_json::json!(["example.com"])
    );
    let sent = stub.replays().len();

    let res = human().await;
    assert!(
        res.starts_with("HTTP/1.1 403") && res.contains("never-test list (example.com)"),
        "{res}"
    );
    let res = post_text(spall, "/api/scope", r#"{"op":"all","on":true}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(
        human().await.starts_with("HTTP/1.1 403"),
        "scope all on doesn't reach it"
    );
    // An agent is refused, not paused: no approval reaches it.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"e1"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(task_of(&r, "T-1").status, spall::tasks::Status::Running);
    assert_eq!(stub.replays().len(), sent, "nothing was sent");
    // Shown where scope is shown.
    let v = body_json(&get_text(spall, "/api/scope/check?target=example.com").await);
    assert_eq!(
        (v["state"].as_str(), v["by"].as_str()),
        (Some("out"), Some("never-test: example.com"))
    );

    // An exception names an entry on the list, and lets that engagement through.
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"never_test_exceptions":["other.test"]}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 400") && res.contains("isn't on the never-test list"),
        "{res}"
    );
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"never_test_exceptions":["EXAMPLE.com"]}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(human().await.starts_with("HTTP/1.1 200"), "exempted");
    let listed = body_json(&get_text(spall, "/api/never-test").await);
    assert_eq!(listed["exceptions"], serde_json::json!(["example.com"]));

    let audit = audit_records(&token);
    assert!(audit
        .iter()
        .any(|a| a["reason"] == "never-test add example.com"
            && a["scope_after"] == serde_json::json!(["example.com"])));
    assert!(
        audit.iter().any(|a| a["reason"]
            .as_str()
            .is_some_and(|s| s.contains("never_test_exceptions"))),
        "the exception is audited"
    );

    // The list outlives the process; the exception lives on the engagement.
    let (again, _) = spall::engagement::Store::open(
        &dir.path().join("engagements"),
        &spall::gate::Scope::default(),
        &spall::roe::Rules::default(),
    )
    .unwrap();
    assert_eq!(again.never_list(), ["example.com"]);
    assert_eq!(
        again.scope().explain("example.com").state,
        spall::gate::HostState::In,
        "the active engagement's exception"
    );
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"never_test_exceptions":[]}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(human().await.starts_with("HTTP/1.1 403"));
    let res = never(r#"{"op":"remove","entry":"example.com"}"#).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(human().await.starts_with("HTTP/1.1 200"));
    let res = never(r#"{"op":"remove","entry":"example.com"}"#).await;
    assert!(res.starts_with("HTTP/1.1 400"), "not on it: {res}");
}

/// Requests that look destructive (#85) are treated as unsafe whatever the
/// method: a human's replay asks first, a tester's pauses naming what
/// matched, a mapper's is refused, and the RoE can refuse them outright.
#[tokio::test]
async fn destructive_looking_requests_ask_first() {
    let (_dir, stub, spall, r) = agent_setup_with(
        "{}",
        &["--scope", "example.com"],
        &[("d1", "http://example.com/api/users/5/delete")],
    )
    .await;
    let human =
        |body: &'static str| async move { post_text(spall, "/api/replay", body, None).await };
    let res = human(r#"{"id":"d1"}"#).await;
    assert!(res.starts_with("HTTP/1.1 428"), "{res}");
    assert!(
        res.contains("looks destructive") && res.contains(r#"\"delete\" in the path"#),
        "{res}"
    );
    let res = human(r#"{"id":"e1","body":"x'; DROP TABLE users--"}"#).await;
    assert!(
        res.starts_with("HTTP/1.1 428") && res.contains("drop table"),
        "a payload: {res}"
    );
    assert!(
        human(r#"{"id":"e1"}"#).await.starts_with("HTTP/1.1 200"),
        "an ordinary GET still sends"
    );
    assert_eq!(stub.replays().len(), 1);

    // A tester's task pauses, the approval card names the match.
    let (status, body) = agent_send(&r, "T-1", r#"{"id":"d1"}"#).await;
    assert_eq!(status, 202, "{body}");
    let t = task_of(&r, "T-1");
    assert_eq!(t.status, spall::tasks::Status::WaitingOnApproval);
    assert!(
        t.pending
            .as_ref()
            .unwrap()
            .reason
            .contains("looks destructive"),
        "{:?}",
        t.pending
    );
    // A mapper's is refused, not paused.
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"map","role":"mapper"}"#,
        None,
    )
    .await;
    let id = body_json(&res)["id"].as_str().unwrap().to_string();
    send_text(
        spall,
        "PATCH",
        &format!("/api/tasks/{id}"),
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    let eng = r.engagements.active().id;
    r.tasks
        .transition(&eng, &id, spall::tasks::Status::Running, None, &now())
        .unwrap();
    let (status, body) = agent_send(&r, &id, r#"{"id":"d1"}"#).await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("looks destructive"),
        "{body}"
    );

    // Stricter rules refuse it outright.
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"refuse_destructive_looking":true}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = human(r#"{"id":"d1"}"#).await;
    assert!(
        res.starts_with("HTTP/1.1 403") && res.contains("refuse such requests"),
        "{res}"
    );
    assert_eq!(
        stub.replays().len(),
        1,
        "nothing destructive-looking was sent"
    );
}

/// sweep (#75): a check's paths against a base, after a soft-404 baseline:
/// hits are the paths that differ, each a recorded exchange; bounded by the
/// RoE and the task's budget; every request gated and attributed; checks
/// are vetted data, and an engagement's own are the tester's to import.
#[tokio::test]
async fn sweep_finds_what_differs_from_the_soft_404_inside_its_bounds() {
    // Enough budget for admin-interfaces (29), not then backup-files (31).
    let (dir, stub, spall, r) = agent_setup(r#"{"budget":40}"#).await;
    let base = "http://example.com/sweep-site";
    // The estimate first, nothing sent.
    let res = post_text(
        spall,
        "/api/sweep",
        &format!(r#"{{"base":"{base}","check":"metafiles"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let est = body_json(&res);
    let planned = est["planned"].as_u64().unwrap();
    assert!(
        est["cost"].as_str().unwrap().contains("soft-404 baseline"),
        "{est}"
    );
    assert!(stub.sends().is_empty());

    // A human's run: robots.txt differs from the soft 404; the rest don't.
    let res = post_text(
        spall,
        "/api/sweep",
        &format!(r#"{{"base":"{base}","check":"metafiles","yes":true}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let v = body_json(&res);
    assert_eq!(v["sent"].as_u64(), Some(planned), "{v}");
    assert_eq!(v["baseline"]["status"], 200);
    let hits: Vec<&str> = v["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert_eq!(hits, ["/robots.txt"], "{v}");
    assert!(v["hits"][0]["exchange"].as_str().unwrap().starts_with("s-"));
    let sends = stub.sends();
    assert!(sends[0]["url"]
        .as_str()
        .unwrap()
        .starts_with("http://example.com/sweep-site/spall-baseline-"));
    assert!(sends.iter().all(|s| s["meta"]["actor"] == "human"));

    // An agent's: attributed to its task, charged, an admin wall is a hit.
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let out = tools.call("T-1", "checks", &serde_json::json!({})).await;
    assert!(
        out.text.contains("admin-interfaces (WSTG-CONF-05"),
        "{}",
        out.text
    );
    let before = task_of(&r, "T-1").spent;
    let out = tools
        .call(
            "T-1",
            "sweep",
            &serde_json::json!({"base": base, "check": "admin-interfaces"}),
        )
        .await;
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("/admin") && out.text.contains("1 hit"),
        "{}",
        out.text
    );
    let n = spall::sweep::built_in()
        .into_iter()
        .find(|c| c.name == "admin-interfaces")
        .unwrap()
        .paths
        .len() as u32
        + 1;
    assert_eq!(
        task_of(&r, "T-1").spent,
        before + n,
        "every request charged"
    );
    assert!(stub
        .sends()
        .iter()
        .rev()
        .take(n as usize)
        .all(|s| s["meta"]["task"] == "T-1"));
    // Past the budget: refused up front, nothing sent.
    let sent = stub.sends().len();
    let out = tools
        .call(
            "T-1",
            "sweep",
            &serde_json::json!({"base": base, "check": "backup-files"}),
        )
        .await;
    assert!(
        out.is_error && out.text.contains("left of its budget"),
        "{}",
        out.text
    );
    assert_eq!(stub.sends().len(), sent);

    // Bounds: out of scope, the RoE's sweep_max.
    let res = post_text(
        spall,
        "/api/sweep",
        r#"{"base":"http://evil.test/","check":"metafiles"}"#,
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 403") && res.contains("isn't in scope"),
        "{res}"
    );
    let res = send_text(
        spall,
        "PATCH",
        "/api/engagement",
        r#"{"rules":{"sweep_max":5}}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/sweep",
        &format!(r#"{{"base":"{base}","check":"metafiles"}}"#),
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 403") && res.contains("allow 5 per sweep"),
        "{res}"
    );

    // An engagement's own check: the tester imports it; it's data, bounded.
    let add = |text: &str| serde_json::json!({"op": "add", "text": text}).to_string();
    let res = post_text(
        spall,
        "/api/checks",
        &add("name mine\nrule WSTG-CONF-04\nhit differs\npath https://evil.test/x"),
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 400") && res.contains("never a host"),
        "{res}"
    );
    let res = post_text(
        spall,
        "/api/checks",
        &add("name mine\nrule WSTG-CONF-04\nhit status 200\nhit differs\npath /robots.txt"),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/checks",
        &add("name metafiles\nrule WSTG-CONF-04\nhit differs\npath /x"),
        None,
    )
    .await;
    assert!(
        res.starts_with("HTTP/1.1 400") && res.contains("built-in"),
        "{res}"
    );
    let res = post_text(
        spall,
        "/api/sweep",
        &format!(r#"{{"base":"{base}","check":"mine","yes":true}}"#),
        None,
    )
    .await;
    let v = body_json(&res);
    assert_eq!(v["hits"][0]["path"], "/robots.txt", "{v}");
    // Agents can't import checks.
    let out = tools.call("T-1", "scope", &serde_json::json!({})).await;
    assert!(out.is_error);
    let audit = audit_records(&dir.path().join("api-token"));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("sweep metafiles on") && s.contains("1 hits"))));
    assert!(audit
        .iter()
        .any(|a| a["reason"] == "checks add mine (WSTG-CONF-04, 1 paths)"));
}

/// A stand-in interactsh server (#1): `/register` keeps the public key,
/// `/poll` answers with the interactions the test queued, encrypted the
/// way interactsh does (an AES-256 key under RSA-OAEP SHA-256, each
/// interaction AES-256-CFB with its IV first).
#[derive(Clone, Default)]
struct FakeOast {
    key: Arc<Mutex<Option<rsa::RsaPublicKey>>>,
    queued: Arc<Mutex<Vec<serde_json::Value>>>,
    polls: Arc<Mutex<u32>>,
}

async fn start_fake_oast() -> (FakeOast, SocketAddr) {
    use axum::routing::{get, post};
    let state = FakeOast::default();
    let app = axum::Router::new()
        .route(
            "/register",
            post(|State(s): State<FakeOast>, headers: axum::http::HeaderMap, body: String| async move {
                use base64::Engine;
                use rsa::pkcs8::DecodePublicKey;
                assert_eq!(headers.get("authorization").and_then(|v| v.to_str().ok()), Some("oast-token"));
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(v["correlation-id"].as_str().unwrap().len(), 20);
                let pem = base64::engine::general_purpose::STANDARD.decode(v["public-key"].as_str().unwrap()).unwrap();
                *s.key.lock().unwrap() = Some(rsa::RsaPublicKey::from_public_key_pem(std::str::from_utf8(&pem).unwrap()).unwrap());
                axum::Json(serde_json::json!({"message": "registration successful"}))
            }),
        )
        .route(
            "/poll",
            get(|State(s): State<FakeOast>| async move {
                use base64::Engine;
                use cfb_mode::cipher::{AsyncStreamCipher, KeyIvInit};
                let b64 = base64::engine::general_purpose::STANDARD;
                *s.polls.lock().unwrap() += 1;
                let queued: Vec<serde_json::Value> = std::mem::take(&mut *s.queued.lock().unwrap());
                let public = s.key.lock().unwrap().clone().unwrap();
                let mut aes_key = [0u8; 32];
                getrandom::fill(&mut aes_key).unwrap();
                let wrapped = public
                    .encrypt(&mut rsa::rand_core::OsRng, rsa::Oaep::new::<rsa::sha2::Sha256>(), &aes_key)
                    .unwrap();
                let data: Vec<String> = queued
                    .iter()
                    .map(|r| {
                        let mut iv = [0u8; 16];
                        getrandom::fill(&mut iv).unwrap();
                        let mut buf = serde_json::to_vec(r).unwrap();
                        cfb_mode::Encryptor::<aes::Aes256>::new_from_slices(&aes_key, &iv).unwrap().encrypt(&mut buf);
                        b64.encode([iv.as_slice(), &buf].concat())
                    })
                    .collect();
                axum::Json(serde_json::json!({"data": data, "extra": null, "aes_key": b64.encode(wrapped), "tld_data": null}))
            }),
        )
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, addr)
}

/// OOB callbacks (#1): an agent mints a payload (nothing sent), carries it
/// in a gated fetch, and a DNS hit on it comes back: recorded, audited,
/// linked to that exchange, on the task's log, and to the agent as
/// untrusted data it can propose with. The tester can mint and list too.
#[tokio::test]
async fn an_oob_hit_is_tied_to_the_task_and_the_exchange_that_carried_it() {
    let (oast, oast_addr) = start_fake_oast().await;
    let server = format!("http://{oast_addr}");
    std::env::set_var("SPALL_OAST_TOKEN", "oast-token");
    let (dir, stub, spall, r) = agent_setup_with(
        "{}",
        &[
            "--scope",
            "example.com",
            "--oast-server",
            &server,
            "--oast-domain",
            "oast.lab",
            "--oast-http",
            "http://interactsh",
        ],
        &[],
    )
    .await;
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let sent = stub.sends().len();
    let out = tools
        .call("T-1", "oob_payload", &serde_json::json!({}))
        .await;
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains(".oast.lab") && out.text.contains("http://interactsh/"),
        "{}",
        out.text
    );
    assert_eq!(stub.sends().len(), sent, "minting sends nothing");
    let eng = r.engagements.active().id;
    let payload = r.oob.as_ref().unwrap().list(&eng, Some("T-1")).0[0].clone();

    // Carried in a request the gate allows: a fetch with it in the query.
    let (status, body) = {
        let res = r
            .agent_fetch("T-1", serde_json::from_value(serde_json::json!({"url": format!("http://example.com/hook?cb={}", payload.host)})).unwrap())
            .await;
        let s = res.status().as_u16();
        let b = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (s, serde_json::from_slice::<serde_json::Value>(&b).unwrap())
    };
    assert_eq!(status, 200, "{body}");
    let carrier = body["id"].as_str().unwrap().to_string();

    // The target calls back: a DNS lookup of the payload.
    oast.queued.lock().unwrap().push(serde_json::json!({
        "protocol": "dns", "unique-id": payload.id, "full-id": payload.id, "q-type": "A",
        "raw-request": format!(";; {}.oast.lab IN A <script>x</script>", payload.id),
        "remote-address": "203.0.113.9", "timestamp": "2026-11-25T10:00:00Z"
    }));
    assert_eq!(r.oob_poll().await.unwrap(), 1);
    let (_, hits) = r.oob.as_ref().unwrap().list(&eng, None);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (
            hits[0].protocol.as_str(),
            hits[0].task.as_deref(),
            hits[0].exchange.as_deref()
        ),
        ("dns", Some("T-1"), Some(carrier.as_str()))
    );
    assert!(task_of(&r, "T-1").log.iter().any(|e| matches!(&e.event, spall::tasks::Event::Interaction { exchange: Some(x), .. } if *x == carrier)));
    let out = tools.call("T-1", "oob_hits", &serde_json::json!({})).await;
    assert!(
        out.text.contains("<untrusted-")
            && out.text.contains("203.0.113.9")
            && out.text.contains(&carrier),
        "{}",
        out.text
    );
    // The carrier is evidence the task sent.
    let out = tools
        .call("T-1", "propose_finding", &serde_json::json!({"exchange_id": carrier, "rule": "WSTG-INPV-19", "message": "blind SSRF: the target looked up our callback host"}))
        .await;
    assert!(!out.is_error, "{}", out.text);

    // The tester's side.
    let res = post_text(spall, "/api/oob", "", None).await;
    assert!(
        res.starts_with("HTTP/1.1 201") && res.contains(".oast.lab"),
        "{res}"
    );
    let v = body_json(&get_text(spall, "/api/oob").await);
    assert_eq!(v["configured"], true);
    assert_eq!(v["payloads"].as_array().unwrap().len(), 2);
    assert_eq!(v["hits"][0]["exchange"], carrier.as_str());
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["oob"]["hits"],
        1
    );
    // A hit that comes before taphttp has stored its carrier (the callback
    // is itself the carrying request): linked on a later poll.
    let mine = r.oob.as_ref().unwrap().list(&eng, None).0[0].clone();
    oast.queued.lock().unwrap().push(serde_json::json!({
        "protocol": "http", "unique-id": mine.id, "full-id": mine.id,
        "raw-request": "GET / HTTP/1.1", "remote-address": "203.0.113.10",
        "timestamp": taphttp_types::Timestamp::now().to_string()
    }));
    assert_eq!(r.oob_poll().await.unwrap(), 1);
    let hit = |r: &spall::replay::Replayer| r.oob.as_ref().unwrap().list(&eng, None).1[0].clone();
    assert_eq!((hit(&r).id, hit(&r).exchange), ("I-2".to_string(), None));
    stub.push_json(
        "late",
        entry("late").replace(
            "http://example.com/late",
            &format!("http://interactsh/{}", mine.id),
        ),
    );
    r.oob_poll().await.unwrap();
    assert_eq!(hit(&r).exchange.as_deref(), Some("late"));
    // A burst of callbacks on one payload: each hit gets the carrier
    // recorded last before it, not whichever is newest when spall polls.
    let p3 = r.oob_mint(None).await.unwrap();
    for (id, ts) in [
        ("b1", "2026-11-25T10:00:00Z"),
        ("b2", "2026-11-25T10:00:30Z"),
    ] {
        stub.push_json(
            id,
            entry(id)
                .replace(
                    &format!("http://example.com/{id}"),
                    &format!("http://interactsh/{}", p3.id),
                )
                .replace("2026-12-25T10:00:00Z", ts),
        );
    }
    for ts in ["2026-11-25T10:00:00.400Z", "2026-11-25T10:00:30.300Z"] {
        oast.queued.lock().unwrap().push(serde_json::json!({"protocol": "http", "unique-id": p3.id, "remote-address": "203.0.113.11", "timestamp": ts}));
    }
    assert_eq!(r.oob_poll().await.unwrap(), 2);
    let burst: Vec<Option<String>> = r
        .oob
        .as_ref()
        .unwrap()
        .list(&eng, None)
        .1
        .iter()
        .filter(|h| h.payload == p3.id)
        .map(|h| h.exchange.clone())
        .collect();
    assert_eq!(
        burst,
        [Some("b2".to_string()), Some("b1".to_string())],
        "newest first, each its own"
    );
    // The e-stop stops agents minting; hits keep coming.
    let res = post_text(spall, "/api/estop", "{}", None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(r
        .oob_mint(Some("T-1"))
        .await
        .unwrap_err()
        .1
        .contains("e-stop"));
    let audit = audit_records(&dir.path().join("api-token"));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("oob payload ") && s.contains("agent:T-1"))));
    assert!(audit.iter().any(|a| a["reason"]
        .as_str()
        .is_some_and(|s| s.starts_with("oob hit I-1: dns from 203.0.113.9"))
        && a["target"] == carrier.as_str()));
    assert!(*oast.polls.lock().unwrap() >= 1);
    // The session (with its key) is kept for late callbacks, 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("engagements/oob/session"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
}

/// A turn the whole fallback chain declines fails its task with the API's
/// reason (not a bare "declined"), and alerts (#86) report it, along with a
/// task waiting on approval: kinds and ids only.
#[tokio::test]
async fn a_declined_task_says_why_and_alerts_say_what_needs_you() {
    let model = Scripted::new(vec![Step::Turn(Turn {
        text: String::new(),
        calls: vec![],
        stop: Stop::Refusal,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 0,
        },
        note: Some("cyber: security testing".into()),
    })]);
    let (_dir, _stub, spall, r) = host_setup(model, "{}").await;
    eventually("declined", || {
        task_now(&r).status == spall::tasks::Status::Failed
    })
    .await;
    let t = task_now(&r);
    assert_eq!(
        t.reason.as_deref(),
        Some(
            "the model API declined (cyber: security testing), and no fallback model took the turn"
        )
    );
    // A second task pauses for approval: that needs the tester too.
    let res = post_text(
        spall,
        "/api/tasks",
        r#"{"goal":"post","role":"tester"}"#,
        None,
    )
    .await;
    let id = body_json(&res)["id"].as_str().unwrap().to_string();
    send_text(
        spall,
        "PATCH",
        &format!("/api/tasks/{id}"),
        r#"{"op":"approve"}"#,
        None,
    )
    .await;
    let eng = r.engagements.active().id;
    r.tasks
        .transition(&eng, &id, spall::tasks::Status::Running, None, &now())
        .unwrap();
    let (status, _) = agent_send(&r, &id, r#"{"id":"e1","method":"POST"}"#).await;
    assert_eq!(status, 202);
    let v = body_json(&get_text(spall, "/api/alerts").await);
    let kinds: Vec<(String, String)> = v["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["kind"].as_str().unwrap().to_string(),
                a["task"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&("failed".into(), "T-1".into())),
        "{kinds:?}"
    );
    assert!(kinds.contains(&("waiting".into(), id.clone())), "{kinds:?}");
    let latest = v["latest"].as_u64().unwrap();
    assert_eq!(
        body_json(&get_text(spall, "/api/status").await)["alerts"],
        latest
    );
    let after = body_json(&get_text(spall, &format!("/api/alerts?after={latest}")).await);
    assert!(after["alerts"].as_array().unwrap().is_empty());
    // Content-free: nothing but kinds, ids and times.
    for a in v["alerts"].as_array().unwrap() {
        let keys: Vec<&str> = a.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(
            keys.iter()
                .all(|k| ["seq", "at", "kind", "engagement", "task", "item"].contains(k)),
            "{keys:?}"
        );
    }
}

/// Surface (#95, #94): endpoints from traffic and an imported spec, with
/// who touched them and coverage by WSTG category: found (a candidate),
/// tested (a sweep, the tester's mark, an agent's record with its own
/// evidence). Out-of-scope hosts only on request.
#[tokio::test]
async fn surface_shows_endpoints_and_what_was_tested_where() {
    let (_dir, _stub, spall, r) = agent_setup("{}").await;
    // An imported API description: a documented endpoint nobody touched.
    let spec = r#"{"api_specs":[{"title":"shop","version":"1","format":"openapi 3.1.0","servers":["http://example.com/api"],"paths":["/orders/{orderId}"],"imported":"t"}]}"#;
    let res = send_text(spall, "PATCH", "/api/engagement", spec, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    // Found: a candidate on e1.
    let res = post_text(
        spall,
        "/api/candidates",
        r#"{"exchange":"e1","rule":"WSTG-INFO-02"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 201"), "{res}");
    // Tested: a sweep, the tester's mark, an agent's record.
    let res = post_text(
        spall,
        "/api/sweep",
        r#"{"base":"http://example.com/sweep-site","check":"metafiles","yes":true}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let key_e1 = "example.com|GET|/e1";
    let res = post_text(
        spall,
        "/api/coverage",
        &format!(r#"{{"key":"{key_e1}","category":"ATHZ","note":"two users, no IDOR"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let res = post_text(
        spall,
        "/api/coverage",
        &format!(r#"{{"key":"{key_e1}","category":"XSS"}}"#),
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 400"), "{res}");
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let out = tools
        .call(
            "T-1",
            "record_coverage",
            &serde_json::json!({"exchange_id": "e1", "category": "INPV", "note": "q"}),
        )
        .await;
    assert!(
        out.is_error && out.text.contains("isn't an exchange"),
        "evidence first: {}",
        out.text
    );
    assert!(
        !tools
            .call("T-1", "open", &serde_json::json!({"id": "e1"}))
            .await
            .is_error
    );
    let out = tools.call("T-1", "record_coverage", &serde_json::json!({"exchange_id": "e1", "category": "INPV", "note": "quotes in q, no SQL error"})).await;
    assert!(!out.is_error, "{}", out.text);

    let v = body_json(&get_text(spall, "/api/surface").await);
    assert_eq!(v["categories"].as_array().unwrap().len(), 12);
    let eps = v["endpoints"].as_array().unwrap();
    let row = |k: &str| {
        eps.iter()
            .find(|e| e["key"] == k)
            .cloned()
            .unwrap_or_else(|| panic!("{k} in {v}"))
    };
    let e1 = row(key_e1);
    assert_eq!(e1["scope"], "in");
    assert_eq!(e1["coverage"]["INFO"]["state"], "found");
    assert_eq!(e1["coverage"]["ATHZ"]["state"], "tested");
    assert_eq!(e1["coverage"]["ATHZ"]["by"], serde_json::json!(["human"]));
    assert_eq!(
        e1["coverage"]["INPV"]["by"],
        serde_json::json!(["agent:T-1"])
    );
    assert!(e1["coverage"].get("SESS").is_none(), "untested");
    let robots = row("example.com|GET|/sweep-site/robots.txt");
    assert_eq!(
        robots["coverage"]["INFO"]["how"],
        serde_json::json!(["sweep metafiles"])
    );
    let doc = row("example.com|ANY|/api/orders/{}");
    assert_eq!(
        (doc["documented"].as_bool(), doc["count"].as_u64()),
        (Some(true), Some(0))
    );
    assert!(
        eps.iter().all(|e| e["host"] != "evil.test"),
        "out of scope only on request"
    );
    let all = body_json(&get_text(spall, "/api/surface?scope=all").await);
    assert!(all["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["host"] == "evil.test" && e["scope"] == "unknown"));
    // The agent's view, wrapped as untrusted. (These tools have their own
    // empty hub; in spall they share the app's, so traffic shows too.)
    let out = tools
        .call("T-1", "surface", &serde_json::json!({"untested": "ATHZ"}))
        .await;
    assert!(
        out.text.contains("<untrusted-") && out.text.contains("/api/orders/{orderId}"),
        "{}",
        out.text
    );
}

/// The sandbox (#13, slice 1) through the agent's own path: a command runs
/// in the task's sandbox, is charged, logged, audited and handed back as
/// untrusted output; containment holds (the fake keeps the contract the
/// real runsc test checks); a role that runs no tools is refused; the
/// e-stop kills a running command and the reaper removes a finished task's
/// sandbox. Without one configured, the agent is told so.
#[tokio::test]
async fn an_agent_runs_tools_in_its_task_sandbox_charged_logged_and_stoppable() {
    use spall::sandbox::{Fake, Key, Sandbox, State};
    let (_dir, _stub, spall, r) =
        agent_setup_with(r#"{"budget":5}"#, &["--scope", "example.com"], &[]).await;
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let eng = r.engagements.active().id;

    // Not configured: the agent is told, nothing is charged.
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["echo", "hi"]}),
        )
        .await;
    assert!(
        out.is_error && out.text.contains("no sandbox is configured"),
        "{}",
        out.text
    );
    assert_eq!(task_of(&r, "T-1").spent, 0);
    let st = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(st["sandbox"]["state"], "off");

    let fake = Arc::new(Fake::default());
    *r.sandbox.write().unwrap() = State::Ready(fake.clone());
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["echo", "hello <b>there</b>"]}),
        )
        .await;
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("echo exited 0"), "{}", out.text);
    assert!(
        out.text.contains("untrusted") && out.text.contains("hello <b>there</b>"),
        "wrapped: {}",
        out.text
    );
    let key = Key {
        engagement: eng.clone(),
        task: "T-1".into(),
    };
    assert_eq!(fake.live(), vec![key.clone()]);
    let t = task_of(&r, "T-1");
    assert_eq!(t.spent, 1, "one command, one unit of budget");
    // The command's full output is kept as a task artifact the event names.
    let art_id = t
        .log
        .iter()
        .find_map(|e| match &e.event {
            spall::tasks::Event::Sandbox { argv, artifact, .. } if argv[0] == "echo" => {
                artifact.clone()
            }
            _ => None,
        })
        .expect("the echo event names its artifact");
    let arts = body_json(&get_text(spall, "/api/tasks/T-1/artifacts").await);
    let a0 = &arts["artifacts"][0];
    assert_eq!(
        (a0["id"].as_str(), a0["kind"].as_str(), a0["task"].as_str()),
        (Some(art_id.as_str()), Some("output"), Some("T-1"))
    );
    assert_eq!(a0["label"], "echo-output.txt");
    assert!(a0["size"].as_u64().unwrap() > 0 && a0["sha256"].as_str().unwrap().len() == 64);
    // The bytes come back with the stored content type and hold the full output.
    let got = get_text(spall, &format!("/api/artifacts/{art_id}")).await;
    assert!(got.contains("content-type: text/plain"), "{got}");
    assert!(
        got.contains("$ echo hello <b>there</b>") && got.contains("--- stdout ---"),
        "{got}"
    );
    assert!(
        get_text(spall, "/api/artifacts/A-404")
            .await
            .starts_with("HTTP/1.1 404"),
        "unknown artifact is 404"
    );

    // save_poc (#13 slice 7) stores the proof as a Poc artifact on the task.
    let out = tools
        .call(
            "T-1",
            "save_poc",
            &serde_json::json!({"proof": "GET /a?id=2 -> 200 shows owner=alice (not me); IDOR, read-only.", "label": "idor-poc.txt"}),
        )
        .await;
    assert!(
        !out.is_error && out.text.contains("saved the PoC"),
        "{}",
        out.text
    );
    let arts = body_json(&get_text(spall, "/api/tasks/T-1/artifacts").await);
    let poc = arts["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "poc")
        .expect("a poc artifact");
    assert_eq!(poc["label"], "idor-poc.txt");
    let body = get_text(
        spall,
        &format!("/api/artifacts/{}", poc["id"].as_str().unwrap()),
    )
    .await;
    assert!(body.contains("IDOR, read-only"), "{body}");
    // An empty proof is refused.
    assert!(
        tools
            .call("T-1", "save_poc", &serde_json::json!({"proof": "   "}))
            .await
            .is_error
    );
    let st = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(
        (
            st["sandbox"]["state"].as_str(),
            st["sandbox"]["live"].as_u64()
        ),
        (Some("ready"), Some(1))
    );

    // Containment is the sandbox's, not a filter's: writing outside scratch
    // fails there, and there's no route out.
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["touch", "/etc/passwd"]}),
        )
        .await;
    assert!(
        out.text.contains("touch exited 1") && out.text.contains("Read-only"),
        "{}",
        out.text
    );
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["curl", "http://evil.test/"]}),
        )
        .await;
    assert!(out.text.contains("curl exited 6"), "{}", out.text);

    // A shell string is refused as an argument shape, not by matching it.
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": "rm -rf /"}),
        )
        .await;
    assert!(out.is_error, "{}", out.text);
    let out = tools
        .call("T-1", "run_in_sandbox", &serde_json::json!({"argv": []}))
        .await;
    assert!(out.is_error, "{}", out.text);

    // The e-stop kills a running command.
    let t2 = tools.clone();
    let running = tokio::spawn(async move {
        t2.call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["block"]}),
        )
        .await
    });
    eventually("block started", || {
        fake.ran
            .lock()
            .unwrap()
            .iter()
            .any(|(_, a)| a[0] == "block")
    })
    .await;
    let res = post_text(spall, "/api/estop", r#"{"reason":"test"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .expect("stopped in time")
        .unwrap();
    assert!(out.is_error || out.text.contains("stopped"), "{}", out.text);
    assert!(fake.live().is_empty(), "the e-stop removed the sandbox");
    assert!(fake.torn.lock().unwrap().contains(&key));
    // The halted task's log is closed (halted is forever), so the kill is
    // on the audit record, next to every other sandbox command.
    let audit = std::fs::read_to_string(_dir.path().join("audit.jsonl")).unwrap_or_default();
    let block = audit
        .lines()
        .find(|l| l.contains(r#""action":"sandbox""#) && l.contains(r#""block""#))
        .unwrap_or_else(|| panic!("no audit for block: {audit}"));
    assert!(
        block.contains("stopped") && block.contains("e-stop"),
        "{block}"
    );
    assert!(task_of(&r, "T-1").log.iter().any(|e| matches!(
        &e.event,
        spall::tasks::Event::Status {
            to: spall::tasks::Status::Halted,
            ..
        }
    )));
}

/// Toolpacks (#13 slice 4b): a packed tool runs only if its pack is enabled;
/// an exploitation-class tool pauses the task for the tester, who approves
/// the exact argv once.
#[tokio::test]
async fn sandbox_toolpacks_gate_a_packed_tool_by_its_category() {
    use spall::sandbox::{Fake, State};
    use spall::tasks::Status;
    let (_dir, _stub, spall, r) = agent_setup_with(
        r#"{"budget":10}"#,
        &["--scope", "example.com", "--sandbox-packs", "fuzz,sqli"],
        &[],
    )
    .await;
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    *r.sandbox.write().unwrap() = State::Ready(Arc::new(Fake::default()));
    let run = |argv: serde_json::Value| {
        let tools = tools.clone();
        async move {
            tools
                .call(
                    "T-1",
                    "run_in_sandbox",
                    &serde_json::json!({ "argv": argv }),
                )
                .await
        }
    };

    // A tool whose pack isn't enabled: refused, nothing charged.
    let out = run(serde_json::json!(["nuclei", "-u", "http://example.com/"])).await;
    assert!(
        out.is_error && out.text.contains("'scan' toolpack"),
        "{}",
        out.text
    );
    assert_eq!(task_of(&r, "T-1").spent, 0);

    // A scanner whose pack is enabled: runs under the task (no pause), charged.
    let out = run(serde_json::json!(["ffuf", "-u", "http://example.com/FUZZ"])).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(task_of(&r, "T-1").spent, 1);
    assert_eq!(task_of(&r, "T-1").status, Status::Running);

    // An exploitation tool pauses the task for the tester; nothing charged.
    let sqlmap = serde_json::json!(["sqlmap", "-u", "http://example.com/?id=1", "--batch"]);
    let out = run(sqlmap.clone()).await;
    assert!(out.is_error && out.text.contains("paused"), "{}", out.text);
    let t = task_of(&r, "T-1");
    assert_eq!(t.status, Status::WaitingOnApproval);
    assert_eq!(t.spent, 1, "a pause charges nothing");
    let pend = t.pending.as_ref().expect("a pending");
    assert!(pend.target.contains("sqlmap -u"), "{}", pend.target);
    assert!(matches!(&pend.ask, spall::tasks::Ask::Tool { name } if name == "sqlmap"));

    // The tester approves it once; the retry runs it and is charged.
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"approve_once"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(task_of(&r, "T-1").status, Status::Running);
    let out = run(sqlmap.clone()).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(task_of(&r, "T-1").spent, 2, "the approved run is charged");
    // The grant is used up: a second run pauses again.
    let out = run(sqlmap).await;
    assert!(out.is_error && out.text.contains("paused"), "{}", out.text);
    assert_eq!(task_of(&r, "T-1").status, Status::WaitingOnApproval);
}

/// A Scoper runs no tools; the reaper removes the sandbox of a task that
/// finished.
#[tokio::test]
async fn a_scoper_runs_no_tools_and_a_finished_tasks_sandbox_is_removed() {
    use spall::sandbox::{Cmd, Fake, Sandbox, State};
    let (_dir, _stub, spall, r) = agent_setup_with("{}", &["--scope", "example.com"], &[]).await;
    let fake = Arc::new(Fake::default());
    *r.sandbox.write().unwrap() = State::Ready(fake.clone());
    let eng = r.engagements.active().id;
    let mut scoper = task_of(&r, "T-1");
    scoper.role = spall::tasks::Role::Scoper;
    let refused = r
        .run_in_sandbox(&scoper, Cmd::new(vec!["echo".into()], None).unwrap())
        .await;
    assert!(refused.unwrap_err().contains("scoper runs no tools"));
    assert!(fake.ran.lock().unwrap().is_empty());

    let t = task_of(&r, "T-1");
    r.run_in_sandbox(&t, Cmd::new(vec!["echo".into(), "x".into()], None).unwrap())
        .await
        .unwrap();
    assert_eq!(fake.live().len(), 1);
    r.sandbox_reap().await;
    assert_eq!(fake.live().len(), 1, "a running task keeps its sandbox");
    let res = send_text(
        spall,
        "PATCH",
        "/api/tasks/T-1",
        r#"{"op":"halt","reason":"done here"}"#,
        None,
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    r.sandbox_reap().await;
    assert!(fake.live().is_empty(), "removed with its task");
    let _ = eng;
}

/// The sandbox's network (#13 slice 2): with taphttp's credential-only
/// listener checked, each command gets its own credential: an allowlist of
/// what can't reach past scope (a wildcard covering an exclusion is left
/// out, with why), the task's remaining budget, its attribution. The
/// requests taphttp counted are charged to the task, the credential is
/// revoked after the command (and at once on the e-stop), and the agent is
/// told what it could reach. Until the listener exists, no network, and
/// the reason says so.
#[tokio::test]
async fn a_sandbox_command_gets_its_own_scoped_credential_and_pays_for_its_requests() {
    use spall::sandbox::{Fake, NetState, Sandbox, State};
    let (dir, stub, spall, r) = agent_setup_with(
        r#"{"budget":10}"#,
        &["--scope", "example.com,*.lab.test,!pay.lab.test"],
        &[],
    )
    .await;
    let tools = spall::agent::Tools {
        replayer: r.clone(),
        hub: Arc::new(spall::hub::Hub::new(16)),
    };
    let fake = Arc::new(Fake::default());
    *r.sandbox.write().unwrap() = State::Ready(fake.clone());

    // taphttp without the listener (the stub's /info has no auth_proxy):
    // no network, and the reason names taphttp#27.
    let st = r.sandbox_net_check().await;
    assert!(
        matches!(&st, NetState::Waiting(w) if w.contains("taphttp#27")),
        "{}",
        st.summary()
    );
    *r.sandbox_net.write().unwrap() = st;
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["curl", "http://example.com/"]}),
        )
        .await;
    assert!(
        out.text.contains("Network: none yet") && out.text.contains("curl exited 6"),
        "{}",
        out.text
    );
    assert!(
        stub.clients().is_empty(),
        "no credential without the listener"
    );
    assert_eq!(fake.nets.lock().unwrap().last().cloned(), Some(None));
    let st = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(st["sandbox"]["network"]["state"], "waiting");

    // With it: a credential per command.
    *r.sandbox_net.write().unwrap() = NetState::Ready {
        proxy: "taphttp:8090".into(),
    };
    *stub.0.client_sent.lock().unwrap() = 3;
    let spent = task_of(&r, "T-1").spent;
    let out = tools
        .call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["curl", "http://example.com/"]}),
        )
        .await;
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("curl exited 0"), "{}", out.text);
    assert!(
        out.text.contains("Network: through taphttp to example.com"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("*.lab.test: covers pay.lab.test (excluded)"),
        "left out, with why: {}",
        out.text
    );
    assert!(
        out.text.contains("3 request(s) sent, 1 refused"),
        "{}",
        out.text
    );
    let issued = stub.clients();
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0]["allow"], serde_json::json!(["example.com"]));
    assert_eq!(
        issued[0]["meta"],
        serde_json::json!({"actor": "agent", "task": "T-1"})
    );
    assert_eq!(
        issued[0]["budget"],
        serde_json::json!(10 - (spent + 1)),
        "the task's remaining budget, after the command's own unit"
    );
    assert_eq!(
        fake.nets.lock().unwrap().last().cloned(),
        Some(Some("http://c-1:p%40ss%2F1@taphttp:8090".to_string())),
        "the command's credential, escaped"
    );
    assert_eq!(
        stub.revoked(),
        vec!["c-1".to_string()],
        "revoked after the command"
    );
    let t = task_of(&r, "T-1");
    assert_eq!(
        t.spent,
        spent + 1 + 3,
        "the command's unit and taphttp's three requests"
    );
    assert!(t.log.iter().any(|e| matches!(
        &e.event,
        spall::tasks::Event::Sandbox {
            sent: Some(3),
            refused: Some(1),
            ..
        }
    )));
    let audit = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
    assert!(
        !audit.contains("p@ss/1") && !audit.contains("p%40ss"),
        "the password never reaches the audit log"
    );
    let st = body_json(&get_text(spall, "/api/status").await);
    assert_eq!(st["sandbox"]["network"]["state"], "ready");

    // The e-stop revokes a running command's credential at once.
    *stub.0.client_sent.lock().unwrap() = 0;
    let t2 = tools.clone();
    let running = tokio::spawn(async move {
        t2.call(
            "T-1",
            "run_in_sandbox",
            &serde_json::json!({"argv": ["block"]}),
        )
        .await
    });
    eventually("block started", || {
        stub.clients().len() == 2
            && fake
                .ran
                .lock()
                .unwrap()
                .iter()
                .any(|(_, a)| a[0] == "block")
    })
    .await;
    let res = post_text(spall, "/api/estop", r#"{"reason":"test"}"#, None).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .expect("stopped in time");
    assert!(
        stub.revoked().contains(&"c-2".to_string()),
        "revoked on the e-stop: {:?}",
        stub.revoked()
    );
    assert!(fake.live().is_empty());
}
