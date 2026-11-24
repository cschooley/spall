//! The model behind spall's own agent host (#63): the tester's key, kept
//! server-side, and a client for one provider.
//!
//! Two providers (decided 2026-11-24): Anthropic's Messages API, and any
//! OpenAI-compatible chat completions endpoint (Ollama, llama.cpp, vLLM),
//! so a tester can keep captured traffic on their own machine.
//!
//! The key is read from a file (0600 or stricter) or `SPALL_MODEL_KEY`,
//! held in [`Key`] (whose `Debug` prints no secret), sent only as a request
//! header to the configured model URL, never to the browser, never logged,
//! and scrubbed from any error text.
//!
//! A conversation is kept in the provider's own wire format and only ever
//! appended to: on models that think, the assistant's turns (thinking
//! blocks included) must go back exactly as they came. For the same
//! reason a conversation's system prompt and tools never change.

use crate::agent::Output;
use anyhow::{bail, Context};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};
use std::future::Future;
use std::net::IpAddr;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

/// The Messages API version spall speaks.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
pub const ANTHROPIC_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
/// Output tokens one model turn may use (thinking included).
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;
/// The most a model's answer may be.
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
/// One model call, end to end. A thinking model can take minutes.
const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Attempts for an overloaded or rate-limited API, and the longest wait.
const ATTEMPTS: u32 = 4;
const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Anthropic's Messages API.
    Anthropic,
    /// An OpenAI-compatible chat completions endpoint (often a local model).
    Openai,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
        }
    }
}

/// The model key. Its `Debug` and `Display` never show it.
#[derive(Clone)]
pub struct Key(String);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

impl Key {
    /// `text` with the key masked, for anything that may reach a log, the
    /// UI or a task.
    pub fn scrub(&self, text: &str) -> String {
        if self.0.len() < 8 {
            return text.to_string();
        }
        text.replace(&self.0, "<model key>")
    }

    fn new(raw: &str) -> anyhow::Result<Key> {
        let k = raw.trim();
        if k.is_empty() {
            bail!("the model key is empty");
        }
        if k.chars().any(|c| c.is_control() || c.is_whitespace()) || !k.is_ascii() {
            bail!("the model key has spaces, control or non-ASCII characters in it");
        }
        Ok(Key(k.to_string()))
    }

    /// The key from `file` (which must be private to its owner) or, without
    /// one, from `env` (`SPALL_MODEL_KEY`). `None` when neither is set.
    pub fn load(file: Option<&Path>, env: Option<String>) -> anyhow::Result<Option<Key>> {
        if let Some(path) = file {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path)
                    .with_context(|| format!("reading the model key file {}", path.display()))?
                    .permissions()
                    .mode();
                if mode & 0o077 != 0 {
                    bail!(
                        "the model key file {} is readable by others (mode {:o}): chmod 600 it",
                        path.display(),
                        mode & 0o777
                    );
                }
            }
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading the model key file {}", path.display()))?;
            return Key::new(&raw).map(Some);
        }
        match env {
            Some(raw) if !raw.trim().is_empty() => Key::new(&raw).map(Some),
            _ => Ok(None),
        }
    }
}

/// Where a model API may be: https anywhere, plain http only on this
/// machine or a private network (a local model), never with credentials,
/// a query or a fragment. Normalized without a trailing slash.
pub fn parse_url(s: &str) -> Result<String, String> {
    let s = s.trim().trim_end_matches('/');
    let uri: hyper::Uri = s.parse().map_err(|_| format!("{s:?} is not a URL"))?;
    let host = uri
        .host()
        .ok_or_else(|| format!("{s:?} has no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if uri.authority().is_some_and(|a| a.as_str().contains('@')) {
        return Err("no credentials in the model URL: use --model-key-file".into());
    }
    if uri.query().is_some() || s.contains('#') {
        return Err("no query or fragment in the model URL".into());
    }
    match uri.scheme_str() {
        Some("https") => Ok(s.to_string()),
        Some("http") if local_host(&host) => Ok(s.to_string()),
        Some("http") => Err(format!(
            "plain http only to a model on this machine or a private network; {host} needs https"
        )),
        _ => Err("the model URL must start with https:// (or http:// for a local model)".into()),
    }
}

/// This machine, a private network, or a name that only resolves inside
/// one (a compose service, `.local`, `.internal`).
fn local_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
    }
    host == "localhost"
        || !host.contains('.')
        || [".localhost", ".local", ".internal", ".lan", ".home.arpa"]
            .iter()
            .any(|s| host.ends_with(s))
}

