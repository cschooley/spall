//! HTTP surface: the built SPA (ui/dist, embedded), the SSE feed, the
//! history proxy, replay (through the action gate), and a health check.

use crate::engagement::Error as EngagementError;
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
    /// The MCP server (#58); `None` without the lever API.
    pub mcp: Option<Arc<crate::mcp::Server>>,
    /// spall's own agent host (#63); `None` without a model configured.
    pub host: Option<Arc<crate::host::Host>>,
    /// The intake (#65), on the host's model; `None` without one.
    pub intake: Option<Arc<crate::intake::Intake>>,
}

/// A task for spall's host when there's no model to run it.
fn no_host() -> Response {
    json_error(
        StatusCode::CONFLICT,
        "spall has no model configured to run a task itself: start it with --model-provider (and a key), or leave the task to an outside MCP client",
    )
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
        .route("/api/never-test", get(api_never).post(api_never_change))
        .route("/api/checks", get(api_checks).post(api_checks_change))
        .route("/api/sweep", post(api_sweep))
        .route("/api/oob", get(api_oob).post(api_oob_mint))
        .route("/api/alerts", get(api_alerts))
        .route("/api/scope/check", get(api_scope_check))
        .route("/api/hosts", get(api_hosts))
        .route("/api/rules", get(api_rules))
        .route("/api/commands", get(api_commands))
        .route(
            "/api/candidates",
            get(api_candidates).post(api_candidates_new),
        )
        .route(
            "/api/candidates/{id}",
            axum::routing::patch(api_candidate_patch),
        )
        .route(
            "/api/candidates/{id}/redaction",
            get(api_candidate_redaction),
        )
        .route("/api/export", post(api_export))
        .route("/api/tasks", get(api_tasks).post(api_tasks_new))
        .route("/api/estop", post(api_estop))
        .route("/api/quad", post(api_quad))
        .route("/api/intake", post(api_intake))
        .route("/api/plans", get(api_plans))
        .route("/api/plans/{id}", axum::routing::patch(api_plan_patch))
        .route(
            "/mcp",
            post(crate::mcp::handle_post)
                .get(crate::mcp::handle_get)
                .delete(crate::mcp::handle_delete),
        )
        .route("/api/estop/clear", post(api_estop_clear))
        .route("/api/tasks/{id}", get(api_task).patch(api_task_patch))
        .route("/api/verbs/{name}", axum::routing::post(api_verb))
        .route("/api/redaction", get(api_redaction))
        .route("/api/ca", get(api_ca))
        .route("/api/ca.pem", get(api_ca_pem))
        .route("/api/ca.der", get(api_ca_der))
        .route(
            "/api/openapi/parse",
            post(api_openapi_parse).layer(DefaultBodyLimit::max(2 * crate::openapi::MAX_BYTES)),
        )
        .route(
            "/api/scope/parse",
            post(api_scope_parse).layer(DefaultBodyLimit::max(crate::scope_list::MAX_BYTES + 1024)),
        )
        .route(
            "/api/engagement",
            get(api_engagement).patch(api_engagement_patch),
        )
        .route(
            "/api/engagements",
            get(api_engagements).post(api_engagements_new),
        )
        .route("/api/engagements/active", post(api_engagements_active))
        .route(
            "/api/replay",
            post(api_replay).layer(DefaultBodyLimit::max(crate::replay::MAX_REQUEST_BYTES)),
        )
        .route(
            "/api/fetch",
            post(api_fetch).layer(DefaultBodyLimit::max(crate::replay::MAX_REQUEST_BYTES)),
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
    body["engagement"] = match &state.replayer {
        Some(r) => {
            let e = r.engagements.active();
            serde_json::json!({ "id": e.id, "name": e.name })
        }
        None => serde_json::Value::Null,
    };
    body["testing"] = state
        .replayer
        .as_ref()
        .map(|r| r.testing_now())
        .unwrap_or(serde_json::Value::Null);
    body["flux"] = serde_json::json!({
        "crate": crate::rules::FLUX_TAG,
        "spec": crate::rules::FLUX_SPEC,
        "rules": format!("OWASP WSTG v{}", crate::rules::WSTG_VERSION),
    });
    // Candidates waiting for a decision (#47): not yet confirmed, not
    // suppressed.
    body["export"] = match &state.replayer {
        Some(r) => serde_json::json!({
            "promoted": r.candidates(None).iter().filter(|c| c.promoted.is_some() && c.promotable()).count(),
            "unredacted_allowed": r.allow_unredacted_export,
        }),
        None => serde_json::Value::Null,
    };
    body["to_triage"] = match &state.replayer {
        Some(r) => r
            .candidates(None)
            .iter()
            .filter(|c| crate::candidates::to_triage(c))
            .count()
            .into(),
        None => serde_json::Value::Null,
    };
    // Tasks waiting on a human's approval (#56).
    body["waiting_approval"] = match &state.replayer {
        Some(r) => r.tasks.waiting(&r.engagements.active().id).into(),
        None => serde_json::Value::Null,
    };
    // spall's own agent host (#63): null, or {model}. Never the key.
    body["agent_host"] = match &state.host {
        Some(h) => serde_json::json!({ "model": h.describe() }),
        None => serde_json::Value::Null,
    };
    // The agent e-stop (#57): null, or {since, by, reason}.
    body["estop"] = match &state.replayer {
        Some(r) => serde_json::to_value(r.tasks.estop()).unwrap_or_default(),
        None => serde_json::Value::Null,
    };
    // Alerts (#86): the newest seq, so the page fetches only when it moves.
    body["alerts"] = state
        .replayer
        .as_ref()
        .map(|r| serde_json::json!(r.tasks.alerts.latest()))
        .unwrap_or_default();
    // OOB (#1): the callback domain and the active engagement's hits, or null.
    body["oob"] = state
        .replayer
        .as_ref()
        .and_then(|r| {
            r.oob.as_ref().map(|o| {
                serde_json::json!({
                    "domain": o.settings.domain,
                    "hits": o.list(&r.engagements.active().id, None).1.len(),
                })
            })
        })
        .unwrap_or_default();
    // Quad (#66): the burst underway or null, and the last scorecard.
    body["quad"] = state
        .replayer
        .as_ref()
        .and_then(|r| r.quad_now())
        .map(|on| quad_json(&on))
        .unwrap_or_default();
    body["last_quad"] = state
        .replayer
        .as_ref()
        .and_then(|r| r.quad.last())
        .map(|c| card_json(&c))
        .unwrap_or_default();
    axum::Json(body).into_response()
}

fn quad_json(on: &crate::quad::On) -> serde_json::Value {
    let left = on
        .until
        .parse::<jiff::Timestamp>()
        .map(|u| u.duration_since(jiff::Timestamp::now()).as_secs().max(0))
        .unwrap_or(0);
    let mut v = serde_json::to_value(on).unwrap_or_default();
    v["remaining_secs"] = left.into();
    v
}

fn card_json(c: &crate::quad::Scorecard) -> serde_json::Value {
    let mut v = serde_json::to_value(c).unwrap_or_default();
    v["card"] = c.card().into();
    v
}

/// The intake (#65): `{"text", "pick"?: {"question", "answer"}}` in, a draft
/// plan (or "no.") out. Needs spall's model; plans only, never acts.
async fn api_intake(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    if state.replayer.is_none() {
        return no_tasks();
    }
    let Some(intake) = &state.intake else {
        return json_error(
            StatusCode::CONFLICT,
            "the intake needs spall's model (--model-provider and a key); the agent buttons and + New task work without one",
        );
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pick {
        question: String,
        answer: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ask {
        text: String,
        pick: Option<Pick>,
    }
    let ask: Ask = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"text", "pick"?: {{"question", "answer"}}}}: {e}"#),
            )
        }
    };
    let pick = ask
        .pick
        .as_ref()
        .map(|p| (p.question.as_str(), p.answer.as_str()));
    match intake.ask(&ask.text, pick).await {
        Ok(crate::intake::Answer::Plan { plan }) => {
            axum::Json(serde_json::json!({ "kind": "plan", "plan": plan_json(&plan) }))
                .into_response()
        }
        Ok(a) => axum::Json(a).into_response(),
        Err(why) => json_error(StatusCode::CONFLICT, &why),
    }
}

