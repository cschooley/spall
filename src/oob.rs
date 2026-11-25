//! Out-of-band interactions (#1): a callback host the request carries, and
//! any DNS/HTTP/SMTP hit on it tied back to the task and request that
//! caused it. Blind SSRF, blind XXE, DNS exfiltration and the like show
//! only this way.
//!
//! spall is a client of a **self-hosted** [interactsh] server (the `oob`
//! compose profile for a lab, or a VPS with a delegated subdomain); never
//! the public `oast.*` servers, since callbacks can carry client data.
//!
//! A primitive in spall's terms (input arrives from outside):
//! - Minting a payload sends nothing; putting it in a request is an
//!   ordinary `replay`/`fetch` through the gate.
//! - Hits are recorded per engagement (0600) and audited; to an agent they
//!   are untrusted output (`untrusted`).
//! - The session (correlation id, secret, RSA key) is kept on disk (0600)
//!   so late callbacks survive a restart; the key never leaves spall.
//! - The e-stop stops agents minting; polling goes on: hits are evidence.
//!
//! The protocol: register a public key under a correlation id, then poll;
//! the server answers with an AES-256 key wrapped by RSA-OAEP (SHA-256)
//! and each interaction AES-256-CFB encrypted (16-byte IV first).
//!
//! [interactsh]: https://github.com/projectdiscovery/interactsh

use crate::engagement::{create_dir_private, write_atomic};
use base64::Engine;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::{Oaep, RsaPrivateKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// interactsh's defaults: a 20-character correlation id, then a
/// 13-character nonce, as one DNS label.
pub const CID_LEN: usize = 20;
pub const NONCE_LEN: usize = 13;
/// Payloads one task may mint.
pub const MAX_PER_TASK: usize = 50;
/// Payloads and interactions kept per engagement.
pub const MAX_KEPT: usize = 5000;
/// A raw request or response kept per interaction.
pub const MAX_RAW: usize = 16 * 1024;
/// How long the poller keeps trying to link a hit to its exchange.
pub const LINK_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// How often the poller asks the server.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(5);

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Where the server is and what it answers for.
#[derive(Clone)]
pub struct Settings {
    /// The server's API, e.g. `http://interactsh` (compose) or
    /// `https://oast.example.net`.
    pub server: String,
    /// The callback domain payloads are under, e.g. `oast.lab`.
    pub domain: String,
    /// A URL targets can reach the server at, for labs without DNS (the
    /// server scans everywhere for the id): `http://interactsh`.
    pub http: Option<String>,
    token: Option<String>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("server", &self.server)
            .field("domain", &self.domain)
            .field("http", &self.http)
            .field("token", &self.token.as_ref().map(|_| "(set)"))
            .finish()
    }
}

impl Settings {
    /// `None` unless both a server and a domain are given (blank counts as
    /// not given); the token from `token_file` or `token`.
    pub fn resolve(
        server: Option<&str>,
        domain: Option<&str>,
        http: Option<&str>,
        token_file: Option<&Path>,
        token: Option<String>,
    ) -> anyhow::Result<Option<Settings>> {
        let blank = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let (Some(server), Some(domain)) = (blank(server), blank(domain)) else {
            return Ok(None);
        };
        anyhow::ensure!(
            server.starts_with("http://") || server.starts_with("https://"),
            "--oast-server: an http(s) URL"
        );
        let domain = domain.trim_matches('.').to_ascii_lowercase();
        anyhow::ensure!(
            !domain.is_empty()
                && domain
                    .split('.')
                    .all(|l| !l.is_empty()
                        && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')),
            "--oast-domain: a domain name like oast.example.net"
        );
        let token = match token_file {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .map_err(|e| anyhow::anyhow!("--oast-token-file {}: {e}", p.display()))?
                    .trim()
                    .to_string(),
            ),
            None => token.map(|t| t.trim().to_string()),
        }
        .filter(|t| !t.is_empty());
        Ok(Some(Settings {
            server: server.trim_end_matches('/').to_string(),
            domain,
            http: blank(http).map(|h| h.trim_end_matches('/').to_string()),
            token,
        }))
    }
}

