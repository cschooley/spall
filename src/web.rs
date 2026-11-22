//! HTTP surface: the built SPA (ui/dist, embedded), the SSE feed, the
//! history proxy, replay (through the action gate), and a health check.

use crate::hub::{Hub, Published};
use crate::lever::Lever;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream::{self, Stream, StreamExt};
use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

/// The page renders attacker-controlled traffic. It never builds HTML from
/// it, and this policy (docs/architecture.md, Frontend; plus form-action)
/// forbids inline script and any off-origin fetch as a second line of defense.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; \
base-uri 'none'; frame-ancestors 'none'; form-action 'none'";

/// Files of the built UI, by path relative to `ui/dist`.
pub trait UiAssets: Send + Sync {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>>;
}

/// `ui/dist`, embedded at compile time in release builds and read from
/// disk in debug builds. Missing until `npm --prefix ui run build` has run;
/// spall still builds and serves the API without it.
#[derive(rust_embed::Embed)]
#[folder = "ui/dist"]
#[allow_missing = true]
struct UiDist;

pub struct EmbeddedUi;

impl UiAssets for EmbeddedUi {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        UiDist::get(path).map(|f| f.data)
    }
}

const UI_NOT_BUILT: &str = "spall's UI is not built.\n\n\
Build it, then rebuild spall:\n\n\
    npm --prefix ui ci\n\
    npm --prefix ui run build\n\
    cargo build\n\n\
The API (/events, /api/exchanges, /healthz) works without it.\n";

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub allowed_hosts: Arc<Vec<String>>,
    /// taphttp lever API client; `None` when tailing a log file.
    pub lever: Option<Arc<Lever>>,
    pub ui: Arc<dyn UiAssets>,
    /// Where clients reach taphttp's proxy (display only).
    pub proxy_address: Arc<str>,
    /// Replay through the action gate; `None` without the lever API.
    pub replayer: Option<Arc<crate::replay::Replayer>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .route("/events", get(events))
        .route("/healthz", get(healthz))
        .route("/api/status", get(api_status))
        .route("/api/exchanges", get(api_exchanges))
        .route("/api/exchanges/{id}", get(api_exchange))
        .route("/api/audit", get(api_audit))
        .route("/api/scope", post(api_scope))
        .route("/api/scope/check", get(api_scope_check))
        .route(
            "/api/replay",
            post(api_replay).layer(DefaultBodyLimit::max(crate::replay::MAX_REQUEST_BYTES)),
        )
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn index(State(state): State<AppState>) -> Response {
    match state.ui.get("index.html") {
        Some(html) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html.into_owned(),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            UI_NOT_BUILT,
        )
            .into_response(),
    }
}

