//! spall: stream taphttp traffic to a browser console. Traffic comes from
//! taphttp's lever API when configured (`--taphttp-api`), otherwise from
//! tailing its `--log-file`.

pub mod addresses;
pub mod agent;
pub mod audit;
pub mod candidates;
pub mod commands;
pub mod config;
pub mod confirm;
pub mod csrf;
pub mod engagement;
pub mod gate;
pub mod host;
pub mod hosts;
pub mod hub;
pub mod intake;
pub mod lever;
pub mod mcp;
pub mod mcp_bridge;
pub mod model;
pub mod openapi;
pub mod plans;
pub mod quad;
pub mod replay;
pub mod roe;
pub mod roles;
pub mod rules;
pub mod scope_list;
pub mod stack;
pub mod tailer;
pub mod tasks;
pub mod untrusted;
pub mod verbs;
pub mod web;

use anyhow::Context;
use config::Config;
use hub::{Hub, Published};
use std::sync::Arc;
use std::time::Duration;
use tailer::LineTailer;
use taphttp_types::{SchemaProbe, TrafficEntry, SCHEMA_VERSION};
use tokio::net::TcpListener;

/// What [`ingest_line`] did with a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingested {
    Published,
    /// Published, but the line declares a schema newer than the
    /// `taphttp-types` spall was built with, so unknown fields were dropped.
    NewerSchema(u32),
    /// Malformed; logged and skipped.
    Skipped,
}

/// Parse one taphttp entry (a log line, an API item or a feed frame).
/// Also returns the declared schema version if it is newer than spall's.
pub fn parse_entry(text: &str) -> Result<(TrafficEntry, Option<u32>), serde_json::Error> {
    let entry: TrafficEntry = serde_json::from_str(text)?;
    let newer = serde_json::from_str::<SchemaProbe>(text)
        .ok()
        .and_then(|p| p.schema_version)
        .filter(|v| *v > SCHEMA_VERSION);
    Ok((entry, newer))
}

/// Publish a parsed entry. Returns false (and logs) if it cannot be
/// serialized, which should not happen for a value that just parsed.
pub fn publish_entry(entry: &TrafficEntry, hub: &Hub) -> bool {
    hub.hosts.observe(entry);
    match Published::from_entry(entry) {
        Ok(p) => {
            hub.publish(p);
            true
        }
        Err(e) => {
            tracing::warn!("skipping entry that failed to serialize: {e}");
            false
        }
    }
}

/// Warns once per newer schema version seen.
#[derive(Default)]
pub struct SchemaWarner(u32);

impl SchemaWarner {
    pub fn note(&mut self, newer: Option<u32>) {
        if let Some(v) = newer.filter(|v| *v > self.0) {
            tracing::warn!(
                "taphttp sends schema v{v}; this spall understands v{SCHEMA_VERSION}. \
                 New fields are dropped until spall's taphttp-types is bumped."
            );
            self.0 = v;
        }
    }
}

/// Parse one taphttp log line and publish it. Malformed lines are logged
/// and skipped. The line's content is never logged: it may hold credentials.
pub fn ingest_line(line: &str, hub: &Hub) -> Ingested {
    let (entry, newer) = match parse_entry(line) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("skipping malformed log line ({} bytes): {e}", line.len());
            return Ingested::Skipped;
        }
    };
    if !publish_entry(&entry, hub) {
        return Ingested::Skipped;
    }
    match newer {
        Some(v) => Ingested::NewerSchema(v),
        None => Ingested::Published,
    }
}

