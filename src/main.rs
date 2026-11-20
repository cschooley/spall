use anyhow::Context;
use clap::Parser;
use spall::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let config = Config::parse();
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    let addr = listener.local_addr()?;

    if !addr.ip().is_loopback() {
        tracing::warn!(
            "listening on non-loopback {addr}: the UI shows raw traffic including credentials \
             and has no auth. Only do this inside a container whose port is published to 127.0.0.1."
        );
    }
    if config.allowed_hosts.iter().any(|h| h == "*") {
        tracing::warn!("Host header check disabled (allowed hosts = *)");
    }
    tracing::info!("spall console on http://{addr}");

    tokio::select! {
        r = spall::serve(config, listener) => r,
        () = shutdown_signal() => {
            tracing::info!("shutting down");
            Ok(())
        }
    }
}

/// Ctrl-C, or SIGTERM (`docker stop`; PID 1 ignores it without a handler).
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("cannot listen for ctrl-c: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!("cannot listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
}