/// A plan with its estimate ("2 tasks, ~10 requests, safe methods only").
fn plan_json(p: &crate::plans::Plan) -> serde_json::Value {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    v["estimate"] = crate::plans::estimate(&p.tasks).into();
    v
}

/// The intake's plans, newest first, with their estimates.
async fn api_plans(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    let plans: Vec<serde_json::Value> = r
        .plans
        .list()
        .into_iter()
        .take(50)
        .map(|p| plan_json(&p))
        .collect();
    axum::Json(serde_json::json!({ "plans": plans })).into_response()
}

/// A human's change to a plan: `{"op": "drop", "index"}`, `{"op":
/// "budget", "index", "budget"}`, `{"op": "discard"}` or `{"op":
/// "approve"}` (makes and approves its tasks). Same-origin, audited.
async fn api_plan_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    let op: crate::plans::Op = match serde_json::from_slice(&body) {
        Ok(o) => o,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!("not a plan change (drop, budget, discard, approve): {e}"),
            )
        }
    };
    let spall_run = matches!(op, crate::plans::Op::Approve)
        && r.plans.find(&id).is_some_and(|p| {
            p.tasks
                .iter()
                .any(|t| t.task.runner == Some(crate::tasks::Runner::Spall))
        });
    if spall_run && state.host.is_none() {
        return no_host();
    }
    match r.plan_op(&id, op) {
        Ok(p) => axum::Json(plan_json(&p)).into_response(),
        Err((status, why)) => json_error(status, &why),
    }
}

