//! Client for taphttp's lever API (taphttp v0.3.0+), used when spall is
//! started with `--taphttp-api`.
//!
//! - [`Follower`] backfills recent history from `GET /exchanges`, then
//!   follows the `/feed` WebSocket with `?since=<last id>` so reconnects
//!   lose nothing. It re-syncs on `reset` and resumes on `lagged`.
//! - [`Lever::get`] is used by the web layer to proxy history queries;
//!   the browser never sees the token or talks to taphttp directly.
//!
//! The token is read from `--taphttp-token-file` on every request, so spall
//! can start before taphttp has created it. It is never logged.

use crate::hub::Hub;
use crate::{parse_entry, publish_entry, SchemaWarner};
use anyhow::{anyhow, bail, Context};
use futures_util::StreamExt;
use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, AUTHORIZATION};
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// taphttp caps `/exchanges` pages at this size.
pub const MAX_PAGE: usize = 500;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// Large bodies arrive as single frames; allow well beyond tungstenite's
/// 16 MiB frame default.
const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

pub struct Lever {
    /// `http://host:port`, validated by config.
    base: String,
    token_file: PathBuf,
    http: Client<HttpConnector, Empty<Bytes>>,
    /// True while the follower holds an open `/feed` connection.
    connected: AtomicBool,
}

impl Lever {
    pub fn new(base: String, token_file: PathBuf) -> Self {
        Self {
            base,
            token_file,
            http: Client::builder(TokioExecutor::new()).build_http(),
            connected: AtomicBool::new(false),
        }
    }

    /// Whether spall is currently following taphttp's feed.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn auth_header(&self) -> anyhow::Result<HeaderValue> {
        let raw = std::fs::read_to_string(&self.token_file)
            .with_context(|| format!("reading token file {}", self.token_file.display()))?;
        let token = raw.trim();
        if token.is_empty() {
            bail!("token file {} is empty", self.token_file.display());
        }
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            anyhow!(
                "token file {} holds an invalid token",
                self.token_file.display()
            )
        })
    }

    /// `GET <base><path_and_query>` with the bearer token. Returns the
    /// response as-is (any status); errors only for transport problems.
    pub async fn get(&self, path_and_query: &str) -> anyhow::Result<Response<Incoming>> {
        let req = Request::get(format!("{}{path_and_query}", self.base))
            .header(AUTHORIZATION, self.auth_header()?)
            .body(Empty::new())
            .context("building request")?;
        tokio::time::timeout(REQUEST_TIMEOUT, self.http.request(req))
            .await
            .map_err(|_| anyhow!("taphttp API timed out"))?
            .context("taphttp API request failed")
    }

    /// The newest `limit` exchanges as raw JSON, newest first.
    async fn newest(&self, limit: usize) -> anyhow::Result<Vec<Box<serde_json::value::RawValue>>> {
        let res = self
            .get(&format!("/exchanges?limit={}", limit.clamp(1, MAX_PAGE)))
            .await?;
        let status = res.status();
        let body = res
            .into_body()
            .collect()
            .await
            .context("reading /exchanges")?
            .to_bytes();
        if status != StatusCode::OK {
            bail!("GET /exchanges returned {status}");
        }
        serde_json::from_slice(&body).context("parsing /exchanges")
    }

    async fn connect_feed(
        &self,
        since: Option<&str>,
    ) -> anyhow::Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let authority = self.base.trim_start_matches("http://");
        let mut url = format!("ws://{authority}/feed");
        if let Some(id) = since {
            url.push('?');
            url.push_str(&serde_urlencoded::to_string([("since", id)]).context("encoding since")?);
        }
        let mut req = url.into_client_request().context("building feed request")?;
        req.headers_mut().insert(AUTHORIZATION, self.auth_header()?);
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (ws, _) = tokio::time::timeout(
            REQUEST_TIMEOUT,
            tokio_tungstenite::connect_async_with_config(req, Some(config), false),
        )
        .await
        .map_err(|_| anyhow!("connecting to /feed timed out"))?
        .context("connecting to /feed")?;
        Ok(ws)
    }
}

