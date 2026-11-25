//! The sandbox sensor (#13, docs/sandbox.md): `spall sandboxd`, the one
//! process that holds Docker, so spall itself never does. Under compose,
//! spall's image has no Docker and never gets the socket (that would hand
//! the app that shows raw traffic root on the host); `sandboxd` gets it and
//! nothing else, and offers spall a deliberately narrow API:
//!
//! - `GET /check`: whether sandboxes can run here, and whether a network
//!   was configured;
//! - `POST /exec`: one argv command in a task's sandbox, optionally with a
//!   per-command proxy credential and taphttp's CA;
//! - `POST /teardown`, `GET /live`;
//! - `POST /network`: check the configured network and switch it on.
//!
//! What it can't be asked for is the point: no image, mount, flag,
//! network or container name comes from the caller. The image, network and
//! taphttp's listeners are this process's own configuration, and every
//! container gets [`crate::sandbox::run_flags`]. A caller needs the token
//! (created here, 0640 so spall's group can read it); a browser is refused
//! whatever it carries.
//!
//! [`Remote`] is spall's side: the same [`Sandbox`] trait, over HTTP, so
//! the gate, the stop path and the tests can't tell it from a local one.

use crate::model::BoxFuture;
use crate::sandbox::{Cmd, Key, Net, NetCfg, Run, Runsc, Sandbox};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// `spall sandboxd` flags.
#[derive(clap::Parser, Debug)]
#[command(
    name = "spall sandboxd",
    about = "The sandbox sensor: runs agents' tool sandboxes for spall (#13)"
)]
pub struct Args {
    /// Where spall reaches it (the compose network only; never published).
    #[arg(long, env = "SPALL_SANDBOXD_LISTEN", default_value = "127.0.0.1:8070")]
    pub listen: std::net::SocketAddr,
    /// The token spall must send; created (0640) if missing.
    #[arg(
        long,
        env = "SPALL_SANDBOXD_TOKEN_FILE",
        default_value = "/sandbox-token/token"
    )]
    pub token_file: PathBuf,
    /// The toolset image (sandbox/Dockerfile).
    #[arg(long, env = "SPALL_SANDBOX_IMAGE", default_value = crate::sandbox::IMAGE_DEFAULT)]
    pub image: String,
    /// The internal network sandboxes join once it checks out (slice 2).
    #[arg(long, env = "SPALL_SANDBOX_NETWORK")]
    pub network: Option<String>,
    /// taphttp's credential-only listener, as seen from that network.
    #[arg(long, env = "SPALL_SANDBOX_PROXY", default_value = "taphttp:8090")]
    pub proxy: String,
    /// taphttp's open listener, which must *not* answer from that network.
    #[arg(long, env = "SPALL_SANDBOX_FENCE", default_value = "taphttp:8080")]
    pub fence: String,
}

/// Read the token at `path`, or create one there: 32 random bytes, hex,
/// mode 0640 (spall reads it through its group).
pub fn token(path: &Path) -> anyhow::Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    let t: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o640)
        .open(path)?;
    f.write_all(t.as_bytes())?;
    Ok(t)
}

/// The daemon's state: the sandbox (or why it can't run yet) and the token.
pub struct Daemon {
    pub sandbox: RwLock<Result<Arc<dyn Sandbox>, String>>,
    pub token: String,
}