/// Quad mode (#66): `{"seconds"?}` starts a burst, `{"stop": true}` ends it
/// early with its scorecard. A human's, never an agent's; audited.
async fn api_quad(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ask {
        seconds: Option<u64>,
        #[serde(default)]
        stop: bool,
    }
    let ask: Ask = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"seconds"?}} or {{"stop": true}}: {e}"#),
            )
        }
    };
    if ask.stop {
        return match r.quad_end(crate::quad::Ended::Stopped) {
            Some(c) => {
                axum::Json(serde_json::json!({ "scorecard": card_json(&c) })).into_response()
            }
            None => json_error(StatusCode::CONFLICT, "quad isn't on"),
        };
    }
    match r.quad_start(ask.seconds) {
        Ok(on) => (
            StatusCode::CREATED,
            axum::Json(serde_json::json!({ "quad": quad_json(&on) })),
        )
            .into_response(),
        Err((status, why)) => json_error(status, &why),
    }
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

/// The CA's public details for browser setup (#32): fingerprint, subject
/// and validity from taphttp's `/info`, nothing else from it.
async fn api_ca(State(state): State<AppState>) -> Response {
    let Some(lever) = &state.lever else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the CA certificate comes from taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    match lever.get_bytes("/info").await {
        Ok((StatusCode::OK, body)) => {
            let info: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let ca = info.get("ca").cloned().unwrap_or_default();
            let field = |k: &str| ca.get(k).and_then(|v| v.as_str()).map(String::from);
            axum::Json(serde_json::json!({
                "sha256": field("sha256"),
                "subject": field("subject"),
                "not_before": field("not_before"),
                "not_after": field("not_after"),
                "taphttp_version": info.get("taphttp_version").and_then(|v| v.as_str()),
                "proxy": &*state.proxy_address,
            }))
            .into_response()
        }
        Ok((status, _)) => json_error(
            StatusCode::BAD_GATEWAY,
            &format!("taphttp answered {status} for /info (taphttp 0.6.0 or later)"),
        ),
        Err(e) => {
            tracing::warn!("ca info: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, "taphttp API unavailable")
        }
    }
}

/// The public CA certificate as a download (taphttp serves only the
/// certificate, never the key). The token stays server-side.
async fn ca_file(
    state: &AppState,
    path: &str,
    content_type: &'static str,
    name: &'static str,
) -> Response {
    let Some(lever) = &state.lever else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the CA certificate comes from taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    match lever.get_bytes(path).await {
        Ok((StatusCode::OK, body)) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                (
                    header::CONTENT_DISPOSITION,
                    HeaderValue::from_static(match name {
                        "taphttp-ca.der" => "attachment; filename=\"taphttp-ca.der\"",
                        _ => "attachment; filename=\"taphttp-ca.pem\"",
                    }),
                ),
                (
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            body,
        )
            .into_response(),
        Ok((StatusCode::NOT_FOUND, _)) => json_error(
            StatusCode::BAD_GATEWAY,
            "this taphttp doesn't serve its CA certificate (needs taphttp 0.6.1 or later)",
        ),
        Ok((status, _)) => json_error(
            StatusCode::BAD_GATEWAY,
            &format!("taphttp answered {status}"),
        ),
        Err(e) => {
            tracing::warn!("ca download: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, "taphttp API unavailable")
        }
    }
}

async fn api_ca_pem(State(state): State<AppState>) -> Response {
    ca_file(
        &state,
        "/ca.pem",
        "application/x-pem-file",
        "taphttp-ca.pem",
    )
    .await
}

async fn api_ca_der(State(state): State<AppState>) -> Response {
    ca_file(
        &state,
        "/ca.der",
        "application/x-x509-ca-cert",
        "taphttp-ca.der",
    )
    .await
}

/// Read an OpenAPI / Swagger file (#40): its servers as a scope preview
/// (like a pasted list) and its paths. Changes nothing; the UI applies the
/// scope entries and saves the spec through the usual audited routes.
async fn api_openapi_parse(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(replayer) = &state.replayer else {
        return no_engagements();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Text {
        text: String,
    }
    let Ok(t) = serde_json::from_slice::<Text>(&body) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            r#"want {"text": "<the spec, JSON or YAML>"}"#,
        );
    };
    // Parsing a few MiB of YAML is CPU work: off the async threads.
    let scope = replayer.scope_now().with_all(false);
    let now = taphttp_types::Timestamp::now().to_string();
    match tokio::task::spawn_blocking(move || crate::openapi::import(&t.text, &scope, &now)).await {
        Ok(Ok(import)) => axum::Json(import).into_response(),
        Ok(Err(m)) => json_error(StatusCode::BAD_REQUEST, &m),
        Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "reading the spec failed"),
    }
}