/// Built files under `ui/dist/assets/`. Lookup is by exact key in the
/// embedded set, so `..` cannot reach anything outside it.
async fn asset(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let key = format!("assets/{path}");
    match state.ui.get(&key) {
        Some(bytes) => (
            [(header::CONTENT_TYPE, content_type(&key))],
            bytes.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Query parameters forwarded to taphttp's `GET /exchanges`. Anything else
/// (notably `token`) is dropped.
const EXCHANGE_PARAMS: &[&str] = &[
    "limit",
    "before",
    "host",
    "method",
    "status",
    "q",
    "since",
    "until",
    "replay_of",
];

/// Keep only the allowlisted parameters, re-encoded. `None` if the query
/// string is not valid form encoding.
pub fn forwardable_query(raw: Option<&str>) -> Option<String> {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(raw.unwrap_or("")).ok()?;
    let kept: Vec<(String, String)> = pairs
        .into_iter()
        .filter(|(k, _)| EXCHANGE_PARAMS.contains(&k.as_str()))
        .collect();
    serde_urlencoded::to_string(kept).ok()
}

/// taphttp exchange ids are UUIDs; accept a conservative superset so a
/// path segment can never smuggle `/`, `..` or a query into the upstream URL.
pub fn valid_exchange_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
}

async fn proxy(lever: Option<&Arc<Lever>>, path_and_query: &str) -> Response {
    let Some(lever) = lever else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "history needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    match lever.get(path_and_query).await {
        Ok(res) => {
            let status = res.status();
            let content_type = res
                .headers()
                .get(header::CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
            // Only status, content type and body cross over; no upstream headers.
            let mut out = Response::new(Body::new(res.into_body()));
            *out.status_mut() = status;
            out.headers_mut().insert(header::CONTENT_TYPE, content_type);
            out
        }
        Err(e) => {
            tracing::warn!("history proxy: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, "taphttp API unavailable")
        }
    }
}

/// Where traffic comes from, for the UI's status bar. `connected` is
/// whether spall is following taphttp's feed (null when tailing a file).
/// `replay` is whether replay is available (lever API mode), and `scope`
/// the engagement scope it's held to.
async fn api_status(State(state): State<AppState>) -> Response {
    let now = state.replayer.as_ref().map(|r| r.scope_now());
    let scope = now.as_ref().map(|s| s.entries()).unwrap_or_default();
    let scope_all = now.as_ref().is_some_and(|s| s.allows_all());
    let mut body = match &state.lever {
        Some(lever) => {
            serde_json::json!({ "source": "lever", "connected": lever.connected(), "proxy": &*state.proxy_address })
        }
        None => {
            serde_json::json!({ "source": "log", "connected": null, "proxy": &*state.proxy_address })
        }
    };
    body["replay"] = state.replayer.is_some().into();
    body["scope"] = scope.into();
    body["scope_all"] = scope_all.into();
    axum::Json(body).into_response()
}

/// The newest replay attempts from the audit log (`?last=N`, default 20,
/// at most 500), oldest first. Read-only; spall's own page and console.
async fn api_audit(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(replayer) = &state.replayer else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the audit log needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw.as_deref().unwrap_or("")).unwrap_or_default();
    let last = match pairs.iter().find(|(k, _)| k == "last") {
        None => 20,
        Some((_, v)) => match v.parse::<usize>() {
            Ok(n) => n.min(500),
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "last must be a number"),
        },
    };
    match replayer.audit.tail(last) {
        Ok(records) => axum::Json(records).into_response(),
        Err(e) => {
            tracing::warn!("reading the audit log: {e}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the audit log",
            )
        }
    }
}

/// Where a target stands (`scope check <url|host>`): `?target=` is an
/// absolute URL or a host (port allowed). Read-only.
async fn api_scope_check(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(replayer) = &state.replayer else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "scope needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw.as_deref().unwrap_or("")).unwrap_or_default();
    let Some(target) = pairs
        .iter()
        .find(|(k, _)| k == "target")
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
    else {
        return json_error(StatusCode::BAD_REQUEST, "want ?target=<url or host>");
    };
    let host = if target.contains("://") {
        match crate::gate::url_host(target) {
            Some(h) if !h.is_empty() => h,
            _ => return json_error(StatusCode::BAD_REQUEST, "that URL has no host"),
        }
    } else {
        target.to_string()
    };
    axum::Json(replayer.scope_now().explain(&host)).into_response()
}

/// Change the engagement scope for this run (`scope add|exclude|remove|all` in
/// the console). Same-origin only, like every POST; audited. Not offered
/// to agents.
async fn api_scope(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(replayer) = &state.replayer else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "scope needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    let Ok(change) = serde_json::from_slice::<crate::replay::ScopeChange>(&body) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            r#"want {"op":"add"|"exclude"|"remove","entry":"..."} or {"op":"all","on":bool}"#,
        );
    };
    match replayer.change_scope(&change) {
        Ok((scope, changed)) => axum::Json(serde_json::json!({
            "scope": scope.entries(),
            "all": scope.allows_all(),
            "changed": changed,
        }))
        .into_response(),
        Err(msg) => json_error(StatusCode::BAD_REQUEST, &msg),
    }
}