impl Daemon {
    fn ready(&self) -> Result<Arc<dyn Sandbox>, String> {
        self.sandbox
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

fn bad(status: StatusCode, why: &str) -> Response {
    (status, Json(serde_json::json!({"error": why}))).into_response()
}

/// Bearer token, compared in constant time; and no browsers: a request
/// carrying an Origin came from a page, whatever else it has.
async fn guard(State(d): State<Arc<Daemon>>, req: Request, next: Next) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return bad(StatusCode::FORBIDDEN, "not for browsers");
    }
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let ok = given.len() == d.token.len()
        && given
            .bytes()
            .zip(d.token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !ok {
        return bad(StatusCode::UNAUTHORIZED, "bad token");
    }
    next.run(req).await
}

/// An engagement id (lowercase letters, digits, `-`) and a task id (`T-n`).
fn key_of(engagement: &str, task: &str) -> Option<Key> {
    let eng_ok = (1..=48).contains(&engagement.len())
        && engagement
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let task_ok = task.len() <= 12
        && task
            .strip_prefix("T-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if !(eng_ok && task_ok) {
        return None;
    }
    Some(Key {
        engagement: engagement.into(),
        task: task.into(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecBody {
    engagement: String,
    task: String,
    argv: Vec<String>,
    timeout_seconds: Option<u64>,
    #[serde(default)]
    net: Option<NetBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetBody {
    proxy_url: String,
    ca_pem: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyBody {
    engagement: String,
    task: String,
}

async fn check(State(d): State<Arc<Daemon>>) -> Response {
    match d.ready() {
        Ok(s) => Json(serde_json::json!({"ok": true, "network": s.has_network()})).into_response(),
        Err(why) => Json(serde_json::json!({"ok": false, "reason": why})).into_response(),
    }
}

async fn exec(State(d): State<Arc<Daemon>>, Json(b): Json<ExecBody>) -> Response {
    let sb = match d.ready() {
        Ok(s) => s,
        Err(why) => return bad(StatusCode::SERVICE_UNAVAILABLE, &why),
    };
    let key = match key_of(&b.engagement, &b.task) {
        Some(k) => k,
        None => return bad(StatusCode::BAD_REQUEST, "not an engagement and task id"),
    };
    let cmd = match Cmd::new(b.argv, b.timeout_seconds) {
        Ok(c) => c,
        Err(why) => return bad(StatusCode::BAD_REQUEST, &why),
    };
    if b.net
        .as_ref()
        .is_some_and(|n| n.ca_pem.len() > 64 * 1024 || n.proxy_url.len() > 1024)
    {
        return bad(StatusCode::BAD_REQUEST, "net too large");
    }
    let net = b.net.map(|n| Net {
        proxy_url: n.proxy_url,
        ca_pem: n.ca_pem,
    });
    // If spall hangs up (a halt, the e-stop), this future is dropped and
    // `docker exec` with it; spall then tears the sandbox down.
    match sb.exec(&key, &cmd, net.as_ref()).await {
        Ok(run) => Json(run).into_response(),
        Err(why) => bad(StatusCode::BAD_GATEWAY, &why),
    }
}

async fn teardown(State(d): State<Arc<Daemon>>, Json(b): Json<KeyBody>) -> Response {
    let key = match key_of(&b.engagement, &b.task) {
        Some(k) => k,
        None => return bad(StatusCode::BAD_REQUEST, "not an engagement and task id"),
    };
    if let Ok(sb) = d.ready() {
        sb.teardown(&key).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn live(State(d): State<Arc<Daemon>>) -> Response {
    let list: Vec<serde_json::Value> = d
        .ready()
        .map(|s| s.live())
        .unwrap_or_default()
        .into_iter()
        .map(|k| serde_json::json!({"engagement": k.engagement, "task": k.task}))
        .collect();
    Json(list).into_response()
}

async fn network(State(d): State<Arc<Daemon>>) -> Response {
    let sb = match d.ready() {
        Ok(s) => s,
        Err(why) => return bad(StatusCode::SERVICE_UNAVAILABLE, &why),
    };
    match sb.check_network().await {
        Ok(proxy) => Json(serde_json::json!({"proxy": proxy})).into_response(),
        Err(why) => bad(StatusCode::CONFLICT, &why),
    }
}

pub fn router(d: Arc<Daemon>) -> Router {
    Router::new()
        .route("/check", get(check))
        .route("/exec", post(exec))
        .route("/teardown", post(teardown))
        .route("/live", get(live))
        .route("/network", post(network))
        .layer(middleware::from_fn_with_state(d.clone(), guard))
        .with_state(d)
}

/// `spall sandboxd`: check the sandbox can run (again every 30 s until it
/// can), clear leftovers, and serve spall.
pub async fn run(args: Args) -> anyhow::Result<()> {
    let token = token(&args.token_file)
        .map_err(|e| anyhow::anyhow!("the token file {}: {e}", args.token_file.display()))?;
    let d = Arc::new(Daemon {
        sandbox: RwLock::new(Err("starting".into())),
        token,
    });
    let cfg = args.network.clone().map(|network| NetCfg {
        network,
        proxy: args.proxy.clone(),
        fence: args.fence.clone(),
    });
    let checker = {
        let d = d.clone();
        let image = args.image.clone();
        async move {
            loop {
                match Runsc::preflight(&image).await {
                    Ok(s) => {
                        tracing::info!("sandboxd: {image} under gVisor; serving spall");
                        let s: Arc<dyn Sandbox> = Arc::new(s.with_network(cfg));
                        *d.sandbox.write().unwrap_or_else(|p| p.into_inner()) = Ok(s);
                        return;
                    }
                    Err(why) => {
                        tracing::warn!("sandboxd: can't run sandboxes yet: {why}");
                        *d.sandbox.write().unwrap_or_else(|p| p.into_inner()) = Err(why);
                    }
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    };
    tokio::spawn(checker);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!("sandboxd listening on {}", listener.local_addr()?);
    axum::serve(listener, router(d)).await?;
    Ok(())
}

/// spall's side of the sensor: a [`Sandbox`] over HTTP to `sandboxd`.
pub struct Remote {
    base: String,
    token: String,
    http: Client<HttpConnector, Full<Bytes>>,
    /// The sandboxes spall started through it (spall is its only caller).
    live: Mutex<HashSet<Key>>,
    network: bool,
}

impl Remote {
    /// Connect to `sandboxd` at `base` with the token in `token_file`: it
    /// must answer `/check` with `ok`; otherwise why not.
    pub async fn connect(base: &str, token_file: &Path) -> Result<Remote, String> {
        let token = std::fs::read_to_string(token_file)
            .map_err(|e| format!("sandboxd's token ({}): {e}", token_file.display()))?
            .trim()
            .to_string();
        let mut r = Remote {
            base: base.trim_end_matches('/').to_string(),
            token,
            http: Client::builder(TokioExecutor::new()).build_http(),
            live: Mutex::new(HashSet::new()),
            network: false,
        };
        let (st, body) = r
            .call("GET", "/check", None, Duration::from_secs(10))
            .await?;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if !st.is_success() || v["ok"] != true {
            return Err(v["reason"]
                .as_str()
                .or(v["error"].as_str())
                .map(|w| format!("sandboxd: {w}"))
                .unwrap_or_else(|| format!("sandboxd answered {st}")));
        }
        r.network = v["network"] == true;
        Ok(r)
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<(StatusCode, Bytes), String> {
        let req = hyper::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            )))
            .map_err(|e| format!("building a request to sandboxd: {e}"))?;
        let res = tokio::time::timeout(timeout, self.http.request(req))
            .await
            .map_err(|_| "sandboxd didn't answer in time".to_string())?
            .map_err(|e| format!("sandboxd unreachable: {e}"))?;
        let st = res.status();
        let bytes = res
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("reading sandboxd: {e}"))?
            .to_bytes();
        Ok((st, bytes))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<Key>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The `error` sandboxd sent, or its status.
fn why(st: StatusCode, body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(String::from))
        .unwrap_or_else(|| format!("sandboxd answered {st}"))
}

impl Sandbox for Remote {
    fn exec<'a>(
        &'a self,
        key: &'a Key,
        cmd: &'a Cmd,
        net: Option<&'a Net>,
    ) -> BoxFuture<'a, Result<Run, String>> {
        Box::pin(async move {
            let mut body = serde_json::json!({
                "engagement": key.engagement,
                "task": key.task,
                "argv": cmd.argv,
                "timeout_seconds": cmd.timeout.as_secs(),
            });
            if let Some(n) = net {
                body["net"] = serde_json::json!({"proxy_url": n.proxy_url, "ca_pem": n.ca_pem});
            }
            self.lock().insert(key.clone());
            let (st, b) = self
                .call(
                    "POST",
                    "/exec",
                    Some(body),
                    cmd.timeout + Duration::from_secs(30),
                )
                .await?;
            if !st.is_success() {
                return Err(why(st, &b));
            }
            serde_json::from_slice::<Run>(&b).map_err(|e| format!("sandboxd's answer: {e}"))
        })
    }

    fn teardown<'a>(&'a self, key: &'a Key) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.lock().remove(key);
            let body = serde_json::json!({"engagement": key.engagement, "task": key.task});
            if let Err(e) = self
                .call("POST", "/teardown", Some(body), Duration::from_secs(30))
                .await
            {
                tracing::warn!("sandboxd teardown of {}: {e}", key.task);
            }
        })
    }

    fn live(&self) -> Vec<Key> {
        let mut v: Vec<Key> = self.lock().iter().cloned().collect();
        v.sort();
        v
    }

    fn has_network(&self) -> bool {
        self.network
    }

    fn check_network<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let (st, b) = self
                .call("POST", "/network", None, Duration::from_secs(60))
                .await?;
            if !st.is_success() {
                return Err(why(st, &b));
            }
            let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
            Ok(v["proxy"].as_str().map(String::from))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::Fake;

    async fn start(sandbox: Result<Arc<dyn Sandbox>, String>) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let tok = token(&dir.path().join("token")).unwrap();
        let d = Arc::new(Daemon {
            sandbox: RwLock::new(sandbox),
            token: tok,
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, router(d)).await.unwrap() });
        (format!("http://{addr}"), dir)
    }

    #[test]
    fn the_token_is_made_once_readable_by_the_group() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t");
        let a = token(&p).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(token(&p).unwrap(), a, "kept, not remade");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[tokio::test]
    async fn spall_drives_sandboxes_through_sandboxd_and_nobody_else_can() {
        let fake = Arc::new(Fake::default());
        let (base, dir) = start(Ok(fake.clone() as Arc<dyn Sandbox>)).await;
        let r = Remote::connect(&base, &dir.path().join("token"))
            .await
            .unwrap();
        assert!(!r.has_network());
        let key = Key {
            engagement: "acme-1a2b".into(),
            task: "T-3".into(),
        };
        let cmd = Cmd::new(vec!["echo".into(), "hi".into()], Some(5)).unwrap();
        let net = Net {
            proxy_url: "http://c-1:pw@taphttp:8090".into(),
            ca_pem: "PEM".into(),
        };
        let run = r.exec(&key, &cmd, Some(&net)).await.unwrap();
        assert_eq!((run.exit, run.stdout.as_str()), (Some(0), "hi\n"));
        assert_eq!(
            fake.nets.lock().unwrap().last().cloned(),
            Some(Some("http://c-1:pw@taphttp:8090".into()))
        );
        assert_eq!(r.live(), vec![key.clone()]);
        assert_eq!(fake.live(), vec![key.clone()]);
        r.teardown(&key).await;
        assert!(fake.live().is_empty() && r.live().is_empty());
        assert_eq!(r.check_network().await, Ok(None));

        // Bad ids, bad argv: refused before anything runs.
        let bad = Key {
            engagement: "../etc".into(),
            task: "T-3".into(),
        };
        assert!(r
            .exec(&bad, &cmd, None)
            .await
            .unwrap_err()
            .contains("not an engagement"));

        // Without the token, or from a browser: nothing.
        let http = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        for (auth, origin) in [
            ("Bearer nope", None),
            (&*format!("Bearer {}", r.token), Some("http://evil.test")),
        ] {
            let mut req =
                hyper::Request::post(format!("{base}/exec")).header(header::AUTHORIZATION, auth);
            if let Some(o) = origin {
                req = req.header(header::ORIGIN, o);
            }
            let res = http
                .request(
                    req.header(header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(
                            r#"{"engagement":"acme-1a2b","task":"T-3","argv":["echo"]}"#,
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                matches!(
                    res.status(),
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ),
                "{}",
                res.status()
            );
        }
        assert_eq!(
            fake.ran.lock().unwrap().len(),
            1,
            "only spall's one command ran"
        );
    }

    #[tokio::test]
    async fn a_sandboxd_that_cant_run_sandboxes_says_why() {
        let (base, dir) = start(Err("gVisor's runsc isn't a Docker runtime here".into())).await;
        let e = Remote::connect(&base, &dir.path().join("token"))
            .await
            .err()
            .unwrap();
        assert!(e.contains("runsc"), "{e}");
        let e = Remote::connect("http://127.0.0.1:9", &dir.path().join("token"))
            .await
            .err()
            .unwrap();
        assert!(e.contains("unreachable"), "{e}");
    }
}