/// Hosts seen in traffic with their scope state (#39), most seen first.
/// `?state=in|out|unknown` narrows the list (counts cover every host).
/// Without the lever API there's no engagement, so every host is unknown.
fn no_candidates() -> Response {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "candidates need taphttp's lever API (start spall with --taphttp-api)",
    )
}

/// The active engagement's candidates (#42), newest first;
/// `?exchange=<id>` for those an exchange backs.
async fn api_candidates(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(r) = &state.replayer else {
        return no_candidates();
    };
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw.as_deref().unwrap_or("")).unwrap_or_default();
    let exchange = pairs
        .iter()
        .find(|(k, _)| k == "exchange")
        .map(|(_, v)| v.as_str());
    axum::Json(serde_json::json!({ "candidates": r.candidates(exchange) })).into_response()
}

/// Flag an exchange as a candidate (#42, #43).
async fn api_candidates_new(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_candidates();
    };
    match serde_json::from_slice::<crate::candidates::Flag>(&body) {
        Ok(flag) => r.flag(flag).await,
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            &format!(
                r#"want {{"exchange", "rule", "parameter"?, "level"?, "rank"?, "message"?, "replays"?, "cwe"?}}: {e}"#
            ),
        ),
    }
}

/// Triage or edit a candidate: `{"op": "confirm" | "reject" | "accept" |
/// "duplicate" | "reopen" | "edit" | "evidence", ...}` (#42, #44).
async fn api_candidate_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_candidates();
    };
    match serde_json::from_slice::<crate::candidates::Op>(&body) {
        Ok(op) => r.triage(&id, op).await,
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            &format!("not a candidate change: {e}"),
        ),
    }
}