/// One `/feed` text frame.
#[derive(Debug)]
pub enum Frame {
    Entry(Box<taphttp_types::TrafficEntry>, Option<u32>),
    /// `since` was unknown to taphttp; re-sync from `/exchanges`.
    Reset,
    /// This client missed exchanges; reconnect with `since`.
    Lagged(u64),
    /// A control frame type this spall does not know. Ignored.
    UnknownControl(String),
}

#[derive(Deserialize)]
struct Control {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    skipped: u64,
}

/// Classify a feed frame. Control frames always have a `type` key;
/// exchanges never do.
pub fn parse_frame(text: &str) -> Result<Frame, serde_json::Error> {
    if let Ok(Control {
        kind: Some(kind),
        skipped,
    }) = serde_json::from_str::<Control>(text)
    {
        return Ok(match kind.as_str() {
            "reset" => Frame::Reset,
            "lagged" => Frame::Lagged(skipped),
            _ => Frame::UnknownControl(kind),
        });
    }
    let (entry, newer) = parse_entry(text)?;
    Ok(Frame::Entry(Box::new(entry), newer))
}

/// Ids published recently, so a re-sync does not publish an exchange twice.
struct Seen {
    order: VecDeque<String>,
    set: HashSet<String>,
    cap: usize,
}

impl Seen {
    fn new(cap: usize) -> Self {
        Self {
            order: VecDeque::new(),
            set: HashSet::new(),
            cap: cap.max(1),
        }
    }

    /// Returns true if `id` was not seen before (and records it).
    fn insert(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return false;
        }
        if self.order.len() == self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.order.push_back(id.to_string());
        self.set.insert(id.to_string());
        true
    }
}

enum FeedEnd {
    Reset,
    Reconnect,
}

pub struct Follower {
    lever: Arc<Lever>,
    hub: Arc<Hub>,
    backfill: usize,
    seen: Seen,
    last: Option<String>,
    schema: SchemaWarner,
    last_error: Option<String>,
}

impl Follower {
    pub fn new(lever: Arc<Lever>, hub: Arc<Hub>, history: usize) -> Self {
        Self {
            lever,
            hub,
            backfill: history.min(MAX_PAGE),
            seen: Seen::new(history.max(MAX_PAGE) * 2),
            last: None,
            schema: SchemaWarner::default(),
            last_error: None,
        }
    }

    fn publish(&mut self, entry: &taphttp_types::TrafficEntry, newer: Option<u32>) {
        self.schema.note(newer);
        self.last = Some(entry.id.clone());
        if self.seen.insert(&entry.id) {
            publish_entry(entry, &self.hub);
        }
    }

    /// Publish the newest page of history (oldest first) and resume after it.
    async fn sync(&mut self) -> anyhow::Result<()> {
        let page = self.lever.newest(self.backfill.max(1)).await?;
        if page.is_empty() {
            self.last = None;
        }
        for raw in page.iter().rev() {
            match parse_entry(raw.get()) {
                Ok((entry, newer)) => self.publish(&entry, newer),
                Err(e) => tracing::warn!("skipping malformed exchange from /exchanges: {e}"),
            }
        }
        Ok(())
    }

    async fn follow_feed(&mut self) -> anyhow::Result<FeedEnd> {
        let mut ws = self.lever.connect_feed(self.last.as_deref()).await?;
        self.lever.connected.store(true, Ordering::Relaxed);
        let result = self.read_feed(&mut ws).await;
        self.lever.connected.store(false, Ordering::Relaxed);
        result
    }