/// One interaction, as the server reports it (decrypted).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Raw {
    pub protocol: String,
    #[serde(rename = "unique-id")]
    pub unique_id: String,
    #[serde(rename = "full-id")]
    pub full_id: String,
    #[serde(rename = "q-type")]
    pub q_type: String,
    #[serde(rename = "raw-request")]
    pub raw_request: String,
    #[serde(rename = "raw-response")]
    pub raw_response: String,
    #[serde(rename = "smtp-from")]
    pub smtp_from: String,
    #[serde(rename = "remote-address")]
    pub remote_address: String,
    pub timestamp: String,
}

/// Decrypt one poll's interactions with the session's key.
pub fn decrypt(
    key: &RsaPrivateKey,
    aes_key_b64: &str,
    data: &[String],
) -> anyhow::Result<Vec<Raw>> {
    use cfb_mode::cipher::{AsyncStreamCipher, KeyIvInit};
    let wrapped = B64.decode(aes_key_b64.trim())?;
    let aes_key = key.decrypt(Oaep::new::<rsa::sha2::Sha256>(), &wrapped)?;
    let mut out = Vec::new();
    for d in data {
        let bytes = B64.decode(d.trim())?;
        anyhow::ensure!(bytes.len() > 16, "an interaction shorter than its IV");
        let (iv, ct) = bytes.split_at(16);
        let mut buf = ct.to_vec();
        cfb_mode::Decryptor::<aes::Aes256>::new_from_slices(&aes_key, iv)
            .map_err(|e| anyhow::anyhow!("AES key: {e}"))?
            .decrypt(&mut buf);
        out.push(serde_json::from_slice(&buf)?);
    }
    Ok(out)
}

/// A registered session: what lets spall poll its interactions.
#[derive(Serialize, Deserialize)]
struct SessionFile {
    cid: String,
    secret: String,
    /// PKCS#8 PEM. The file is 0600; the key never leaves spall.
    key: String,
}

struct Session {
    cid: String,
    secret: String,
    key: RsaPrivateKey,
}

fn random_label(n: usize) -> String {
    const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut b = vec![0u8; n];
    let _ = getrandom::fill(&mut b);
    b.iter()
        .map(|x| ALPHA[(*x as usize) % ALPHA.len()] as char)
        .collect()
}

/// A minted payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    /// cid + nonce: what a hit's `unique-id` is.
    pub id: String,
    /// `<id>.<domain>`.
    pub host: String,
    /// `http://<id>.<domain>/`, and the lab form when configured.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lab_url: Option<String>,
    /// `human` or `agent:T-n`.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub at: String,
}

/// A recorded interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    /// `I-n` within the engagement.
    pub id: String,
    pub payload: String,
    pub protocol: String,
    pub remote: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub q_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub smtp_from: String,
    /// Capped at [`MAX_RAW`].
    pub raw_request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The exchange that carried the payload, when spall found it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    next: u64,
    payloads: Vec<Payload>,
    hits: Vec<Hit>,
}

/// The client and the records.
pub struct Oob {
    pub settings: Settings,
    http: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        http_body_util::Full<hyper::body::Bytes>,
    >,
    session_file: PathBuf,
    session: tokio::sync::Mutex<Option<Session>>,
    dir: PathBuf,
    books: Mutex<HashMap<String, Book>>,
}

fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} … ({} bytes, cut)", &s[..end], s.len())
}