/// A candidate as it would be exported, and what redaction changes (#49).
async fn api_candidate_redaction(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_candidates();
    };
    r.redaction(&id)
}

fn no_tasks() -> Response {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "tasks need taphttp's lever API (start spall with --taphttp-api)",
    )
}

/// Engage the agent e-stop (#57): `{"reason"?}`. Same-origin JSON like
/// every change; an empty or unreadable body still stops.
async fn api_estop(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    let reason = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from))
        .unwrap_or_default();
    r.engage_estop(&reason)
}

/// Clear the e-stop: `{}`. Halted tasks stay halted.
async fn api_estop_clear(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    r.clear_estop()
}

/// The active engagement's tasks (#54), newest first, without logs.
async fn api_tasks(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    axum::Json(serde_json::json!({ "tasks": r.tasks() })).into_response()
}

/// Make a draft task: `{"goal", "role", "constraints"?, "plan"?, "note"?}`.
async fn api_tasks_new(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    match serde_json::from_slice::<crate::tasks::New>(&body) {
        Ok(new) if new.runner == Some(crate::tasks::Runner::Spall) && state.host.is_none() => {
            no_host()
        }
        Ok(new) => r.create_task(new),
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            &format!(r#"want {{"goal", "role", "constraints"?, "plan"?, "note"?}}: {e}"#),
        ),
    }
}

/// A verb on an object (#67): `{"id", "category"?}` drafts the task it
/// pre-fills; 201 `{"task", "cost"}`. Drafts only; same-origin, audited.
async fn api_verb(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ask {
        id: String,
        category: Option<String>,
    }
    let ask: Ask = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"id", "category"?}}: {e}"#),
            )
        }
    };
    match r
        .verb("human", &name, &ask.id, ask.category.as_deref())
        .await
    {
        Ok((t, cost)) => (
            StatusCode::CREATED,
            axum::Json(serde_json::json!({ "task": t, "cost": cost })),
        )
            .into_response(),
        Err((status, why)) => json_error(status, &why),
    }
}

/// One task of the active engagement, log included.
async fn api_task(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    match r.task(&id) {
        Some(t) => axum::Json(t).into_response(),
        None => json_error(StatusCode::NOT_FOUND, "no such task"),
    }
}

/// A human's change to a task: `{"op": "approve" | "halt" | "edit", ...}`,
/// or an answer to its paused request: `{"op": "approve_once" |
/// "approve_for_task" | "deny", "reason"?}` (#56).
async fn api_task_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    if let Ok(op) = serde_json::from_slice::<crate::tasks::Op>(&body) {
        if matches!(
            op,
            crate::tasks::Op::Edit {
                runner: Some(crate::tasks::Runner::Spall),
                ..
            }
        ) && state.host.is_none()
        {
            return no_host();
        }
        return r.task_op(&id, op);
    }
    match serde_json::from_slice::<crate::tasks::Decision>(&body) {
        Ok(d) => r.task_decide(&id, d),
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            &format!("not a task change (approve, edit, halt, approve_once, approve_for_task, deny): {e}"),
        ),
    }
}

