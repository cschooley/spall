//! spall M0: tail taphttp's traffic log and stream it to a browser console.

pub mod config;
pub mod entry;
pub mod hub;
pub mod tailer;
pub mod web;

use anyhow::Context;
use config::Config;
use hub::{Hub, Published};
use std::sync::Arc;
use std::time::Duration;
use tailer::LineTailer;
use tokio::net::TcpListener;

/// Parse one log line and publish it. Malformed lines are logged and
/// skipped. The line's content is never logged: it may hold credentials.
pub fn ingest_line(line: &str, hub: &Hub) -> bool {
    let entry = match entry::parse_line(line) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("skipping malformed log line ({} bytes): {e}", line.len());
            return false;
        }
    };
    match Published::from_entry(&entry) {
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

/// Poll the log forever, publishing each entry. Only returns if the
/// blocking poll task itself dies, which should never happen.
pub async fn tail_into_hub(
    mut tailer: LineTailer,
    interval: Duration,
    hub: Arc<Hub>,
) -> anyhow::Result<()> {
    tracing::info!("tailing {}", tailer.path().display());
    let mut last_error: Option<String> = None;
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
                    ingest_line(line, &hub);
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

/// Run the tailer and web server on an already-bound listener.
pub async fn serve(config: Config, listener: TcpListener) -> anyhow::Result<()> {
    let hub = Arc::new(Hub::new(config.history));
    let state = web::AppState {
        hub: hub.clone(),
        allowed_hosts: Arc::new(config.allowed_hosts.clone()),
    };
    let tail = tail_into_hub(
        LineTailer::new(&config.log_file),
        Duration::from_millis(config.poll_ms),
        hub,
    );
    tokio::select! {
        r = axum::serve(listener, web::router(state)) => r.context("web server failed"),
        r = tail => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_publishes_valid_and_skips_malformed() {
        let hub = Hub::new(10);
        assert!(!ingest_line("garbage", &hub));
        assert!(!ingest_line("{\"id\":\"half\"", &hub));
        assert!(ingest_line(
            r#"{"id":"ok","ts":"2026-11-20T00:00:00Z","host":"h","method":"GET","url":"/"}"#,
            &hub
        ));
        let (history, _) = hub.subscribe();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id.as_deref(), Some("ok"));
    }
}