/// The host's model settings, resolved from the config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Settings {
    pub provider: Provider,
    pub model: String,
    pub url: String,
    pub max_tokens: u32,
}

impl Settings {
    /// `None` without a provider (no host). Anthropic needs a key; a local
    /// endpoint may not.
    pub fn resolve(
        provider: Option<Provider>,
        model: Option<&str>,
        url: Option<&str>,
        max_tokens: u32,
        key: Option<&Key>,
    ) -> anyhow::Result<Option<Settings>> {
        let Some(provider) = provider else {
            if model.is_some() || url.is_some() || key.is_some() {
                bail!("a model, model URL or key is set but no --model-provider: say which (anthropic or openai)");
            }
            return Ok(None);
        };
        let (model, url) = match provider {
            Provider::Anthropic => {
                if key.is_none() {
                    bail!("--model-provider anthropic needs a key: --model-key-file (0600) or SPALL_MODEL_KEY");
                }
                (
                    model.unwrap_or(ANTHROPIC_MODEL),
                    url.unwrap_or(ANTHROPIC_URL),
                )
            }
            Provider::Openai => (
                model.context("--model-provider openai needs --model (the name the endpoint serves)")?,
                url.context(
                    "--model-provider openai needs --model-url, e.g. http://host.docker.internal:11434/v1",
                )?,
            ),
        };
        let model = model.trim();
        if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
            bail!("--model: a model name, at most 200 characters");
        }
        let url = parse_url(url).map_err(anyhow::Error::msg)?;
        Ok(Some(Settings {
            provider,
            model: model.to_string(),
            url,
            max_tokens,
        }))
    }
}

/// A tool as the model sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// The registry's agent commands, as tools.
pub fn tool_defs() -> Vec<ToolDef> {
    crate::commands::offered()
        .map(|c| ToolDef {
            name: c.name.to_string(),
            description: c.desc.to_string(),
            schema: c.args(),
        })
        .collect()
}

/// A conversation in the provider's wire format, oldest first. Only the
/// [`Model`] that started it appends to it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub messages: Vec<Value>,
}

/// A tool call the model made.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    /// `Err` when the model's arguments weren't JSON.
    pub input: Result<Value, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Finished: no more tool calls.
    EndTurn,
    /// Wants its tool calls run.
    ToolUse,
    /// Cut off at `max_tokens`: its tool calls may be truncated.
    MaxTokens,
    /// Declined to go on.
    Refusal,
    Other(String),
}

/// Tokens one call used. Input counts cache reads and writes too: it's
/// what the call was charged for, in tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One model turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The model's text, for the task log (thinking isn't included).
    pub text: String,
    pub calls: Vec<Call>,
    pub stop: Stop,
    pub usage: Usage,
}

/// A failed model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub message: String,
    /// Worth trying again later (overloaded, rate limited, network).
    pub retryable: bool,
    /// How long the API asked us to wait.
    pub retry_after: Option<Duration>,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the host needs from a model. [`Client`] talks to a real one; tests
/// script one.
pub trait Model: Send + Sync {
    /// Provider and model, for the status bar and the task log.
    fn describe(&self) -> String;
    /// A new conversation that opens with the user's `text`.
    fn start(&self, text: &str) -> Conversation;
    /// One turn: the model's answer is appended to `c` and returned.
    fn turn<'a>(
        &'a self,
        system: &'a str,
        tools: &'a [ToolDef],
        c: &'a mut Conversation,
    ) -> BoxFuture<'a, Result<Turn, ModelError>>;
    /// Answer the last turn's calls, in order, then add `note` (a human's
    /// steering, an approval's outcome) if any.
    fn answer(&self, c: &mut Conversation, results: &[(String, Output)], note: Option<&str>);
}

type Https = hyper_rustls::HttpsConnector<HttpConnector>;

/// A real model API.
pub struct Client {
    settings: Settings,
    key: Option<Key>,
    http: HyperClient<Https, Full<Bytes>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("settings", &self.settings)
            .field("key", &self.key)
            .finish()
    }
}