impl Oob {
    /// The client for `settings`, keeping its session and records under
    /// `dir` (`<engagements>/oob`).
    pub fn new(settings: Settings, dir: &Path) -> anyhow::Result<Oob> {
        create_dir_private(dir)?;
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())?
            .https_or_http()
            .enable_http1()
            .build();
        let mut books = HashMap::new();
        for entry in std::fs::read_dir(dir)?.flatten() {
            let p = entry.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(eng) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let book: Book = serde_json::from_slice(&std::fs::read(&p)?)
                .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            books.insert(eng.to_string(), book);
        }
        Ok(Oob {
            settings,
            http: hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(https),
            session_file: dir.join("session"),
            session: tokio::sync::Mutex::new(None),
            dir: dir.to_path_buf(),
            books: Mutex::new(books),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Book>> {
        self.books.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn save(&self, eng: &str, book: &Book) {
        let r = serde_json::to_vec(book)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.dir.join(format!("{eng}.json")), &b));
        if let Err(e) = r {
            tracing::warn!("saving {eng}'s OOB records: {e:#}");
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        use http_body_util::BodyExt;
        let url = format!("{}{path}", self.settings.server);
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(&url)
            .header("content-type", "application/json");
        if let Some(t) = &self.settings.token {
            req = req.header("authorization", t.as_str());
        }
        let bytes = body
            .map(|b| serde_json::to_vec(&b).unwrap_or_default())
            .unwrap_or_default();
        let req = req.body(http_body_util::Full::new(hyper::body::Bytes::from(bytes)))?;
        let res = tokio::time::timeout(std::time::Duration::from_secs(15), self.http.request(req))
            .await
            .map_err(|_| {
                anyhow::anyhow!("the OOB server at {} took too long", self.settings.server)
            })?
            .map_err(|e| {
                anyhow::anyhow!(
                    "can't reach the OOB server at {}: {e}",
                    self.settings.server
                )
            })?;
        let status = res.status();
        let body = http_body_util::Limited::new(res.into_body(), 32 << 20)
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("reading the OOB server's answer: {e}"))?
            .to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        if !status.is_success() {
            let why = v["error"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| status.to_string());
            anyhow::bail!("the OOB server answered {status}: {why}");
        }
        Ok(v)
    }

    /// The session, registered with the server: the saved one (registered
    /// again, which the server accepts after a restart of its own), or a
    /// new one.
    pub async fn ensure_session(&self) -> anyhow::Result<String> {
        let mut g = self.session.lock().await;
        if let Some(s) = g.as_ref() {
            return Ok(s.cid.clone());
        }
        let saved = std::fs::read(&self.session_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<SessionFile>(&b).ok())
            .and_then(|f| {
                RsaPrivateKey::from_pkcs8_pem(&f.key).ok().map(|k| Session {
                    cid: f.cid,
                    secret: f.secret,
                    key: k,
                })
            });
        let s = match saved {
            Some(s) => s,
            None => {
                let mut rng = rsa::rand_core::OsRng;
                let key = RsaPrivateKey::new(&mut rng, 2048)?;
                let s = Session {
                    cid: random_label(CID_LEN),
                    secret: random_label(32),
                    key,
                };
                let pem = s.key.to_pkcs8_pem(LineEnding::LF)?;
                let file = SessionFile {
                    cid: s.cid.clone(),
                    secret: s.secret.clone(),
                    key: pem.to_string(),
                };
                write_atomic(&self.session_file, &serde_json::to_vec(&file)?)?;
                s
            }
        };
        let public = s.key.to_public_key().to_public_key_pem(LineEnding::LF)?;
        let r = self
            .call(
                "POST",
                "/register",
                Some(serde_json::json!({
                    "public-key": B64.encode(public.as_bytes()),
                    "secret-key": s.secret,
                    "correlation-id": s.cid,
                })),
            )
            .await;
        match r {
            Ok(_) => {}
            // Registered already (spall restarted, the server didn't).
            Err(e) if e.to_string().contains("already") => {}
            Err(e) => return Err(e),
        }
        let cid = s.cid.clone();
        *g = Some(s);
        Ok(cid)
    }

    /// A new payload for `eng`, by `by` (`human`, `agent:T-n`). Sends
    /// nothing; registers the session first if needed.
    pub async fn mint(&self, eng: &str, by: &str, task: Option<&str>) -> anyhow::Result<Payload> {
        let cid = self.ensure_session().await?;
        let id = format!("{cid}{}", random_label(NONCE_LEN));
        let host = format!("{id}.{}", self.settings.domain);
        let p = Payload {
            url: format!("http://{host}/"),
            lab_url: self.settings.http.as_ref().map(|h| format!("{h}/{id}")),
            id,
            host,
            by: by.to_string(),
            task: task.map(String::from),
            at: taphttp_types::Timestamp::now().to_string(),
        };
        let mut all = self.lock();
        let book = all.entry(eng.to_string()).or_default();
        if let Some(t) = task {
            if book
                .payloads
                .iter()
                .filter(|x| x.task.as_deref() == Some(t))
                .count()
                >= MAX_PER_TASK
            {
                anyhow::bail!(
                    "{t} has minted {MAX_PER_TASK} payloads: reuse one (oob_hits lists them)"
                );
            }
        }
        if book.payloads.len() >= MAX_KEPT {
            book.payloads.remove(0);
        }
        book.payloads.push(p.clone());
        self.save(eng, book);
        Ok(p)
    }

    /// Ask the server for new interactions and record each under the
    /// engagement whose payload it hit. Returns them as (engagement, hit).
    pub async fn poll(&self) -> anyhow::Result<Vec<(String, Hit)>> {
        self.ensure_session().await?;
        let (cid, secret) = {
            let g = self.session.lock().await;
            let s = g
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no OOB session"))?;
            (s.cid.clone(), s.secret.clone())
        };
        let v = match self
            .call("GET", &format!("/poll?id={cid}&secret={secret}"), None)
            .await
        {
            Ok(v) => v,
            // The server forgot us (it restarted): register again, next poll.
            Err(e)
                if e.to_string().contains("could not get interactions")
                    || e.to_string().contains("not found") =>
            {
                *self.session.lock().await = None;
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let data: Vec<String> = v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let raws = {
            let g = self.session.lock().await;
            let s = g
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no OOB session"))?;
            decrypt(&s.key, v["aes_key"].as_str().unwrap_or(""), &data)?
        };
        Ok(self.record(raws))
    }

    /// Record decrypted interactions against their payloads.
    pub fn record(&self, raws: Vec<Raw>) -> Vec<(String, Hit)> {
        let mut out = Vec::new();
        let mut all = self.lock();
        let mut touched = Vec::new();
        for r in raws {
            let uid = r.unique_id.to_ascii_lowercase();
            let Some((eng, book)) = all
                .iter_mut()
                .find(|(_, b)| b.payloads.iter().any(|p| p.id == uid))
            else {
                tracing::debug!("an OOB interaction for no payload of ours ({uid})");
                continue;
            };
            let p = book.payloads.iter().find(|p| p.id == uid).cloned();
            book.next += 1;
            let hit = Hit {
                id: format!("I-{}", book.next),
                payload: uid.clone(),
                protocol: cap(&r.protocol, 16),
                remote: cap(&r.remote_address, 64),
                at: if r.timestamp.is_empty() {
                    taphttp_types::Timestamp::now().to_string()
                } else {
                    cap(&r.timestamp, 64)
                },
                q_type: cap(&r.q_type, 16),
                smtp_from: cap(&r.smtp_from, 256),
                raw_request: cap(&r.raw_request, MAX_RAW),
                task: p.and_then(|p| p.task),
                exchange: None,
            };
            if book.hits.len() >= MAX_KEPT {
                book.hits.remove(0);
            }
            book.hits.push(hit.clone());
            if !touched.contains(eng) {
                touched.push(eng.clone());
            }
            out.push((eng.clone(), hit));
        }
        for eng in touched {
            if let Some(b) = all.get(&eng) {
                self.save(&eng, b);
            }
        }
        out
    }

    /// Hits without an exchange yet, recorded within `within`, at most 20:
    /// (engagement, hit).
    pub fn unlinked(&self, within: std::time::Duration) -> Vec<(String, Hit)> {
        let cutoff = jiff::Timestamp::now()
            .checked_sub(jiff::SignedDuration::try_from(within).unwrap_or_default())
            .unwrap_or_else(|_| jiff::Timestamp::now());
        let all = self.lock();
        let mut out = Vec::new();
        for (eng, b) in all.iter() {
            for h in b.hits.iter().rev().take(200) {
                let recent = h.at.parse::<jiff::Timestamp>().is_ok_and(|t| t >= cutoff);
                if h.exchange.is_none() && recent && out.len() < 20 {
                    out.push((eng.clone(), h.clone()));
                }
            }
        }
        out
    }

    /// Note the exchange a hit's payload rode in on.
    pub fn link(&self, eng: &str, hit: &str, exchange: &str) {
        let mut all = self.lock();
        if let Some(b) = all.get_mut(eng) {
            if let Some(h) = b.hits.iter_mut().find(|h| h.id == hit) {
                h.exchange = Some(exchange.to_string());
            }
            self.save(eng, b);
        }
    }

    /// An engagement's payloads and hits, newest first; a task's only when
    /// `task` is given.
    pub fn list(&self, eng: &str, task: Option<&str>) -> (Vec<Payload>, Vec<Hit>) {
        let all = self.lock();
        let Some(b) = all.get(eng) else {
            return (Vec::new(), Vec::new());
        };
        let mine = |t: &Option<String>| task.is_none() || t.as_deref() == task;
        (
            b.payloads
                .iter()
                .rev()
                .filter(|p| mine(&p.task))
                .cloned()
                .collect(),
            b.hits
                .iter()
                .rev()
                .filter(|h| mine(&h.task))
                .cloned()
                .collect(),
        )
    }

    /// Deregister (at shutdown, or when the tester wants the session gone).
    pub async fn deregister(&self) {
        let s = self.session.lock().await.take();
        if let Some(s) = s {
            let _ = self
                .call(
                    "POST",
                    "/deregister",
                    Some(serde_json::json!({"correlation-id": s.cid, "secret-key": s.secret})),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What an interactsh server does to one interaction, for the tests.
    pub fn seal(public: &rsa::RsaPublicKey, raws: &[serde_json::Value]) -> (String, Vec<String>) {
        use cfb_mode::cipher::{AsyncStreamCipher, KeyIvInit};
        let mut rng = rsa::rand_core::OsRng;
        let mut aes_key = [0u8; 32];
        getrandom::fill(&mut aes_key).unwrap();
        let wrapped = public
            .encrypt(&mut rng, Oaep::new::<rsa::sha2::Sha256>(), &aes_key)
            .unwrap();
        let data = raws
            .iter()
            .map(|r| {
                let mut iv = [0u8; 16];
                getrandom::fill(&mut iv).unwrap();
                let mut buf = serde_json::to_vec(r).unwrap();
                cfb_mode::Encryptor::<aes::Aes256>::new_from_slices(&aes_key, &iv)
                    .unwrap()
                    .encrypt(&mut buf);
                B64.encode([iv.as_slice(), &buf].concat())
            })
            .collect();
        (B64.encode(wrapped), data)
    }

    #[test]
    fn interactions_decrypt_as_interactsh_sends_them() {
        let mut rng = rsa::rand_core::OsRng;
        let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let (k, data) = seal(
            &key.to_public_key(),
            &[
                serde_json::json!({"protocol": "dns", "unique-id": "abc", "full-id": "abc", "q-type": "A", "raw-request": "<script>", "remote-address": "203.0.113.9", "timestamp": "2026-11-25T10:00:00Z"}),
            ],
        );
        let raws = decrypt(&key, &k, &data).unwrap();
        assert_eq!(raws[0].protocol, "dns");
        assert_eq!(raws[0].raw_request, "<script>");
        assert!(decrypt(&key, "not base64!", &data).is_err());
    }

    #[test]
    fn settings_need_a_server_and_a_domain_and_never_show_the_token() {
        assert!(Settings::resolve(None, Some("oast.lab"), None, None, None)
            .unwrap()
            .is_none());
        assert!(
            Settings::resolve(Some(" "), Some("oast.lab"), None, None, None)
                .unwrap()
                .is_none(),
            "blank is off"
        );
        assert!(Settings::resolve(Some("ftp://x"), Some("oast.lab"), None, None, None).is_err());
        assert!(
            Settings::resolve(Some("http://x"), Some("not a domain"), None, None, None).is_err()
        );
        let s = Settings::resolve(
            Some("http://interactsh/"),
            Some(".OAST.lab."),
            Some("http://interactsh"),
            None,
            Some("sekrit".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (s.server.as_str(), s.domain.as_str()),
            ("http://interactsh", "oast.lab")
        );
        assert!(!format!("{s:?}").contains("sekrit"));
    }

    #[tokio::test]
    async fn hits_land_on_their_payloads_engagement_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::resolve(
            Some("http://127.0.0.1:9"),
            Some("oast.lab"),
            Some("http://interactsh"),
            None,
            None,
        )
        .unwrap()
        .unwrap();
        let o = Oob::new(s.clone(), dir.path()).unwrap();
        // A session without a server: minting needs one, so fake it.
        *o.session.lock().await = Some(Session {
            cid: "c".repeat(CID_LEN),
            secret: "s".into(),
            key: RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap(),
        });
        let p = o.mint("eng-a", "agent:T-1", Some("T-1")).await.unwrap();
        assert_eq!(p.id.len(), CID_LEN + NONCE_LEN);
        assert!(
            p.host.ends_with(".oast.lab")
                && p.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert_eq!(
            p.lab_url.as_deref(),
            Some(format!("http://interactsh/{}", p.id).as_str())
        );
        let hits = o.record(vec![
            Raw {
                protocol: "http".into(),
                unique_id: p.id.to_uppercase(),
                remote_address: "203.0.113.9".into(),
                raw_request: "x".repeat(MAX_RAW * 2),
                ..Raw::default()
            },
            Raw {
                protocol: "dns".into(),
                unique_id: "someone-elses".into(),
                ..Raw::default()
            },
        ]);
        assert_eq!(hits.len(), 1);
        let (eng, h) = &hits[0];
        assert_eq!(
            (eng.as_str(), h.id.as_str(), h.task.as_deref()),
            ("eng-a", "I-1", Some("T-1"))
        );
        assert!(h.raw_request.len() < MAX_RAW + 64 && h.raw_request.ends_with("cut)"));
        o.link("eng-a", "I-1", "s-9");
        let again = Oob::new(s, dir.path()).unwrap();
        let (ps, hs) = again.list("eng-a", Some("T-1"));
        assert_eq!((ps.len(), hs[0].exchange.as_deref()), (1, Some("s-9")));
        assert!(again.list("eng-a", Some("T-2")).1.is_empty());
    }
}

/// Against a real interactsh-server (opt-in): `SPALL_TEST_INTERACTSH` is
/// its URL, `SPALL_TEST_INTERACTSH_TOKEN` its token. Registers, mints,
/// calls back over HTTP (the server scans everywhere for the id), polls.
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore = "needs a running interactsh-server (SPALL_TEST_INTERACTSH)"]
    async fn a_real_server_round_trip() {
        let Ok(server) = std::env::var("SPALL_TEST_INTERACTSH") else {
            return;
        };
        let token = std::env::var("SPALL_TEST_INTERACTSH_TOKEN").ok();
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::resolve(Some(&server), Some("oast.lab"), Some(&server), None, token)
            .unwrap()
            .unwrap();
        let o = Oob::new(s, dir.path()).unwrap();
        let p = o.mint("eng-live", "human", None).await.unwrap();
        let lab = p.lab_url.clone().unwrap();
        // The callback a vulnerable target would make.
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build_http::<http_body_util::Full<hyper::body::Bytes>>();
        let req = hyper::Request::get(&lab)
            .header("host", p.host.as_str())
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .unwrap();
        client.request(req).await.unwrap();
        let mut hits = Vec::new();
        for _ in 0..20 {
            hits = o.poll().await.unwrap();
            if !hits.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        assert!(!hits.is_empty(), "no interaction came back");
        let (eng, h) = &hits[0];
        assert_eq!(
            (eng.as_str(), h.protocol.as_str(), h.payload.as_str()),
            ("eng-live", "http", p.id.as_str())
        );
        assert!(h.raw_request.contains(&p.id), "{}", h.raw_request);
        // A restart keeps the session: the same id, still registered.
        let again = Oob::new(o.settings.clone(), dir.path()).unwrap();
        assert_eq!(again.ensure_session().await.unwrap(), p.id[..CID_LEN]);
        again.deregister().await;
    }
}
