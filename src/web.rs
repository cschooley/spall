//! HTTP surface: the static console page, the SSE feed, and a health check.

use crate::hub::{Hub, Published};
use crate::lever::Lever;
use axum::body::Body;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{self, Stream, StreamExt};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_JS: &str = include_str!("../static/app.js");
const APP_CSS: &str = include_str!("../static/app.css");

/// The page renders attacker-controlled traffic. It never builds HTML from
/// it, and this policy forbids inline script and any off-origin fetch as a
/// second line of defense.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
style-src 'self'; connect-src 'self'; img-src 'none'; base-uri 'none'; \
form-action 'none'; frame-ancestors 'none'";

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub allowed_hosts: Arc<Vec<String>>,
    /// taphttp lever API client; `None` when tailing a log file.
    pub lever: Option<Arc<Lever>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/app.css", get(app_css))
        .route("/events", get(events))
        .route("/healthz", get(healthz))
        .route("/api/exchanges", get(api_exchanges))
        .route("/api/exchanges/{id}", get(api_exchange))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

/// Query parameters forwarded to taphttp's `GET /exchanges`. Anything else
/// (notably `token`) is dropped.
const EXCHANGE_PARAMS: &[&str] = &[
    "limit", "before", "host", "method", "status", "q", "since", "until",
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

/// Rejects requests whose Host is not allowlisted (DNS rebinding defense)
/// and adds security headers to every response.
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
    let mut res = if allowed {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "host not allowed\n").into_response()
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

    fn app(hub: Arc<Hub>) -> Router {
        router(AppState {
            hub,
            allowed_hosts: Arc::new(defaults()),
            lever: None,
        })
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
    async fn index_is_html_with_strict_csp() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(get_req("/", "localhost:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"));
        assert!(!csp.contains("unsafe-inline"));
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let html = body_string(res).await;
        assert!(html.contains("<script src=\"/app.js\""));
        assert!(
            !html.contains("<script>"),
            "no inline script allowed by CSP"
        );
    }

    #[tokio::test]
    async fn static_assets_are_served_with_types() {
        for (path, ty) in [("/app.js", "text/javascript"), ("/app.css", "text/css")] {
            let res = app(Arc::new(Hub::new(1)))
                .oneshot(get_req(path, "localhost"))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(res.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with(ty));
        }
    }

    #[tokio::test]
    async fn foreign_host_is_rejected() {
        for path in ["/", "/events", "/healthz", "/app.js"] {
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