/// Export the promoted candidates as a flux report (#51):
/// `{"unredacted"?: bool}`. Same-origin JSON; audited.
async fn api_export(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_candidates();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ask {
        #[serde(default)]
        unredacted: bool,
    }
    match serde_json::from_slice::<Ask>(&body) {
        Ok(ask) => r.export(ask.unredacted),
        Err(_) => json_error(StatusCode::BAD_REQUEST, r#"want {"unredacted"?: bool}"#),
    }
}

/// What flux redacts (#49), for the inspector's credential highlighting:
/// the secret header names and the sensitive parameter names.
async fn api_redaction() -> Response {
    let headers: Vec<&str> = flux::redact::SECRET_HEADERS
        .iter()
        .map(|(h, _)| *h)
        .collect();
    let names: Vec<&str> = flux::redact::SECRET_NAMES.iter().map(|(n, _)| *n).collect();
    axum::Json(serde_json::json!({ "headers": headers, "names": names })).into_response()
}

/// The rule catalog (#41): WSTG v4.2 ids, titles, CWE and help links.
/// The command registry (#53): what the palette, console and MCP offer.
async fn api_commands() -> Response {
    axum::Json(crate::commands::specs()).into_response()
}

async fn api_rules() -> Response {
    let rules: Vec<serde_json::Value> = crate::rules::CATALOG
        .iter()
        .map(|e| serde_json::json!({"id": e.id, "title": e.title, "cwe": e.cwe, "help": e.help_uri()}))
        .collect();
    axum::Json(serde_json::json!({
        "wstg": crate::rules::WSTG_VERSION,
        "attribution": crate::rules::ATTRIBUTION,
        "rules": rules,
    }))
    .into_response()
}

async fn api_hosts(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let scope = state
        .replayer
        .as_ref()
        .map(|r| r.scope_now())
        .unwrap_or_default();
    let pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw.as_deref().unwrap_or("")).unwrap_or_default();
    let want = pairs
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.as_str());
    let want = match want {
        None => None,
        Some("in") => Some(crate::gate::HostState::In),
        Some("out") => Some(crate::gate::HostState::Out),
        Some("unknown") => Some(crate::gate::HostState::Unknown),
        Some(_) => return json_error(StatusCode::BAD_REQUEST, "state must be in, out or unknown"),
    };
    let mut listing = state.hub.hosts.list(&scope);
    if let Some(w) = want {
        listing.hosts.retain(|h| h.state == w);
    }
    axum::Json(listing).into_response()
}

/// Preview a pasted scope list (#38): what each line becomes, against the
/// current scope. Changes nothing; POST only because the text can be big.
/// Same-origin JSON like every POST.
async fn api_scope_parse(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(replayer) = &state.replayer else {
        return no_engagements();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Text {
        text: String,
    }
    let Ok(t) = serde_json::from_slice::<Text>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, r#"want {"text": "..."}"#);
    };
    match crate::scope_list::parse(&t.text, &replayer.scope_now().with_all(false)) {
        Ok(items) => {
            let (add, exclude) = crate::scope_list::to_apply(&items);
            axum::Json(serde_json::json!({ "items": items, "add": add, "exclude": exclude }))
                .into_response()
        }
        Err(m) => json_error(StatusCode::BAD_REQUEST, &m),
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
            r#"want {"op":"add"|"exclude"|"remove","entry":"..."}, {"op":"apply","add":[...],"exclude":[...]} or {"op":"all","on":bool}"#,
        );
    };
    match replayer.change_scope(&change) {
        Ok((scope, changed)) => axum::Json(serde_json::json!({
            "scope": scope.entries(),
            "all": scope.allows_all(),
            "changed": changed,
        }))
        .into_response(),
        Err(e) => engagement_error(&e),
    }
}

/// The global never-test list (#84) and the active engagement's
/// exceptions to it.
async fn api_never(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the never-test list needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    axum::Json(serde_json::json!({
        "entries": r.engagements.never_list(),
        "exceptions": r.engagements.active().never_test_exceptions,
    }))
    .into_response()
}

/// Change the never-test list: `{"op": "add" | "remove", "entry"}`. A
/// human's `policy` change (same-origin), audited.
async fn api_never_change(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the never-test list needs taphttp's lever API (start spall with --taphttp-api)",
        );
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Change {
        op: String,
        entry: String,
    }
    let c: Change = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"op": "add"|"remove", "entry"}}: {e}"#),
            )
        }
    };
    let add = match c.op.as_str() {
        "add" => true,
        "remove" => false,
        _ => return json_error(StatusCode::BAD_REQUEST, r#"op: "add" or "remove""#),
    };
    match r.change_never(add, &c.entry) {
        Ok((entries, changed)) => {
            axum::Json(serde_json::json!({ "entries": entries, "changed": changed }))
                .into_response()
        }
        Err(e) => engagement_error(&e),
    }
}