async fn api_replay(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    match &state.replayer {
        Some(r) => r.handle(&body).await,
        None => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "replay needs taphttp's lever API (start spall with --taphttp-api)",
        ),
    }
}

async fn api_exchanges(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(query) = forwardable_query(raw.as_deref()) else {
        return json_error(StatusCode::BAD_REQUEST, "malformed query string");
    };
    let path = if query.is_empty() {
        "/exchanges".to_string()
    } else {
        format!("/exchanges?{query}")
    };
    proxy(state.lever.as_ref(), &path).await
}

async fn api_exchange(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_exchange_id(&id) {
        return json_error(StatusCode::BAD_REQUEST, "invalid exchange id");
    }
    proxy(state.lever.as_ref(), &format!("/exchanges/{id}")).await
}

async fn healthz() -> &'static str {
    "ok"
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (history, rx) = state.hub.subscribe();
    let live = BroadcastStream::new(rx).filter_map(|r| async move {
        match r {
            Ok(item) => Some(item),
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("SSE client fell behind; skipped {n} entries");
                None
            }
        }
    });
    let stream = stream::iter(history)
        .chain(live)
        .map(|item| Ok(to_event(&item)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_event(item: &Published) -> Event {
    let event = Event::default().data(&item.data);
    match &item.id {
        Some(id) => event.id(id),
        None => event,
    }
}

/// Rejects requests whose Host is not allowlisted (DNS rebinding defense),
/// then state-changing requests not from spall's own page (CSRF, see
/// `csrf`), and adds security headers to every response.
async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().host());
    let allowed = match host {
        Some(h) => host_allowed(h, &state.allowed_hosts),
        None => false,
    };
    let mut res = if !allowed {
        (StatusCode::FORBIDDEN, "host not allowed\n").into_response()
    } else if let Err(why) = crate::csrf::check(req.method(), req.headers()) {
        // Same-origin guard (#17), on every route so new ones are covered
        // by default. No CORS headers are ever sent.
        json_error(StatusCode::FORBIDDEN, why)
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Whether a Host header value (with or without port) is allowlisted.
pub fn host_allowed(host_header: &str, allowed: &[String]) -> bool {
    let host = strip_port(host_header.trim());
    allowed
        .iter()
        .any(|a| a == "*" || a.eq_ignore_ascii_case(host))
}

fn strip_port(h: &str) -> &str {
    if h.starts_with('[') {
        return match h.find(']') {
            Some(i) => &h[..=i],
            None => h,
        };
    }
    match h.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use std::time::Duration;
    use tower::ServiceExt;

    fn defaults() -> Vec<String> {
        ["localhost", "127.0.0.1", "[::1]"]
            .map(String::from)
            .to_vec()
    }

    /// In-memory stand-in for the embedded `ui/dist`.
    struct FakeUi(std::collections::HashMap<&'static str, &'static [u8]>);

    impl UiAssets for FakeUi {
        fn get(&self, path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
            self.0.get(path).map(|b| std::borrow::Cow::Borrowed(*b))
        }
    }

    const FAKE_INDEX: &str = "<!doctype html><script type=\"module\" src=\"/assets/index-abc.js\"></script><div id=\"root\"></div>";

    fn fake_ui() -> Arc<dyn UiAssets> {
        Arc::new(FakeUi(
            [
                ("index.html", FAKE_INDEX.as_bytes()),
                ("assets/index-abc.js", b"console.log(1)".as_slice()),
                ("assets/index-abc.css", b"body{}".as_slice()),
                ("assets/font.woff2", b"wOF2".as_slice()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn app_with(hub: Arc<Hub>, ui: Arc<dyn UiAssets>) -> Router {
        router(AppState {
            hub,
            allowed_hosts: Arc::new(defaults()),
            lever: None,
            ui,
            proxy_address: Arc::from("127.0.0.1:8080"),
            replayer: None,
        })
    }

    fn app(hub: Arc<Hub>) -> Router {
        app_with(hub, fake_ui())
    }

    fn get_req(path: &str, host: &str) -> Request {
        Request::builder()
            .uri(path)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap()
    }

    async fn body_string(res: Response) -> String {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Read SSE body frames until `needle` appears (or time out).
    async fn read_until(body: &mut Body, needle: &str) -> String {
        let mut seen = String::new();
        let res = tokio::time::timeout(Duration::from_secs(5), async {
            while !seen.contains(needle) {
                let frame = body.frame().await.unwrap().unwrap();
                if let Some(data) = frame.data_ref() {
                    seen.push_str(std::str::from_utf8(data).unwrap());
                }
            }
        })
        .await;
        assert!(
            res.is_ok(),
            "timed out waiting for {needle:?}; got {seen:?}"
        );
        seen
    }

    #[test]
    fn only_known_history_params_are_forwarded() {
        assert_eq!(forwardable_query(None).unwrap(), "");
        assert_eq!(
            forwardable_query(Some("limit=5&token=evil&q=a+b&x=1&before=abc")).unwrap(),
            "limit=5&q=a+b&before=abc"
        );
        assert_eq!(
            forwardable_query(Some("q=%26token%3Devil")).unwrap(),
            "q=%26token%3Devil",
            "values are re-encoded, so they cannot add parameters"
        );
    }

    #[test]
    fn replay_of_is_forwarded_for_the_replays_tab() {
        assert_eq!(
            forwardable_query(Some("replay_of=3f2a1b4c&token=evil")).unwrap(),
            "replay_of=3f2a1b4c"
        );
    }

    #[test]
    fn exchange_ids_are_validated() {
        assert!(valid_exchange_id("3f2a1b4c-8d9e-4a7b-b3c1-2e5f6a7d8e9f"));
        for bad in [
            "",
            "../exchanges",
            "a/b",
            "a?b",
            "a b",
            "a%2Fb",
            &"x".repeat(129),
        ] {
            assert!(!valid_exchange_id(bad), "{bad}");
        }
    }

    #[test]
    fn host_allowlist_matching() {
        let a = defaults();
        for ok in [
            "localhost",
            "localhost:3000",
            "LOCALHOST:3000",
            "127.0.0.1:3000",
            "[::1]:3000",
            "[::1]",
        ] {
            assert!(host_allowed(ok, &a), "{ok}");
        }
        for bad in [
            "evil.com",
            "evil.com:3000",
            "localhost.evil.com",
            "127.0.0.2:3000",
            "",
            "[::2]:3000",
        ] {
            assert!(!host_allowed(bad, &a), "{bad}");
        }
        assert!(host_allowed("anything:1", &["*".to_string()]));
    }

    #[tokio::test]
    async fn healthz_ok() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(get_req("/healthz", "127.0.0.1:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_string(res).await, "ok");
    }

    #[tokio::test]
    async fn index_is_the_built_spa_with_strict_csp() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(get_req("/", "localhost:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        assert_eq!(
            res.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; \
             img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; \
             form-action 'none'",
            "the policy from docs/architecture.md (Frontend), plus form-action"
        );
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(body_string(res).await, FAKE_INDEX);
    }

    #[tokio::test]
    async fn built_assets_are_served_with_types() {
        for (path, ty) in [
            ("/assets/index-abc.js", "text/javascript"),
            ("/assets/index-abc.css", "text/css"),
            ("/assets/font.woff2", "font/woff2"),
        ] {
            let res = app(Arc::new(Hub::new(1)))
                .oneshot(get_req(path, "localhost"))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(
                res.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with(ty),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_or_escaping_asset_paths_are_404() {
        for path in [
            "/assets/nope.js",
            "/assets/..%2Findex.html",
            "/assets/%2e%2e/index.html",
            "/index.html",
        ] {
            let res = app(Arc::new(Hub::new(1)))
                .oneshot(get_req(path, "localhost"))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn without_a_built_ui_the_root_says_how_to_build_it() {
        let empty: Arc<dyn UiAssets> = Arc::new(FakeUi(Default::default()));
        let res = app_with(Arc::new(Hub::new(1)), empty)
            .oneshot(get_req("/", "localhost"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let text = body_string(res).await;
        assert!(text.contains("npm --prefix ui ci"), "{text}");
    }

    #[tokio::test]
    async fn foreign_host_is_rejected() {
        for path in [
            "/",
            "/events",
            "/healthz",
            "/assets/index-abc.js",
            "/api/exchanges",
        ] {
            let res = app(Arc::new(Hub::new(1)))
                .oneshot(get_req(path, "attacker.example:3000"))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn missing_host_is_rejected() {
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let res = app(Arc::new(Hub::new(1))).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    fn post_req(path: &str, headers: &[(&str, &str)]) -> Request {
        let mut b = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::HOST, "127.0.0.1:3000");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::from("{}")).unwrap()
    }

    #[tokio::test]
    async fn cross_site_posts_are_refused_on_every_route() {
        for path in ["/api/replay", "/api/exchanges", "/", "/nope"] {
            for headers in [
                &[
                    ("content-type", "text/plain"),
                    ("origin", "http://evil.example"),
                ][..],
                &[
                    ("content-type", "application/json"),
                    ("sec-fetch-site", "cross-site"),
                ][..],
                &[("content-type", "application/json")][..],
            ] {
                let res = app(Arc::new(Hub::new(1)))
                    .oneshot(post_req(path, headers))
                    .await
                    .unwrap();
                assert_eq!(res.status(), StatusCode::FORBIDDEN, "{path} {headers:?}");
                assert!(res.headers().get("access-control-allow-origin").is_none());
            }
        }
    }

    #[tokio::test]
    async fn same_origin_json_posts_get_past_the_guard() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(post_req(
                "/healthz",
                &[
                    ("content-type", "application/json"),
                    ("sec-fetch-site", "same-origin"),
                    ("origin", "http://127.0.0.1:3000"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn preflights_are_never_approved() {
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/api/replay")
            .header(header::HOST, "127.0.0.1:3000")
            .header("origin", "http://evil.example")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .body(Body::empty())
            .unwrap();
        let res = app(Arc::new(Hub::new(1))).oneshot(req).await.unwrap();
        assert!(!res.status().is_success(), "{}", res.status());
        for h in [
            "access-control-allow-origin",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "access-control-allow-credentials",
        ] {
            assert!(res.headers().get(h).is_none(), "{h}");
        }
    }

    #[tokio::test]
    async fn events_streams_history_then_live_entries() {
        let hub = Arc::new(Hub::new(10));
        hub.publish(Published {
            id: Some("old".into()),
            data: "{\"id\":\"old\"}".into(),
        });
        let res = app(hub.clone())
            .oneshot(get_req("/events", "127.0.0.1:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        let mut body = res.into_body();
        let seen = read_until(&mut body, "{\"id\":\"old\"}").await;
        assert!(seen.contains("id: old"), "{seen:?}");

        hub.publish(Published {
            id: Some("new".into()),
            data: "{\"id\":\"new\"}".into(),
        });
        let seen = read_until(&mut body, "data: {\"id\":\"new\"}").await;
        assert!(seen.contains("id: new"), "{seen:?}");
    }

    #[tokio::test]
    async fn events_omits_id_line_when_unsafe() {
        let hub = Arc::new(Hub::new(10));
        hub.publish(Published {
            id: None,
            data: "{\"x\":1}".into(),
        });
        let res = app(hub)
            .oneshot(get_req("/events", "localhost"))
            .await
            .unwrap();
        let mut body = res.into_body();
        let seen = read_until(&mut body, "data: {\"x\":1}").await;
        assert!(!seen.contains("id:"), "{seen:?}");
    }
}