impl Client {
    pub fn new(settings: Settings, key: Option<Key>) -> anyhow::Result<Client> {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .context("setting up TLS for the model API")?
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Client {
            settings,
            key,
            http: HyperClient::builder(TokioExecutor::new()).build(https),
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn scrub(&self, s: &str) -> String {
        match &self.key {
            Some(k) => k.scrub(s),
            None => s.to_string(),
        }
    }

    fn fail(&self, message: String, retryable: bool) -> ModelError {
        ModelError {
            message: self.scrub(&message),
            retryable,
            retry_after: None,
        }
    }

    /// POST `body` to `path`, retrying what's worth retrying.
    async fn post(&self, path: &str, body: &Value) -> Result<Value, ModelError> {
        let mut last = None;
        for attempt in 0..ATTEMPTS {
            match self.post_once(path, body).await {
                Ok(v) => return Ok(v),
                Err(e) if e.retryable && attempt + 1 < ATTEMPTS => {
                    let wait = e
                        .retry_after
                        .unwrap_or(Duration::from_secs(2u64 << attempt))
                        .min(MAX_WAIT);
                    tracing::warn!("model API: {e}; trying again in {}s", wait.as_secs());
                    tokio::time::sleep(wait).await;
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| self.fail("the model API didn't answer".into(), true)))
    }

    async fn post_once(&self, path: &str, body: &Value) -> Result<Value, ModelError> {
        let url = format!("{}{path}", self.settings.url);
        let bytes = serde_json::to_vec(body)
            .map_err(|e| self.fail(format!("encoding the request: {e}"), false))?;
        let mut req = hyper::Request::post(&url).header("content-type", "application/json");
        match (self.settings.provider, &self.key) {
            (Provider::Anthropic, Some(k)) => {
                req = req
                    .header("x-api-key", &k.0)
                    .header("anthropic-version", ANTHROPIC_VERSION);
            }
            (Provider::Anthropic, None) => {
                req = req.header("anthropic-version", ANTHROPIC_VERSION);
            }
            (Provider::Openai, Some(k)) => {
                req = req.header("authorization", format!("Bearer {}", k.0));
            }
            (Provider::Openai, None) => {}
        }
        let req = req
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|e| self.fail(format!("building the request: {e}"), false))?;
        let res = match tokio::time::timeout(CALL_TIMEOUT, self.http.request(req)).await {
            Err(_) => return Err(self.fail("the model API took too long".into(), true)),
            Ok(Err(e)) => {
                return Err(self.fail(
                    format!("can't reach the model API at {}: {e}", self.settings.url),
                    true,
                ))
            }
            Ok(Ok(r)) => r,
        };
        let status = res.status();
        let retry_after = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(Duration::from_secs_f64);
        let body = Limited::new(res.into_body(), MAX_RESPONSE)
            .collect()
            .await
            .map(|b| b.to_bytes())
            .map_err(|e| self.fail(format!("reading the model's answer: {e}"), true))?;
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if status.is_success() {
            if v.is_null() {
                return Err(self.fail(
                    "the model API answered with something that isn't JSON".into(),
                    false,
                ));
            }
            return Ok(v);
        }
        let why = v
            .pointer("/error/message")
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .and_then(Value::as_str)
            .map(|s| s.chars().take(500).collect::<String>())
            .unwrap_or_else(|| "no reason given".into());
        let code = status.as_u16();
        let (message, retryable) = match code {
            401 | 403 => (
                format!("the model API refused the key ({code}): {why}"),
                false,
            ),
            429 => (format!("the model API is rate limiting ({why})"), true),
            500..=599 => (
                format!("the model API is unavailable ({code}): {why}"),
                true,
            ),
            _ => (
                format!("the model API refused the request ({code}): {why}"),
                false,
            ),
        };
        Err(ModelError {
            message: self.scrub(&message),
            retryable,
            retry_after,
        })
    }
}

impl Model for Client {
    fn describe(&self) -> String {
        format!(
            "{} {}",
            self.settings.provider.as_str(),
            self.settings.model
        )
    }

    fn start(&self, text: &str) -> Conversation {
        Conversation {
            messages: vec![json!({"role": "user", "content": text})],
        }
    }

    fn turn<'a>(
        &'a self,
        system: &'a str,
        tools: &'a [ToolDef],
        c: &'a mut Conversation,
    ) -> BoxFuture<'a, Result<Turn, ModelError>> {
        Box::pin(async move {
            let s = &self.settings;
            let (path, body) = match s.provider {
                Provider::Anthropic => (
                    "/v1/messages",
                    anthropic::request(&s.model, s.max_tokens, system, tools, c),
                ),
                Provider::Openai => (
                    "/chat/completions",
                    openai::request(&s.model, s.max_tokens, system, tools, c),
                ),
            };
            let v = self.post(path, &body).await?;
            let parsed = match s.provider {
                Provider::Anthropic => anthropic::response(&v),
                Provider::Openai => openai::response(&v),
            };
            let (turn, message) = parsed.map_err(|e| self.fail(e, false))?;
            c.messages.push(message);
            Ok(turn)
        })
    }