/// The checks a sweep may run (#75): built-in, then the engagement's.
async fn api_checks(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    axum::Json(serde_json::json!({ "checks": r.checks() })).into_response()
}

/// Import or remove an engagement check: `{"op": "add", "text"}` or
/// `{"op": "remove", "name"}`. The tester's `policy` change, audited.
async fn api_checks_change(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Change {
        op: String,
        text: Option<String>,
        name: Option<String>,
    }
    let c: Change = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"op": "add", "text"}} or {{"op": "remove", "name"}}: {e}"#),
            )
        }
    };
    let res = match (c.op.as_str(), &c.text, &c.name) {
        ("add", Some(t), None) => r.change_checks(Some(t), None),
        ("remove", None, Some(n)) => r.change_checks(None, Some(n)),
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                r#"want {"op": "add", "text"} or {"op": "remove", "name"}"#,
            )
        }
    };
    match res {
        Ok(checks) => axum::Json(serde_json::json!({ "checks": checks })).into_response(),
        Err(e) => engagement_error(&e),
    }
}

/// A sweep (#75): `{"base", "check", "yes"?}`. Without `yes`, what it
/// would cost; with it, the run and its summary. Same-origin; every request
/// is gated and audited.
async fn api_sweep(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ask {
        base: String,
        check: String,
        #[serde(default)]
        yes: bool,
    }
    let a: Ask = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                &format!(r#"want {{"base", "check", "yes"?}}: {e}"#),
            )
        }
    };
    match r.sweep(None, &a.base, &a.check, a.yes).await {
        Ok(v) => axum::Json(v).into_response(),
        Err((status, why)) => json_error(status, &why),
    }
}

/// Tester alerts (#86) after `?after=<seq>`: kinds and ids only.
async fn api_alerts(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    let after: u64 = raw
        .as_deref()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("after="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    axum::Json(serde_json::json!({
        "alerts": r.tasks.alerts.after(after),
        "latest": r.tasks.alerts.latest(),
    }))
    .into_response()
}

/// OOB callbacks (#1) in the active engagement: whether a server is set
/// up, the payloads and the hits (raw requests are captured data: shown
/// as text).
async fn api_oob(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    let Some(o) = &r.oob else {
        return axum::Json(serde_json::json!({ "configured": false })).into_response();
    };
    let (payloads, hits) = o.list(&r.engagements.active().id, None);
    axum::Json(serde_json::json!({
        "configured": true,
        "domain": o.settings.domain,
        "payloads": payloads,
        "hits": hits,
    }))
    .into_response()
}

/// Mint a payload for the tester (#1). Same-origin; sends nothing.
async fn api_oob_mint(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_tasks();
    };
    match r.oob_mint(None).await {
        Ok(p) => (StatusCode::CREATED, axum::Json(p)).into_response(),
        Err((status, why)) => json_error(status, &why),
    }
}

fn engagement_error(e: &EngagementError) -> Response {
    match e {
        EngagementError::Invalid(m) => json_error(StatusCode::BAD_REQUEST, m),
        EngagementError::NotFound(m) => json_error(StatusCode::NOT_FOUND, m),
        EngagementError::Io(m) => {
            tracing::error!("{m}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, m)
        }
    }
}

fn no_engagements() -> Response {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "engagements need taphttp's lever API (start spall with --taphttp-api)",
    )
}

/// The active engagement (#35), with `scope_all` for this process.
async fn api_engagement(State(state): State<AppState>) -> Response {
    let Some(r) = &state.replayer else {
        return no_engagements();
    };
    let mut body = serde_json::to_value(r.engagements.active()).unwrap_or_default();
    body["scope_all"] = r.scope_now().allows_all().into();
    body["testing_now"] = r.testing_now();
    axum::Json(body).into_response()
}

