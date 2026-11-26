//! `spall mcp`: the stdio transport for the MCP server (#58). An MCP client
//! (Claude Code, Claude Desktop) starts it and speaks JSON-RPC on stdin and
//! stdout; each message is forwarded to the running spall's `POST /mcp`
//! with its token, so the server process stays the only one touching the
//! stores. In Docker: `docker compose exec -T spall spall mcp`.
//!
//! Nothing but JSON-RPC goes to stdout; notes and errors go to stderr.

use anyhow::Context;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const USAGE: &str = "usage: spall mcp [--url http://127.0.0.1:3000] [--token-file /data/mcp-token] [--task T-n | --goal TEXT [--role mapper|scoper|tester|reporter]]

Bridges an MCP client on stdin/stdout to the running spall's /mcp.
Every session works under a task. With no options it follows the tester's
queue: the oldest approved task for an outside client, then the next as each
ends, with no reconnecting. --task pins one; --goal (and --role) makes a
draft that a human approves before anything sends.
Environment: SPALL_URL, SPALL_MCP_TOKEN_FILE.";

/// Tool calls can wait on a slow target (a replay gets 120 s upstream).
const TIMEOUT: Duration = Duration::from_secs(150);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub url: String,
    pub token_file: PathBuf,
    pub task: Option<String>,
    pub goal: Option<String>,
    pub role: Option<String>,
}

/// Parse the arguments after `mcp`; `Err` is a message for stderr.
pub fn parse(args: &[String], env: impl Fn(&str) -> Option<String>) -> Result<Args, String> {
    let mut a = Args {
        url: env("SPALL_URL").unwrap_or_else(|| "http://127.0.0.1:3000".into()),
        token_file: env("SPALL_MCP_TOKEN_FILE")
            .unwrap_or_else(|| "/data/mcp-token".into())
            .into(),
        task: None,
        goal: None,
        role: None,
    };
    let mut it = args.iter();
    while let Some(k) = it.next() {
        let mut v = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{k} needs a value\n{USAGE}"))
        };
        match k.as_str() {
            "--url" => a.url = v()?,
            "--token-file" => a.token_file = v()?.into(),
            "--task" => a.task = Some(v()?),
            "--goal" => a.goal = Some(v()?),
            "--role" => a.role = Some(v()?),
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let url = a.url.trim_end_matches('/').to_string();
    if !url.starts_with("http://") {
        return Err(format!(
            "--url: spall's own address, plain http (got {url})"
        ));
    }
    a.url = url;
    if a.task.is_some() && (a.goal.is_some() || a.role.is_some()) {
        return Err(
            "--task names an existing task; --goal and --role make a new one: not both".into(),
        );
    }
    Ok(a)
}

impl Args {
    fn endpoint(&self) -> String {
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(t) = &self.task {
            q.push(("task", t));
        }
        if let Some(g) = &self.goal {
            q.push(("goal", g));
        }
        if let Some(r) = &self.role {
            q.push(("role", r));
        }
        let qs = serde_urlencoded::to_string(&q).unwrap_or_default();
        if qs.is_empty() {
            format!("{}/mcp", self.url)
        } else {
            format!("{}/mcp?{qs}", self.url)
        }
    }
}

fn rpc_error(id: &Value, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
        .to_string()
}

/// Run the bridge until stdin closes.
pub async fn run(a: Args) -> anyhow::Result<()> {
    let token = std::fs::read_to_string(&a.token_file)
        .with_context(|| {
            format!(
                "reading the MCP token {} (spall creates it on start)",
                a.token_file.display()
            )
        })?
        .trim()
        .to_string();
    let http: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let endpoint = a.endpoint();
    let mut session: Option<String> = None;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned());
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(&endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("authorization", format!("Bearer {token}"));
        if let Some(s) = &session {
            req = req.header("mcp-session-id", s);
        }
        let req = req
            .body(Full::new(Bytes::from(line)))
            .context("building a request")?;
        let answer = match tokio::time::timeout(TIMEOUT, http.request(req)).await {
            Ok(Ok(res)) => {
                let status = res.status();
                if let Some(s) = res
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                {
                    session = Some(s.to_string());
                }
                let body = res
                    .into_body()
                    .collect()
                    .await
                    .map(|b| b.to_bytes())
                    .unwrap_or_default();
                if status == StatusCode::ACCEPTED || body.is_empty() {
                    None
                } else if serde_json::from_slice::<Value>(&body).is_ok() {
                    Some(String::from_utf8_lossy(&body).trim().to_string())
                } else {
                    let msg = format!(
                        "spall answered {status}: {}",
                        String::from_utf8_lossy(&body).trim()
                    );
                    eprintln!("spall mcp: {msg}");
                    id.as_ref().map(|i| rpc_error(i, &msg))
                }
            }
            Ok(Err(e)) => {
                let msg = format!("spall unreachable at {}: {e}", a.url);
                eprintln!("spall mcp: {msg}");
                id.as_ref().map(|i| rpc_error(i, &msg))
            }
            Err(_) => id
                .as_ref()
                .map(|i| rpc_error(i, "spall took too long to answer")),
        };
        if let Some(text) = answer {
            out.write_all(text.as_bytes()).await?;
            out.write_all(b"\n").await?;
            out.flush().await?;
        }
    }
    if let Some(s) = session {
        let req = Request::builder()
            .method(Method::DELETE)
            .uri(format!("{}/mcp", a.url))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("mcp-session-id", s)
            .body(Full::new(Bytes::new()))
            .context("building a request")?;
        let _ = tokio::time::timeout(Duration::from_secs(5), http.request(req)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            |_| None,
        )
    }

    #[test]
    fn arguments() {
        let a = p(&[]).unwrap();
        assert_eq!(a.endpoint(), "http://127.0.0.1:3000/mcp");
        assert_eq!(a.token_file, PathBuf::from("/data/mcp-token"));
        let a = p(&["--url", "http://127.0.0.1:13000/", "--task", "T-3"]).unwrap();
        assert_eq!(a.endpoint(), "http://127.0.0.1:13000/mcp?task=T-3");
        let a = p(&["--goal", "map the API & more", "--role", "tester"]).unwrap();
        assert_eq!(
            a.endpoint(),
            "http://127.0.0.1:3000/mcp?goal=map+the+API+%26+more&role=tester"
        );
        assert!(p(&["--task", "T-1", "--goal", "x"])
            .unwrap_err()
            .contains("not both"));
        assert!(p(&["--url", "https://x"])
            .unwrap_err()
            .contains("plain http"));
        assert!(p(&["--nope"]).unwrap_err().contains("unknown argument"));
        assert!(p(&["--task"]).unwrap_err().contains("needs a value"));
        let env = parse(&[], |k| {
            (k == "SPALL_URL").then(|| "http://127.0.0.1:9".to_string())
        })
        .unwrap();
        assert_eq!(env.url, "http://127.0.0.1:9");
    }
}