    fn answer(&self, c: &mut Conversation, results: &[(String, Output)], note: Option<&str>) {
        match self.settings.provider {
            Provider::Anthropic => anthropic::answer(c, results, note),
            Provider::Openai => openai::answer(c, results, note),
        }
    }
}

/// Anthropic's Messages API.
pub mod anthropic {
    use super::*;

    pub fn request(
        model: &str,
        max_tokens: u32,
        system: &str,
        tools: &[ToolDef],
        c: &Conversation,
    ) -> Value {
        let tools: Vec<Value> = tools
            .iter()
            .map(
                |t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}),
            )
            .collect();
        json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "tools": tools,
            "messages": c.messages,
            // Cache the growing prefix: each turn resends the whole task.
            "cache_control": {"type": "ephemeral"},
        })
    }

    /// The turn, and the assistant message to append (verbatim: thinking
    /// blocks must go back unchanged).
    pub fn response(v: &Value) -> Result<(Turn, Value), String> {
        let content = v
            .get("content")
            .and_then(Value::as_array)
            .ok_or("the model's answer has no content")?;
        let mut text = Vec::new();
        let mut calls = Vec::new();
        for b in content {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        text.push(t.to_string());
                    }
                }
                Some("tool_use") => calls.push(Call {
                    id: b
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: b
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: Ok(b.get("input").cloned().unwrap_or(json!({}))),
                }),
                _ => {}
            }
        }
        let stop = match v.get("stop_reason").and_then(Value::as_str) {
            Some("end_turn") | Some("stop_sequence") => Stop::EndTurn,
            Some("tool_use") => Stop::ToolUse,
            Some("max_tokens") => Stop::MaxTokens,
            Some("refusal") => Stop::Refusal,
            Some(other) => Stop::Other(other.to_string()),
            None => Stop::Other("none".into()),
        };
        let u = |k: &str| {
            v.pointer(&format!("/usage/{k}"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let usage = Usage {
            input_tokens: u("input_tokens")
                + u("cache_creation_input_tokens")
                + u("cache_read_input_tokens"),
            output_tokens: u("output_tokens"),
        };
        Ok((
            Turn {
                text: text.join("\n"),
                calls,
                stop,
                usage,
            },
            json!({"role": "assistant", "content": content}),
        ))
    }

    pub fn answer(c: &mut Conversation, results: &[(String, Output)], note: Option<&str>) {
        let mut content: Vec<Value> = results
            .iter()
            .map(|(id, out)| {
                json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": out.text,
                    "is_error": out.is_error,
                })
            })
            .collect();
        if let Some(n) = note {
            content.push(json!({"type": "text", "text": n}));
        }
        if content.is_empty() {
            return;
        }
        // A note after tool results joins their message: turns alternate.
        if results.is_empty() {
            if let Some(last) = c.messages.last_mut().filter(|m| m["role"] == "user") {
                if let Some(s) = last["content"].as_str().map(str::to_string) {
                    last["content"] = json!([{"type": "text", "text": s}]);
                }
                if let Some(blocks) = last["content"].as_array_mut() {
                    blocks.extend(content);
                    return;
                }
            }
        }
        c.messages.push(json!({"role": "user", "content": content}));
    }
}

/// OpenAI-compatible chat completions.
pub mod openai {
    use super::*;

