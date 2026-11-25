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
        .with_writer(std::io::stderr)
        .init();

    // `spall mcp`: the stdio bridge to a running spall's MCP server (#58).
    // Its logs go to stderr; stdout carries only JSON-RPC.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("mcp") {
        let args = match spall::mcp_bridge::parse(&argv[2..], |k| std::env::var(k).ok()) {
            Ok(a) => a,
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        };
        return spall::mcp_bridge::run(args).await;
    }

    // `spall sandboxd`: the sandbox sensor (#13), the only process that
    // holds Docker; spall talks to it with --sandbox-url.
    if argv.get(1).map(String::as_str) == Some("sandboxd") {
        let args = spall::sandboxd::Args::parse_from(&argv[1..]);
        return spall::sandboxd::run(args).await;
    }

    // `spall sandbox-shell [--image I] [-- cmd args...]`: a shell for the
    // tester in a sandbox with the agent's exact flags (#13). Host-side only.
    if argv.get(1).map(String::as_str) == Some("sandbox-shell") {
        let mut rest = &argv[2..];
        let mut image = std::env::var("SPALL_SANDBOX_IMAGE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| spall::sandbox::IMAGE_DEFAULT.to_string());
        if rest.first().map(String::as_str) == Some("--image") {
            let Some(i) = rest.get(1) else {
                eprintln!("usage: spall sandbox-shell [--image IMAGE] [-- CMD ARGS...]");
                std::process::exit(2);
            };
            image = i.clone();
            rest = &rest[2..];
        }
        if rest.first().map(String::as_str) == Some("--") {
            rest = &rest[1..];
        }
        let code = spall::sandbox::shell(&image, rest).await?;
        std::process::exit(code);
    }

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