/// Poll the log forever, publishing each entry. Only returns if the
/// blocking poll task itself dies, which should never happen.
pub async fn tail_into_hub(
    mut tailer: LineTailer,
    interval: Duration,
    hub: Arc<Hub>,
) -> anyhow::Result<()> {
    tracing::info!("tailing {}", tailer.path().display());
    let mut last_error: Option<String> = None;
    let mut schema = SchemaWarner::default();
    loop {
        let (t, result) = tokio::task::spawn_blocking(move || {
            let r = tailer.poll();
            (tailer, r)
        })
        .await
        .context("log tailer task failed")?;
        tailer = t;
        match result {
            Ok(lines) => {
                last_error = None;
                let more = !lines.is_empty();
                for line in &lines {
                    if let Ingested::NewerSchema(v) = ingest_line(line, &hub) {
                        schema.note(Some(v));
                    }
                }
                // Keep reading without sleeping while catching up a backlog.
                if more {
                    tokio::task::yield_now().await;
                    continue;
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if last_error.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("reading {}: {msg}", tailer.path().display());
                    last_error = Some(msg);
                }
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Run the traffic source (lever API follower or log tailer) and the web
/// server on an already-bound listener.
pub async fn serve(config: Config, listener: TcpListener) -> anyhow::Result<()> {
    build(config)?.run(listener).await
}

/// Everything `serve` runs, opened from the config. Tests hold `replayer`
/// to drive the agent side (`Replayer::agent_replay`), which has no HTTP
/// route of its own: no browser request can claim to be an agent.
pub struct App {
    config: Config,
    hub: Arc<Hub>,
    lever: Option<Arc<lever::Lever>>,
    pub replayer: Option<Arc<replay::Replayer>>,
    pub mcp: Option<Arc<mcp::Server>>,
    /// spall's own agent host (#63), with a model configured.
    pub host: Option<Arc<host::Host>>,
    /// The intake's orchestrator (#65), on the same model.
    pub intake: Option<Arc<intake::Intake>>,
}

/// Open the stores and the audit log for `config`; errors stop spall.
pub fn build(config: Config) -> anyhow::Result<App> {
    let hub = Arc::new(Hub::new(config.history));
    let lever = config.taphttp_api.as_ref().map(|base| {
        Arc::new(lever::Lever::new(
            base.clone(),
            config.taphttp_token_file.clone(),
        ))
    });
    let replayer = match &lever {
        Some(lever) => {
            let audit = audit::AuditLog::open(&config.audit_log).with_context(|| {
                format!(
                    "opening the audit log {} (set --audit-log / SPALL_AUDIT_LOG)",
                    config.audit_log.display()
                )
            })?;
            let (engagements, opened) = engagement::Store::open(
                &config.engagements,
                &config.scope,
                &roe::Rules {
                    safe_methods: config.safe_methods.methods().to_vec(),
                    ..roe::Rules::default()
                },
            )
            .with_context(|| {
                format!(
                    "loading engagements from {} (set --engagements / SPALL_ENGAGEMENTS)",
                    config.engagements.display()
                )
            })?;
            for skipped in &opened.skipped {
                tracing::warn!("engagement file skipped: {skipped}");
            }
            let active = engagements.active();
            if opened.bootstrapped {
                tracing::info!("created engagement {} from --scope", active.id);
            }
            if opened.config_ignored {
                tracing::warn!(
                    "--scope / SPALL_SCOPE differs from engagement {}'s saved scope; using the engagement's (change it with `scope` in the console)",
                    active.id
                );
            }
            let scope = engagements.scope().with_all(config.scope.allows_all());
            tracing::info!("engagement: {} ({})", active.name, active.id);
            if scope.allows_all() {
                tracing::warn!(
                    "scope is * (SPALL_SCOPE): replays may go to every host, third parties included"
                );
            } else if scope.is_empty() {
                tracing::warn!(
                    "the engagement has no scope: every replay will be denied (`scope add` in the console, or --scope for a new store)"
                );
            } else {
                tracing::info!("replay scope: {}", scope.entries().join(", "));
            }
            tracing::info!("audit log: {}", config.audit_log.display());
            let candidates_dir = config.engagements.join("candidates");
            let candidates = candidates::Store::open(&candidates_dir)
                .with_context(|| format!("loading candidates from {}", candidates_dir.display()))?;
            let tasks_dir = config.engagements.join("tasks");
            let tasks = tasks::Store::open(&tasks_dir)
                .with_context(|| format!("loading tasks from {}", tasks_dir.display()))?;
            let plans_dir = config.engagements.join("plans");
            let plans = plans::Store::open(&plans_dir)
                .with_context(|| format!("loading plans from {}", plans_dir.display()))?;
            let addresses_dir = config.engagements.join("addresses");
            let addresses = addresses::Store::open(&addresses_dir)
                .with_context(|| format!("loading addresses from {}", addresses_dir.display()))?;
            Some(Arc::new(replay::Replayer {
                lever: lever.clone(),
                engagements,
                all_hosts: std::sync::atomic::AtomicBool::new(config.scope.allows_all()),
                limiter: std::sync::Mutex::new(roe::Limiter::default()),
                task_limiters: std::sync::Mutex::new(std::collections::HashMap::new()),
                confirmations: confirm::Confirmations::new(Duration::from_secs(120), 256),
                audit,
                candidates,
                tasks,
                allow_unredacted_export: config.allow_unredacted_export,
                stack: stack::Stack::from_config(&config),
                spall_runs: std::sync::atomic::AtomicBool::new(false),
                quad: quad::Boost::default(),
                plans,
                addresses: Arc::new(addresses),
                resolver: std::sync::RwLock::new(Arc::new(addresses::System)),
            }))
        }
        None => None,
    };
    let mcp = match &replayer {
        Some(r) => {
            let token = mcp::token(&config.mcp_token_file).with_context(|| {
                format!(
                    "the MCP token {} (set --mcp-token-file / SPALL_MCP_TOKEN_FILE)",
                    config.mcp_token_file.display()
                )
            })?;
            let tools = agent::Tools {
                replayer: r.clone(),
                hub: hub.clone(),
            };
            Some(mcp::arc(tools, token))
        }
        None => None,
    };
    let key = model::Key::load(
        config.model_key_file.as_deref(),
        std::env::var("SPALL_MODEL_KEY").ok(),
    )?;
    let settings = model::Settings::resolve(
        config.model_provider,
        config.model.as_deref(),
        config.model_url.as_deref(),
        config.model_max_tokens,
        key.as_ref(),
    )?;
    let mut app = App {
        config,
        hub,
        lever,
        replayer,
        mcp,
        host: None,
        intake: None,
    };
    if let Some(settings) = settings {
        if app.replayer.is_none() {
            anyhow::bail!("spall's agent host needs taphttp's lever API (--taphttp-api)");
        }
        tracing::info!(
            "agent host: {} {} at {}",
            settings.provider.as_str(),
            settings.model,
            settings.url
        );
        if !settings.url.starts_with("http://") {
            tracing::warn!(
                "spall-run tasks send what their agent reads (captured traffic, credentials masked) to {}",
                settings.url
            );
        }
        app = app.with_model(Arc::new(model::Client::new(settings, key)?));
    }
    Ok(app)
}

impl App {
    /// Look host names up with `resolver` (#65); tests pass a fake one.
    pub fn with_resolver(self, resolver: Arc<dyn addresses::Resolve>) -> App {
        if let Some(r) = &self.replayer {
            *r.resolver.write().unwrap_or_else(|p| p.into_inner()) = resolver;
        }
        self
    }

    /// Run spall's agent host on `model` (#63); tests pass a scripted one.
    /// Without the lever API there's nothing for it to drive.
    pub fn with_model(mut self, model: Arc<dyn model::Model>) -> App {
        if let Some(r) = &self.replayer {
            let tools = agent::Tools {
                replayer: r.clone(),
                hub: self.hub.clone(),
            };
            let dir = self.config.engagements.join("tasks").join("transcripts");
            self.host = Some(host::Host::new(tools, model.clone(), &dir));
            self.intake = Some(intake::Intake::new(r.clone(), model));
            r.spall_runs
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self
    }

    pub async fn run(self, listener: TcpListener) -> anyhow::Result<()> {
        let App {
            config,
            hub,
            lever,
            replayer,
            mcp,
            host,
            intake,
        } = self;
        let state = web::AppState {
            hub: hub.clone(),
            allowed_hosts: Arc::new(config.allowed_hosts.clone()),
            lever: lever.clone(),
            ui: Arc::new(web::EmbeddedUi),
            proxy_address: Arc::from(config.proxy_address.as_str()),
            replayer,
            mcp,
            host: host.clone(),
            intake,
        };
        let agents = async {
            match host {
                Some(h) => h.run().await,
                None => std::future::pending().await,
            }
        };
        let source = async {
            match lever {
                Some(lever) => {
                    tracing::info!("traffic from taphttp lever API at {}", lever.base());
                    lever::Follower::new(lever, hub, config.history).run().await
                }
                None => {
                    tail_into_hub(
                        LineTailer::new(&config.log_file),
                        Duration::from_millis(config.poll_ms),
                        hub,
                    )
                    .await
                }
            }
        };
        tokio::select! {
            r = axum::serve(listener, web::router(state)) => r.context("web server failed"),
            r = source => r,
            r = agents => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v3 line as taphttp v0.2.0 writes it: repeated Set-Cookie, a
    /// non-UTF-8 response body stored as base64.
    const V3_LINE: &str = r#"{"schema_version":3,"id":"v3","ts":"2026-12-25T09:40:03.123Z","host":"example.com","method":"GET","url":"https://example.com/","req_headers":[["host","example.com"]],"req_body":null,"req_body_encoding":"utf8","status":200,"res_headers":[["set-cookie","a=1; Secure"],["content-type","image/png"],["set-cookie","b=2; HttpOnly"]],"res_body":"iVBORw0KGgo=","res_body_encoding":"base64","duration_ms":7,"replay_of":null}"#;

    /// A v2 line (taphttp before v0.2.0): headers as a map, no encodings.
    const V2_LINE: &str = r#"{"id":"v2","ts":"2026-11-20T17:03:04Z","host":"example.com","method":"GET","url":"http://example.com/","req_headers":{"host":"example.com"},"req_body":null,"status":200,"res_headers":{"content-type":"text/html"},"res_body":"<h1>hi</h1>","duration_ms":42,"replay_of":null}"#;

    fn published(hub: &Hub) -> Vec<serde_json::Value> {
        hub.subscribe()
            .0
            .iter()
            .map(|p| serde_json::from_str(&p.data).unwrap())
            .collect()
    }

    #[test]
    fn malformed_lines_are_skipped_not_published() {
        let hub = Hub::new(10);
        for bad in [
            "garbage",
            "{\"id\":\"half\"",
            "[]",
            r#"{"id":"a"}"#,
            r#"{"id":"a","ts":"yesterday","host":"h","method":"GET","url":"/"}"#,
            r#"{"id":"a","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/","status":70000}"#,
        ] {
            assert_eq!(ingest_line(bad, &hub), Ingested::Skipped, "{bad:?}");
        }
        assert!(published(&hub).is_empty());
    }

    #[test]
    fn v3_line_is_published_as_current_schema_with_headers_in_wire_order() {
        let hub = Hub::new(10);
        assert_eq!(ingest_line(V3_LINE, &hub), Ingested::Published);
        let out = published(&hub);
        assert_eq!(out[0]["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            out[0]["res_headers"],
            serde_json::json!([
                ["set-cookie", "a=1; Secure"],
                ["content-type", "image/png"],
                ["set-cookie", "b=2; HttpOnly"]
            ])
        );
    }

    #[test]
    fn binary_body_passes_through_as_base64() {
        let hub = Hub::new(10);
        ingest_line(V3_LINE, &hub);
        let out = published(&hub);
        assert_eq!(out[0]["res_body"], "iVBORw0KGgo=");
        assert_eq!(out[0]["res_body_encoding"], "base64");
    }

    #[test]
    fn v2_line_is_upgraded_to_current_schema_on_the_way_out() {
        let hub = Hub::new(10);
        assert_eq!(ingest_line(V2_LINE, &hub), Ingested::Published);
        let out = published(&hub);
        assert_eq!(out[0]["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            out[0]["res_headers"],
            serde_json::json!([["content-type", "text/html"]])
        );
        assert_eq!(out[0]["res_body_encoding"], "utf8");
    }

    #[test]
    fn newer_schema_is_published_but_reported() {
        let hub = Hub::new(10);
        let next = SCHEMA_VERSION + 1;
        let newer = V3_LINE.replace(
            "\"schema_version\":3",
            &format!("\"schema_version\":{next}"),
        );
        assert_eq!(ingest_line(&newer, &hub), Ingested::NewerSchema(next));
        assert_eq!(published(&hub).len(), 1);
    }

    #[test]
    fn source_passes_through_and_defaults_to_null() {
        let hub = Hub::new(10);
        let v4 = V3_LINE
            .replace("\"schema_version\":3", "\"schema_version\":4")
            .replace(
                "\"replay_of\":null",
                "\"replay_of\":null,\"source\":\"alice\"",
            );
        ingest_line(&v4, &hub);
        ingest_line(V3_LINE, &hub);
        let out = published(&hub);
        assert_eq!(out[0]["source"], "alice");
        assert_eq!(out[1]["source"], serde_json::Value::Null);
    }

    #[test]
    fn unsafe_ids_still_publish_without_sse_id() {
        let hub = Hub::new(10);
        let line = V3_LINE.replace("\"id\":\"v3\"", "\"id\":\"a\\nb\"");
        assert_eq!(ingest_line(&line, &hub), Ingested::Published);
        assert_eq!(hub.subscribe().0[0].id, None);
    }
}