    pub fn request(
        model: &str,
        max_tokens: u32,
        system: &str,
        tools: &[ToolDef],
        c: &Conversation,
    ) -> Value {
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {"name": t.name, "description": t.description, "parameters": t.schema},
                })
            })
            .collect();
        let mut messages = vec![json!({"role": "system", "content": system})];
        messages.extend(c.messages.iter().cloned());
        json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "tools": tools,
        })
    }

    pub fn response(v: &Value) -> Result<(Turn, Value), String> {
        let choice = v
            .pointer("/choices/0")
            .ok_or("the model's answer has no choices")?;
        let m = choice
            .get("message")
            .ok_or("the model's answer has no message")?;
        let text = m
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let raw_calls = m
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let calls: Vec<Call> = raw_calls
            .iter()
            .map(|tc| {
                let args = tc
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                Call {
                    id: tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: if args.trim().is_empty() {
                        Ok(json!({}))
                    } else {
                        serde_json::from_str(args).map_err(|e| e.to_string())
                    },
                }
            })
            .collect();
        // Some servers say "stop" with tool calls in hand: the calls decide.
        let stop = match choice.get("finish_reason").and_then(Value::as_str) {
            Some("length") => Stop::MaxTokens,
            Some("content_filter") => Stop::Refusal,
            _ if !calls.is_empty() => Stop::ToolUse,
            Some("stop") | None => Stop::EndTurn,
            Some(other) => Stop::Other(other.to_string()),
        };
        let u = |k: &str| {
            v.pointer(&format!("/usage/{k}"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        let mut message = json!({"role": "assistant", "content": m.get("content").cloned().unwrap_or(Value::Null)});
        if !raw_calls.is_empty() {
            message["tool_calls"] = Value::Array(raw_calls);
        }
        Ok((
            Turn {
                text,
                calls,
                stop,
                usage: Usage {
                    input_tokens: u("prompt_tokens"),
                    output_tokens: u("completion_tokens"),
                },
            },
            message,
        ))
    }

    pub fn answer(c: &mut Conversation, results: &[(String, Output)], note: Option<&str>) {
        for (id, out) in results {
            // No error flag in this API: say so in the text.
            let content = if out.is_error {
                format!("error: {}", out.text)
            } else {
                out.text.clone()
            };
            c.messages
                .push(json!({"role": "tool", "tool_call_id": id, "content": content}));
        }
        if let Some(n) = note {
            c.messages.push(json!({"role": "user", "content": n}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(text: &str, is_error: bool) -> Output {
        Output {
            text: text.into(),
            is_error,
        }
    }

    #[test]
    fn the_key_never_prints_and_is_scrubbed() {
        let k = Key::new("sk-ant-secret-1234567890\n").unwrap();
        assert_eq!(format!("{k:?}"), "Key(<redacted>)");
        assert_eq!(
            k.scrub("refused sk-ant-secret-1234567890 here"),
            "refused <model key> here"
        );
        assert!(Key::new("  ").is_err());
        assert!(Key::new("two words").is_err());
        let settings = Settings::resolve(Some(Provider::Anthropic), None, None, 100, Some(&k))
            .unwrap()
            .unwrap();
        let c = Client::new(settings, Some(k)).unwrap();
        assert!(!format!("{c:?}").contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("spall-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("key");
        std::fs::write(&f, "sk-from-file-123456\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = Key::load(Some(&f), None).unwrap_err().to_string();
        assert!(e.contains("chmod 600"), "{e}");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        let k = Key::load(Some(&f), Some("sk-from-env-123456".into()))
            .unwrap()
            .unwrap();
        assert_eq!(k.0, "sk-from-file-123456", "the file wins over the env");
        let k = Key::load(None, Some("sk-from-env-123456".into()))
            .unwrap()
            .unwrap();
        assert_eq!(k.0, "sk-from-env-123456");
        assert!(Key::load(None, None).unwrap().is_none());
        assert!(Key::load(None, Some(" ".into())).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn model_urls_are_https_or_local() {
        for good in [
            "https://api.anthropic.com",
            "https://api.anthropic.com/",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8000/v1",
            "http://host.docker.internal:11434/v1",
            "http://ollama:11434/v1",
            "http://10.1.2.3:8000/v1",
            "http://192.168.1.5/v1",
            "http://[::1]:8000/v1",
            "http://gpu.lan:8000/v1",
        ] {
            assert!(parse_url(good).is_ok(), "{good}");
        }
        assert_eq!(
            parse_url("https://x.test/v1/").unwrap(),
            "https://x.test/v1"
        );
        for bad in [
            "http://api.example.com/v1",
            "http://8.8.8.8/v1",
            "ftp://x.test",
            "api.anthropic.com",
            "https://user:pw@x.test",
            "https://x.test/v1?key=1",
            "https://x.test/v1#f",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_need_what_each_provider_needs() {
        let k = Key::new("sk-123456789").unwrap();
        let a = Settings::resolve(Some(Provider::Anthropic), None, None, 100, Some(&k))
            .unwrap()
            .unwrap();
        assert_eq!(a.model, ANTHROPIC_MODEL);
        assert_eq!(a.url, ANTHROPIC_URL);
        assert!(Settings::resolve(Some(Provider::Anthropic), None, None, 100, None).is_err());
        let o = Settings::resolve(
            Some(Provider::Openai),
            Some("qwen3"),
            Some("http://localhost:11434/v1/"),
            100,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.url, "http://localhost:11434/v1");
        assert!(Settings::resolve(
            Some(Provider::Openai),
            None,
            Some("http://localhost/v1"),
            1,
            None
        )
        .is_err());
        assert!(Settings::resolve(Some(Provider::Openai), Some("m"), None, 1, None).is_err());
        assert!(Settings::resolve(None, None, None, 1, None)
            .unwrap()
            .is_none());
        assert!(
            Settings::resolve(None, Some("m"), None, 1, None).is_err(),
            "a model without a provider is a mistake, not silence"
        );
    }

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "filter".into(),
            description: "find exchanges".into(),
            schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        }]
    }

    #[test]
    fn anthropic_round_trip_keeps_the_assistant_turn_verbatim() {
        let mut c = Conversation {
            messages: vec![json!({"role": "user", "content": "go"})],
        };
        let req = anthropic::request("claude-opus-5-5", 16000, "sys", &tools(), &c);
        assert_eq!(req["system"], "sys");
        assert_eq!(req["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(req["cache_control"]["type"], "ephemeral");
        assert!(
            req.get("temperature").is_none(),
            "2026-10-21 has no sampling params"
        );
        let answer = json!({
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "text", "text": "Looking."},
                {"type": "tool_use", "id": "toolu_1", "name": "filter", "input": {"q": "host:a"}},
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "cache_read_input_tokens": 90, "cache_creation_input_tokens": 5, "output_tokens": 7},
        });
        let (turn, msg) = anthropic::response(&answer).unwrap();
        assert_eq!(turn.text, "Looking.");
        assert_eq!(turn.stop, Stop::ToolUse);
        assert_eq!(turn.calls[0].input, Ok(json!({"q": "host:a"})));
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 105,
                output_tokens: 7
            }
        );
        assert_eq!(
            msg["content"], answer["content"],
            "thinking goes back unchanged"
        );
        c.messages.push(msg);
        anthropic::answer(
            &mut c,
            &[("toolu_1".into(), out("none", true))],
            Some("tester: look at /admin"),
        );
        let last = c.messages.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(last["content"][0]["is_error"], true);
        assert_eq!(last["content"][1]["text"], "tester: look at /admin");
        // A later note joins the same user message.
        anthropic::answer(&mut c, &[], Some("approved once"));
        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.messages[2]["content"][2]["text"], "approved once");
        let mut fresh = Conversation {
            messages: vec![json!({"role": "user", "content": "go"})],
        };
        anthropic::answer(&mut fresh, &[], Some("more"));
        assert_eq!(fresh.messages.len(), 1);
        assert_eq!(fresh.messages[0]["content"][1]["text"], "more");
        for (reason, stop) in [
            ("end_turn", Stop::EndTurn),
            ("max_tokens", Stop::MaxTokens),
            ("refusal", Stop::Refusal),
        ] {
            let (t, _) =
                anthropic::response(&json!({"content": [], "stop_reason": reason})).unwrap();
            assert_eq!(t.stop, stop);
        }
        assert!(anthropic::response(&json!({"type": "error"})).is_err());
    }

    /// A fake Messages API on loopback: what a real client sends, and
    /// what it makes of the answers.
    #[tokio::test]
    async fn the_client_speaks_to_the_api_and_never_repeats_the_key() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let hits = Arc::new(AtomicU32::new(0));
        let h = hits.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let h = h.clone();
                async move {
                    let n = h.fetch_add(1, Ordering::SeqCst);
                    let key = headers["x-api-key"].to_str().unwrap().to_string();
                    assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
                    let req: Value = serde_json::from_str(&body).unwrap();
                    if req["model"] == "bad-key" {
                        let msg = json!({"error": {"message": format!("invalid x-api-key {key}")}});
                        return (StatusCode::UNAUTHORIZED, axum::Json(msg)).into_response();
                    }
                    if n == 0 {
                        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "0")], "{}")
                            .into_response();
                    }
                    axum::Json(json!({
                        "content": [{"type": "tool_use", "id": "t1", "name": "task", "input": {}}],
                        "stop_reason": "tool_use",
                        "usage": {"input_tokens": 12, "output_tokens": 3},
                    }))
                    .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let key = Key::new("sk-ant-test-0123456789").unwrap();
        let settings = |model: &str| {
            Settings::resolve(
                Some(Provider::Anthropic),
                Some(model),
                Some(&url),
                1024,
                Some(&key),
            )
            .unwrap()
            .unwrap()
        };
        let client = Client::new(settings("claude-opus-5-5"), Some(key.clone())).unwrap();
        let mut c = client.start("go");
        let t = client.turn("sys", &[], &mut c).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2, "a 429 is tried again");
        assert_eq!((t.stop, t.calls[0].name.as_str()), (Stop::ToolUse, "task"));
        assert_eq!(c.messages.len(), 2);

        let client = Client::new(settings("bad-key"), Some(key)).unwrap();
        let e = client
            .turn("sys", &[], &mut client.start("go"))
            .await
            .unwrap_err();
        assert!(!e.retryable);
        assert!(e.message.contains("refused the key"), "{e}");
        assert!(
            !e.message.contains("0123456789"),
            "the key is scrubbed: {e}"
        );
    }

    #[test]
    fn openai_round_trip() {
        let mut c = Conversation {
            messages: vec![json!({"role": "user", "content": "go"})],
        };
        let req = openai::request("qwen3", 4000, "sys", &tools(), &c);
        assert_eq!(
            req["messages"][0],
            json!({"role": "system", "content": "sys"})
        );
        assert_eq!(req["messages"][1]["content"], "go");
        assert_eq!(req["tools"][0]["function"]["parameters"]["type"], "object");
        let answer = json!({
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "filter", "arguments": "{\"q\":\"x\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "filter", "arguments": "{not json"}},
            ]}}],
            "usage": {"prompt_tokens": 50, "completion_tokens": 9},
        });
        let (turn, msg) = openai::response(&answer).unwrap();
        assert_eq!(
            turn.stop,
            Stop::ToolUse,
            "tool calls in hand decide, whatever finish_reason says"
        );
        assert_eq!(turn.calls[0].input, Ok(json!({"q": "x"})));
        assert!(turn.calls[1].input.is_err());
        assert_eq!(
            turn.usage,
            Usage {
                input_tokens: 50,
                output_tokens: 9
            }
        );
        assert_eq!(msg["tool_calls"][0]["id"], "c1");
        c.messages.push(msg);
        openai::answer(
            &mut c,
            &[
                ("c1".into(), out("found", false)),
                ("c2".into(), out("bad args", true)),
            ],
            None,
        );
        assert_eq!(c.messages[2]["role"], "tool");
        assert_eq!(c.messages[3]["content"], "error: bad args");
        let (t, _) = openai::response(
            &json!({"choices": [{"finish_reason": "length", "message": {"content": "x"}}]}),
        )
        .unwrap();
        assert_eq!(t.stop, Stop::MaxTokens);
        let (t, _) = openai::response(
            &json!({"choices": [{"finish_reason": "stop", "message": {"content": "done"}}]}),
        )
        .unwrap();
        assert_eq!((t.stop, t.text.as_str()), (Stop::EndTurn, "done"));
    }
}