    async fn read_feed(
        &mut self,
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> anyhow::Result<FeedEnd> {
        if self.last_error.take().is_some() {
            tracing::info!("connected to taphttp feed");
        }
        while let Some(msg) = ws.next().await {
            let text = match msg.context("reading /feed")? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            match parse_frame(text.as_str()) {
                Ok(Frame::Entry(entry, newer)) => self.publish(&entry, newer),
                Ok(Frame::Reset) => {
                    tracing::info!(
                        "taphttp does not know the last exchange id; re-syncing history"
                    );
                    return Ok(FeedEnd::Reset);
                }
                Ok(Frame::Lagged(n)) => {
                    tracing::warn!("fell behind taphttp's feed ({n} skipped); resuming");
                    return Ok(FeedEnd::Reconnect);
                }
                Ok(Frame::UnknownControl(kind)) => {
                    tracing::debug!("ignoring unknown feed control frame {kind:?}");
                }
                Err(e) => {
                    tracing::warn!("skipping malformed feed frame ({} bytes): {e}", text.len())
                }
            }
        }
        Ok(FeedEnd::Reconnect)
    }

    fn note_error(&mut self, e: &anyhow::Error) {
        let msg = format!("{e:#}");
        if self.last_error.as_deref() != Some(msg.as_str()) {
            tracing::warn!("taphttp API: {msg}; retrying");
            self.last_error = Some(msg);
        }
    }

    /// Follow taphttp forever.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut need_sync = true;
        let mut backoff = MIN_BACKOFF;
        loop {
            if need_sync {
                if let Err(e) = self.sync().await {
                    self.note_error(&e);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
                need_sync = false;
            }
            match self.follow_feed().await {
                Ok(FeedEnd::Reset) => {
                    need_sync = true;
                    backoff = MIN_BACKOFF;
                }
                Ok(FeedEnd::Reconnect) => {
                    backoff = MIN_BACKOFF;
                    tokio::time::sleep(MIN_BACKOFF).await;
                }
                Err(e) => {
                    self.note_error(&e);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = r#"{"schema_version":3,"id":"x1","ts":"2026-12-25T10:00:00Z","host":"h","method":"GET","url":"/","req_headers":[],"req_body":null,"req_body_encoding":"utf8","status":200,"res_headers":[],"res_body":null,"res_body_encoding":"utf8","duration_ms":1,"replay_of":null}"#;

    #[test]
    fn frames_are_classified() {
        assert!(matches!(parse_frame(ENTRY), Ok(Frame::Entry(e, None)) if e.id == "x1"));
        assert!(matches!(
            parse_frame(r#"{"type":"reset","since":"abc"}"#),
            Ok(Frame::Reset)
        ));
        assert!(matches!(
            parse_frame(r#"{"type":"lagged","skipped":7}"#),
            Ok(Frame::Lagged(7))
        ));
        assert!(matches!(
            parse_frame(r#"{"type":"something-new"}"#),
            Ok(Frame::UnknownControl(k)) if k == "something-new"
        ));
        assert!(parse_frame("not json").is_err());
        assert!(parse_frame(r#"{"id":"half"}"#).is_err());
    }

    #[test]
    fn newer_schema_entry_is_flagged() {
        let v4 = ENTRY.replace("\"schema_version\":3", "\"schema_version\":4");
        assert!(matches!(parse_frame(&v4), Ok(Frame::Entry(_, Some(4)))));
    }

    #[test]
    fn seen_dedupes_and_is_bounded() {
        let mut s = Seen::new(2);
        assert!(s.insert("a"));
        assert!(!s.insert("a"));
        assert!(s.insert("b"));
        assert!(s.insert("c")); // evicts "a"
        assert!(s.insert("a"));
        assert!(!s.insert("c"));
    }

    #[test]
    fn token_is_read_trimmed_and_required() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("api-token");
        let lever = Lever::new("http://127.0.0.1:1".into(), path.clone());
        assert!(lever.auth_header().is_err(), "missing file");
        std::fs::write(&path, "  \n").unwrap();
        assert!(lever.auth_header().is_err(), "empty file");
        std::fs::write(&path, "abc123\n").unwrap();
        assert_eq!(lever.auth_header().unwrap(), "Bearer abc123");
        std::fs::write(&path, "bad\ntoken").unwrap();
        assert!(lever.auth_header().is_err(), "header injection");
    }
}