/// Change the active engagement's details (not its scope: `/api/scope`).
/// Same-origin JSON only; audited with before/after.
async fn api_engagement_patch(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_engagements();
    };
    let patch = match serde_json::from_slice::<crate::engagement::Patch>(&body) {
        Ok(p) => p,
        Err(e) => {
            let hint = if e.to_string().contains("`scope`") {
                "change scope with POST /api/scope (`scope` in the console)".to_string()
            } else {
                format!("bad engagement change: {e}")
            };
            return json_error(StatusCode::BAD_REQUEST, &hint);
        }
    };
    match r.update_engagement(patch) {
        Ok((e, changed)) => {
            axum::Json(serde_json::json!({ "engagement": e, "changed": changed })).into_response()
        }
        Err(e) => engagement_error(&e),
    }
}

async fn api_engagements(State(state): State<AppState>) -> Response {
    match &state.replayer {
        Some(r) => axum::Json(r.engagements.list()).into_response(),
        None => no_engagements(),
    }
}

/// Create an engagement and make it active. Same-origin JSON; audited.
async fn api_engagements_new(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let Some(r) = &state.replayer else {
        return no_engagements();
    };
    let Ok(new) = serde_json::from_slice::<crate::engagement::New>(&body) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            r#"want {"name", "client"?, "scope"?: [...]}"#,
        );
    };
    match r.create_engagement(new) {
        Ok(e) => (StatusCode::CREATED, axum::Json(e)).into_response(),
        Err(e) => engagement_error(&e),
    }
}

/// Switch the active engagement: `{"id": id or name}`. Audited.
async fn api_engagements_active(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Response {
    let Some(r) = &state.replayer else {
        return no_engagements();
    };
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Which {
        id: String,
    }
    let Ok(which) = serde_json::from_slice::<Which>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, r#"want {"id": "<id or name>"}"#);
    };
    match r.activate_engagement(&which.id) {
        Ok(e) => axum::Json(e).into_response(),
        Err(e) => engagement_error(&e),
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

/// A new GET or HEAD (#74), through the same gate as a replay.
async fn api_fetch(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    match &state.replayer {
        Some(r) => r.handle_fetch(&body).await,
        None => json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fetch needs taphttp's lever API (start spall with --taphttp-api)",
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
    } else if let Err(why) = if req.uri().path() == "/mcp" {
        crate::csrf::check_mcp(req.method(), req.headers())
    } else {
        crate::csrf::check(req.method(), req.headers())
    } {
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
            mcp: None,
            host: None,
            intake: None,
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
    async fn rules_list_the_wstg_catalog() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(get_req("/api/rules", "127.0.0.1:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        assert_eq!(v["wstg"], "4.2");
        assert!(v["attribution"].as_str().unwrap().contains("CC BY-SA 4.0"));
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules.len(), crate::rules::CATALOG.len());
        let sqli = rules.iter().find(|r| r["id"] == "WSTG-INPV-05").unwrap();
        assert_eq!(sqli["cwe"], 89);
        assert!(sqli["help"]
            .as_str()
            .unwrap()
            .ends_with("05-Testing_for_SQL_Injection"));
        assert_eq!(rules.last().unwrap()["cwe"], serde_json::Value::Null);
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
        for path in [
            "/api/replay",
            "/api/exchanges",
            "/api/candidates",
            "/",
            "/nope",
        ] {
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
    async fn candidates_need_the_lever_api() {
        let res = app(Arc::new(Hub::new(1)))
            .oneshot(get_req("/api/candidates", "127.0.0.1:3000"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_string(res).await.contains("lever API"));
    }

    #[tokio::test]
    async fn cross_site_patches_are_refused_too() {
        for headers in [
            &[
                ("content-type", "application/json"),
                ("origin", "http://evil.example"),
            ][..],
            &[("content-type", "application/json")][..],
        ] {
            let mut req = post_req("/api/candidates/c-1", headers);
            *req.method_mut() = axum::http::Method::PATCH;
            let res = app(Arc::new(Hub::new(1))).oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{headers:?}");
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
