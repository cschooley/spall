//! Candidates (#42): flux findings flagged during an engagement, kept by
//! spall until triage. Each is a `flux::Finding` (spall defines no finding
//! type of its own) plus spall-only bookkeeping: its id, who flagged it,
//! and when. One file per engagement under `<engagements>/candidates/`,
//! written atomically, mode 0600. Unredacted: this is the tester's local
//! store; redaction happens on export (M5).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flux::fingerprint::{self, DAST_V1};
use flux::{
    ArtifactContent, Disposition, Finding, Level, Location, Provenance, Status, Suppression, Taxon,
    WebRequest, WebResponse,
};
use serde::{Deserialize, Serialize};
use taphttp_types::{BodyEncoding, TrafficEntry};

use crate::engagement::{create_dir_private, write_atomic};
use crate::rules::Entry;

pub const SCHEMA_VERSION: u32 = 1;
/// Per engagement; a flagging session never gets near it, a runaway does.
pub const MAX_PER_ENGAGEMENT: usize = 10_000;
/// Bodies larger than this aren't copied inline; the exchange id is kept.
pub const MAX_INLINE_BODY: usize = 256 * 1024;
pub const MAX_MESSAGE: usize = 4000;
pub const MAX_JUSTIFICATION: usize = 2000;
pub const MAX_EVIDENCE: usize = 200;
/// The parameter the candidate is about, in the finding's property bag.
pub const PARAMETER_KEY: &str = "spall/parameter";

/// A candidate: a flux finding and spall's bookkeeping around it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub created: String,
    pub updated: String,
    /// `human` now; agents (M6) will be `agent:<task>`.
    pub flagged_by: String,
    /// When a human promoted it for export (#50); never an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted: Option<String>,
    pub finding: Finding,
}

impl Candidate {
    /// Can be promoted: confirmed (or suspected) and not suppressed (flux
    /// SPEC.md, "Flows": promotion).
    pub fn promotable(&self) -> bool {
        matches!(
            self.finding.status(),
            Some(Status::Confirmed) | Some(Status::Suspected)
        ) && !self.finding.is_suppressed()
    }

    pub fn parameter(&self) -> Option<&str> {
        self.finding.properties.extra.get(PARAMETER_KEY)?.as_str()
    }

    /// The exchanges backing it, decisive one first.
    pub fn exchanges(&self) -> &[String] {
        &self.finding.properties.replay_ids
    }

    /// Shares a fingerprint value with `f` (flux: consumers match on any
    /// shared key).
    pub fn matches(&self, f: &Finding) -> bool {
        f.fingerprints
            .iter()
            .any(|(k, v)| self.finding.fingerprints.get(k) == Some(v))
    }
}

/// What the tester gives when flagging an exchange.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub exchange: String,
    pub rule: String,
    #[serde(default)]
    pub parameter: Option<String>,
    #[serde(default)]
    pub level: Option<Level>,
    #[serde(default)]
    pub rank: Option<f64>,
    #[serde(default)]
    pub message: Option<String>,
    /// More exchanges (replays) backing it, after the decisive one.
    #[serde(default)]
    pub replays: Vec<String>,
    /// Only for `spall/other`: the CWE, when the tester knows it.
    #[serde(default)]
    pub cwe: Option<u32>,
}

impl Flag {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(p) = &self.parameter {
            if p.is_empty() || p.len() > 200 || p.chars().any(char::is_control) {
                return Err("parameter: 1-200 characters, no control characters".into());
            }
        }
        if let Some(r) = self.rank {
            if !(0.0..=100.0).contains(&r) {
                return Err("rank: 0-100".into());
            }
        }
        if let Some(m) = &self.message {
            message_ok(m)?;
        }
        if self.replays.len() > MAX_EVIDENCE {
            return Err(format!("replays: at most {MAX_EVIDENCE}"));
        }
        if self.cwe == Some(0) {
            return Err("cwe: a CWE number".into());
        }
        Ok(())
    }
}

fn message_ok(m: &str) -> Result<(), String> {
    if m.trim().is_empty() || m.chars().count() > MAX_MESSAGE {
        return Err(format!("message: 1-{MAX_MESSAGE} characters"));
    }
    Ok(())
}

fn justification_ok(j: &str) -> Result<(), String> {
    if j.trim().is_empty() || j.chars().count() > MAX_JUSTIFICATION {
        return Err(format!(
            "justification: required, at most {MAX_JUSTIFICATION} characters"
        ));
    }
    Ok(())
}

/// A triage or edit step (`PATCH /api/candidates/<id>`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum Op {
    /// Real: `flux/status: confirmed`, and a `correlationGuid` if it has none.
    Confirm,
    /// Not real (false positive).
    Reject {
        justification: String,
    },
    /// Real, and the risk is accepted.
    Accept {
        justification: String,
    },
    /// The same issue as candidate `of`.
    Duplicate {
        of: String,
        justification: String,
    },
    /// Undo a reject, accept or duplicate: its suppressions are removed.
    Reopen,
    Edit {
        #[serde(default)]
        level: Option<Level>,
        #[serde(default)]
        rank: Option<f64>,
        #[serde(default)]
        message: Option<String>,
    },
    /// More exchanges backing it: appended, never replacing.
    Evidence {
        exchanges: Vec<String>,
    },
    /// Mark it for export (#50). A human's call, never an agent's.
    Promote,
    Unpromote,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::Confirm => "confirm",
            Op::Reject { .. } => "reject",
            Op::Accept { .. } => "accept",
            Op::Duplicate { .. } => "duplicate",
            Op::Reopen => "reopen",
            Op::Edit { .. } => "edit",
            Op::Evidence { .. } => "evidence",
            Op::Promote => "promote",
            Op::Unpromote => "unpromote",
        }
    }
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    Full,
    Save(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no candidate {id}"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::Full => write!(
                f,
                "this engagement has {MAX_PER_ENGAGEMENT} candidates already"
            ),
            Error::Save(m) => write!(f, "saving candidates: {m}"),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct FileBody {
    schema_version: u32,
    engagement: String,
    candidates: Vec<Candidate>,
}

/// Every engagement's candidates, loaded at start.
pub struct Store {
    dir: PathBuf,
    by_engagement: Mutex<HashMap<String, Vec<Candidate>>>,
}

impl Store {
    /// Load `dir` (created if missing, mode 0700). A file that doesn't
    /// parse, or comes from a newer spall, is an error: spall doesn't start
    /// with candidates silently missing.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        use anyhow::Context;
        create_dir_private(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut by_engagement = HashMap::new();
        for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let (eng, list) = load(&path)?;
            by_engagement.insert(eng, list);
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            by_engagement: Mutex::new(by_engagement),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Candidate>>> {
        self.by_engagement.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The engagement's candidates, newest first.
    pub fn list(&self, engagement: &str) -> Vec<Candidate> {
        let mut v = self.lock().get(engagement).cloned().unwrap_or_default();
        v.reverse();
        v
    }

    pub fn get(&self, engagement: &str, id: &str) -> Option<Candidate> {
        self.lock()
            .get(engagement)?
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// An existing candidate sharing a fingerprint with `f`, if any.
    pub fn matching(&self, engagement: &str, f: &Finding) -> Option<Candidate> {
        self.lock()
            .get(engagement)?
            .iter()
            .find(|c| c.matches(f))
            .cloned()
    }

    /// Add `c` and save. On a failed save nothing is added.
    pub fn add(&self, engagement: &str, c: Candidate) -> Result<Candidate, Error> {
        let mut all = self.lock();
        let list = all.entry(engagement.to_string()).or_default();
        if list.len() >= MAX_PER_ENGAGEMENT {
            return Err(Error::Full);
        }
        list.push(c.clone());
        if let Err(e) = save(&self.dir, engagement, list) {
            list.pop();
            return Err(Error::Save(format!("{e:#}")));
        }
        Ok(c)
    }

    /// Apply `op` to candidate `id` and save. Returns it before and after.
    /// `guid` makes a new `correlationGuid` when one is needed.
    pub fn apply(
        &self,
        engagement: &str,
        id: &str,
        op: &Op,
        now: &str,
        guid: &mut dyn FnMut() -> String,
    ) -> Result<(Candidate, Candidate), Error> {
        let mut all = self.lock();
        let list = all
            .get_mut(engagement)
            .ok_or_else(|| Error::NotFound(id.into()))?;
        let at = list
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(id.into()))?;
        let before_list = list.clone();
        let before = list[at].clone();
        // A duplicate names the original's correlationGuid; give it one.
        if let Op::Duplicate { of, .. } = op {
            if of == id {
                return Err(Error::Invalid("a candidate can't duplicate itself".into()));
            }
            let orig = list
                .iter_mut()
                .find(|c| &c.id == of)
                .ok_or_else(|| Error::Invalid(format!("no candidate {of} to duplicate")))?;
            if orig.finding.correlation_guid.is_none() {
                orig.finding.correlation_guid = Some(guid());
                orig.updated = now.into();
            }
        }
        let original_guid = match op {
            Op::Duplicate { of, .. } => list
                .iter()
                .find(|c| &c.id == of)
                .and_then(|c| c.finding.correlation_guid.clone()),
            _ => None,
        };
        let c = &mut list[at];
        if let Err(e) = step(c, op, original_guid.as_deref(), now, guid) {
            *list = before_list;
            return Err(e);
        }
        c.updated = now.into();
        let after = c.clone();
        if let Err(e) = save(&self.dir, engagement, list) {
            *list = before_list;
            return Err(Error::Save(format!("{e:#}")));
        }
        Ok((before, after))
    }
}

fn step(
    c: &mut Candidate,
    op: &Op,
    original_guid: Option<&str>,
    now: &str,
    guid: &mut dyn FnMut() -> String,
) -> Result<(), Error> {
    match op {
        Op::Promote => {
            if !c.promotable() {
                return Err(Error::Invalid(
                    "only a confirmed candidate that isn't rejected, accepted or a duplicate can be promoted".into(),
                ));
            }
            c.promoted = Some(now.into());
            return Ok(());
        }
        Op::Unpromote => {
            if c.promoted.take().is_none() {
                return Err(Error::Invalid("not promoted".into()));
            }
            return Ok(());
        }
        _ => {}
    }
    // A decision against it takes it out of the export.
    if matches!(
        op,
        Op::Reject { .. } | Op::Accept { .. } | Op::Duplicate { .. }
    ) {
        c.promoted = None;
    }
    let f = &mut c.finding;
    let suppress = |f: &mut Finding, s: Suppression| {
        // One decision in effect at a time: a new one replaces the last.
        f.suppressions.clear();
        f.suppressions.push(s);
    };
    match op {
        Op::Confirm => {
            f.properties.status = Some(Status::Confirmed);
            if f.correlation_guid.is_none() {
                f.correlation_guid = Some(guid());
            }
            if !f.properties.confirmed_by.iter().any(|t| t == "spall") {
                f.properties.confirmed_by.push("spall".into());
            }
        }
        Op::Reject { justification } => {
            justification_ok(justification).map_err(Error::Invalid)?;
            suppress(f, Suppression::false_positive(justification.trim()));
        }
        Op::Accept { justification } => {
            justification_ok(justification).map_err(Error::Invalid)?;
            suppress(f, Suppression::accepted_risk(justification.trim()));
        }
        Op::Duplicate { justification, .. } => {
            justification_ok(justification).map_err(Error::Invalid)?;
            let g = original_guid
                .ok_or_else(|| Error::Invalid("the original has no correlationGuid".into()))?;
            suppress(f, Suppression::duplicate_of(g, justification.trim()));
        }
        Op::Reopen => {
            if f.suppressions.is_empty() {
                return Err(Error::Invalid(
                    "not rejected, accepted or a duplicate".into(),
                ));
            }
            f.suppressions.clear();
        }
        Op::Edit {
            level,
            rank,
            message,
        } => {
            if level.is_none() && rank.is_none() && message.is_none() {
                return Err(Error::Invalid("edit: give level, rank or message".into()));
            }
            if let Some(r) = rank {
                if !(0.0..=100.0).contains(r) {
                    return Err(Error::Invalid("rank: 0-100".into()));
                }
                f.rank = Some(*r);
            }
            if let Some(m) = message {
                message_ok(m).map_err(Error::Invalid)?;
                f.message.text = m.trim().into();
            }
            if let Some(l) = level {
                f.level = Some(*l);
            }
        }
        Op::Evidence { exchanges } => {
            if exchanges.is_empty() || exchanges.iter().any(|x| !crate::web::valid_exchange_id(x)) {
                return Err(Error::Invalid("evidence: exchange ids".into()));
            }
            let ids = &mut f.properties.replay_ids;
            for x in exchanges {
                if !ids.contains(x) {
                    ids.push(x.clone());
                }
            }
            if ids.len() > MAX_EVIDENCE {
                return Err(Error::Invalid(format!(
                    "evidence: at most {MAX_EVIDENCE} exchanges"
                )));
            }
            let p = f.provenance.get_or_insert_with(Provenance::default);
            p.last_detection_time_utc = Some(now.into());
        }
        Op::Promote | Op::Unpromote => {}
    }
    Ok(())
}

/// An export (#51): the promoted findings as a flux report.
pub struct Export {
    pub report: flux::Report,
    pub findings: usize,
    /// Secrets replaced (0 for an unredacted export).
    pub redactions: usize,
}

/// The promoted, still-promotable candidates of `list` (stored order,
/// oldest first) as a flux report from spall `version`: redacted, or, only
/// when the caller has checked the opt-in, marked `flux/redacted: false`.
/// The report is validated; problems are returned instead of a report.
pub fn export(list: &[Candidate], redacted: bool, version: &str) -> Result<Export, Vec<String>> {
    let mut findings: Vec<Finding> = list
        .iter()
        .filter(|c| c.promoted.is_some() && c.promotable())
        .map(|c| c.finding.clone())
        .collect();
    let mut redactions = 0;
    for f in &mut findings {
        if redacted {
            redactions += flux::redact::redact_finding(f).len();
        } else {
            flux::redact::mark_unredacted(f);
        }
    }
    let mut driver = flux::ToolComponent::tool("spall", version);
    driver.information_uri = Some("https://github.com/cschooley/spall".into());
    driver.rules = crate::rules::flux_rules(findings.iter().map(|f| f.rule_id.as_str()));
    let n = findings.len();
    let report = flux::Report::new(driver, findings);
    let problems = flux::validate::validate(&report);
    if !problems.is_empty() {
        return Err(problems.iter().map(ToString::to_string).collect());
    }
    Ok(Export {
        report,
        findings: n,
        redactions,
    })
}

/// `(host, path)` of a URL as the fingerprint wants them: host lowercased
/// without userinfo or a default port; path with its query.
pub fn host_and_path(url: &str) -> (String, &str) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let path = &rest[end..];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match (scheme.to_ascii_lowercase().as_str(), host.rsplit_once(':')) {
        ("http", Some((h, "80"))) | ("https", Some((h, "443"))) => h,
        _ => host,
    };
    (
        host.to_ascii_lowercase(),
        if path.is_empty() { "/" } else { path },
    )
}

/// The finding for a flagged exchange: rule and CWE from the catalog, the
/// endpoint and decisive request/response from the exchange, the
/// `flux/dast/v1` fingerprint, `flux/status: candidate`.
pub fn finding_for(rule: &Entry, flag: &Flag, entry: &TrafficEntry, now: &str) -> Finding {
    let (host, path) = host_and_path(&entry.url);
    let param = flag.parameter.as_deref();
    let mut f = Finding::new(rule.id, default_message(rule, entry, param));
    if let Some(m) = &flag.message {
        f.message.text = m.trim().into();
    }
    f.level = Some(flag.level.unwrap_or(Level::Warning));
    f.rank = flag.rank;
    f.fingerprints.insert(
        DAST_V1.into(),
        fingerprint::dast_v1(&host, &entry.method, path, param, rule.id),
    );
    f.locations
        .push(Location::endpoint(&entry.method, &entry.url));
    if let Some(cwe) = rule.cwe.or(flag.cwe.filter(|_| rule.cwe.is_none())) {
        f.taxa.push(Taxon::cwe(cwe));
    }
    f.provenance = Some(Provenance {
        first_detection_time_utc: Some(now.into()),
        last_detection_time_utc: Some(now.into()),
        ..Default::default()
    });
    let (req, res) = evidence(entry);
    f.web_request = Some(req);
    f.web_response = res;
    f.properties.status = Some(Status::Candidate);
    f.properties.fingerprint_version = Some(DAST_V1.into());
    f.properties.replay_ids = std::iter::once(entry.id.clone())
        .chain(flag.replays.iter().filter(|r| **r != entry.id).cloned())
        .collect();
    if let Some(p) = param {
        f.properties.extra.insert(PARAMETER_KEY.into(), p.into());
    }
    f
}

fn default_message(rule: &Entry, entry: &TrafficEntry, param: Option<&str>) -> String {
    let url = fingerprint::strip_query(&entry.url);
    let on = param.map_or(String::new(), |p| format!(", parameter {p}"));
    format!("{}: {} {url}{on}", rule.title, entry.method)
}

/// SARIF's headers are an object: a repeated name is joined (", "; one
/// line per value for Set-Cookie, which can't be comma-joined).
fn header_map(headers: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    let mut m: std::collections::BTreeMap<String, String> = Default::default();
    for (k, v) in headers {
        let sep = if k.eq_ignore_ascii_case("set-cookie") {
            "\n"
        } else {
            ", "
        };
        m.entry(k.clone())
            .and_modify(|e| {
                e.push_str(sep);
                e.push_str(v);
            })
            .or_insert_with(|| v.clone());
    }
    m
}

fn body(text: &Option<String>, enc: &BodyEncoding) -> Option<ArtifactContent> {
    let b = text.as_ref()?;
    if b.len() > MAX_INLINE_BODY {
        return None;
    }
    Some(match enc {
        BodyEncoding::Base64 => ArtifactContent {
            binary: Some(b.clone()),
            ..Default::default()
        },
        _ => ArtifactContent::text(b.clone()),
    })
}

fn evidence(e: &TrafficEntry) -> (WebRequest, Option<WebResponse>) {
    let req_headers: Vec<(String, String)> = e
        .req_headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let res_headers: Vec<(String, String)> = e
        .res_headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let req = WebRequest {
        protocol: Some("HTTP".into()),
        target: Some(e.url.clone()),
        method: Some(e.method.clone()),
        headers: header_map(&req_headers),
        body: body(&e.req_body, &e.req_body_encoding),
        ..Default::default()
    };
    let res = e.status.map(|s| WebResponse {
        protocol: Some("HTTP".into()),
        status_code: Some(i64::from(s)),
        headers: header_map(&res_headers),
        body: body(&e.res_body, &e.res_body_encoding),
        ..Default::default()
    });
    let res = res.or_else(|| {
        Some(WebResponse {
            no_response_received: Some(true),
            ..Default::default()
        })
    });
    (req, res)
}

/// A fresh candidate id: `c-` and 8 hex digits.
pub fn new_id() -> String {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    format!("c-{:08x}", u32::from_le_bytes(b))
}

/// A random (v4) GUID for `correlationGuid`.
pub fn new_guid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Waiting for a human decision: still a candidate, not suppressed.
pub fn to_triage(c: &Candidate) -> bool {
    c.finding.status() == Some(Status::Candidate) && !c.finding.is_suppressed()
}

/// What a disposition is called in the UI and audit log.
pub fn disposition_word(d: Disposition) -> &'static str {
    match d {
        Disposition::FalsePositive => "rejected",
        Disposition::AcceptedRisk => "accepted risk",
        Disposition::Duplicate => "duplicate",
    }
}

fn file_for(dir: &Path, engagement: &str) -> PathBuf {
    dir.join(format!("{engagement}.json"))
}

fn load(path: &Path) -> anyhow::Result<(String, Vec<Candidate>)> {
    use anyhow::Context;
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{}: not JSON", path.display()))?;
    let version = v.get("schema_version").and_then(|s| s.as_u64());
    anyhow::ensure!(
        version == Some(u64::from(SCHEMA_VERSION)),
        "{}: schema_version {version:?}; this spall reads {SCHEMA_VERSION} (upgrade spall, or move the file aside)",
        path.display()
    );
    let body: FileBody = serde_json::from_value(v)
        .with_context(|| format!("{}: not a candidates file", path.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str());
    anyhow::ensure!(
        stem == Some(body.engagement.as_str()),
        "{}: engagement {:?} doesn't match the file name",
        path.display(),
        body.engagement
    );
    Ok((body.engagement, body.candidates))
}

fn save(dir: &Path, engagement: &str, list: &[Candidate]) -> anyhow::Result<()> {
    let body = FileBody {
        schema_version: SCHEMA_VERSION,
        engagement: engagement.into(),
        candidates: list.to_vec(),
    };
    let mut bytes = serde_json::to_vec_pretty(&body)?;
    bytes.push(b'\n');
    write_atomic(&file_for(dir, engagement), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules;

    const NOW: &str = "2026-11-23T15:00:00Z";

    fn entry(id: &str, url: &str) -> TrafficEntry {
        serde_json::from_value(serde_json::json!({
            "id": id, "ts": "2026-11-23T14:00:00Z", "host": "shop.example", "method": "GET", "url": url,
            "req_headers": [["host", "shop.example"], ["cookie", "a=1"], ["cookie", "b=2"]],
            "status": 500,
            "res_headers": [["set-cookie", "x=1"], ["set-cookie", "y=2"], ["content-type", "text/plain"]],
            "res_body": "ERROR: unterminated quoted string",
        }))
        .unwrap()
    }

    fn flag(rule: &str, param: Option<&str>) -> Flag {
        Flag {
            exchange: "ex-1".into(),
            rule: rule.into(),
            parameter: param.map(Into::into),
            ..Default::default()
        }
    }

    fn candidate(id: &str, url: &str, param: Option<&str>) -> Candidate {
        let rule = rules::find("WSTG-INPV-05").unwrap();
        Candidate {
            id: id.into(),
            created: NOW.into(),
            updated: NOW.into(),
            flagged_by: "human".into(),
            promoted: None,
            finding: finding_for(rule, &flag(rule.id, param), &entry("ex-1", url), NOW),
        }
    }

    fn guids() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("00000000-0000-4000-8000-{n:012}")
        }
    }

    #[test]
    fn a_flagged_exchange_is_a_valid_flux_candidate() {
        let c = candidate(
            "c-1",
            "https://Shop.Example:443/api/products/17?sort=x'",
            Some("sort"),
        );
        let f = &c.finding;
        assert_eq!(f.rule_id, "WSTG-INPV-05");
        assert_eq!(f.status(), Some(Status::Candidate));
        assert_eq!(f.cwes(), vec![89]);
        assert_eq!(
            f.endpoint(),
            Some(("GET", "https://Shop.Example:443/api/products/17"))
        );
        assert_eq!(
            f.fingerprints[DAST_V1],
            fingerprint::dast_v1(
                "shop.example",
                "GET",
                "/api/products/17",
                Some("sort"),
                "WSTG-INPV-05"
            )
        );
        assert_eq!(f.message.text, "Testing for SQL Injection: GET https://Shop.Example:443/api/products/17, parameter sort");
        assert_eq!(c.parameter(), Some("sort"));
        assert_eq!(c.exchanges(), ["ex-1"]);
        let req = f.web_request.as_ref().unwrap();
        assert_eq!(req.headers["cookie"], "a=1, b=2");
        let res = f.web_response.as_ref().unwrap();
        assert_eq!(res.headers["set-cookie"], "x=1\ny=2");
        assert_eq!(res.status_code, Some(500));
        assert_eq!(
            res.body.as_ref().and_then(|b| b.text.as_deref()),
            Some("ERROR: unterminated quoted string")
        );
        let mut driver = flux::ToolComponent::tool("spall", "0.4.0");
        driver.rules = rules::flux_rules([f.rule_id.as_str()]);
        assert_eq!(
            flux::validate::validate(&flux::Report::new(driver, vec![f.clone()])),
            vec![]
        );
    }

    #[test]
    fn hosts_and_paths_for_the_fingerprint() {
        assert_eq!(
            host_and_path("https://A.test:443/x?q"),
            ("a.test".into(), "/x?q")
        );
        assert_eq!(host_and_path("http://a.test:80"), ("a.test".into(), "/"));
        assert_eq!(
            host_and_path("http://a.test:8080/x"),
            ("a.test:8080".into(), "/x")
        );
        assert_eq!(
            host_and_path("https://u:p@a.test/x"),
            ("a.test".into(), "/x")
        );
        assert_eq!(host_and_path("https://[::1]:443/x"), ("[::1]".into(), "/x"));
        assert_eq!(host_and_path("https://[::1]/x"), ("[::1]".into(), "/x"));
    }

    #[test]
    fn the_same_endpoint_with_another_id_matches() {
        let a = candidate("c-1", "https://a.test/u/1?id=1", Some("id"));
        let b = candidate("c-2", "https://a.test/u/99", Some("id"));
        let other_param = candidate("c-3", "https://a.test/u/1", Some("name"));
        assert!(a.matches(&b.finding));
        assert!(!a.matches(&other_param.finding));
    }

    #[test]
    fn other_takes_the_testers_cwe_catalog_rules_their_own() {
        let e = entry("ex-1", "https://a.test/x");
        let other = rules::find(rules::OTHER).unwrap();
        let mut fl = flag(rules::OTHER, None);
        fl.cwe = Some(693);
        assert_eq!(finding_for(other, &fl, &e, NOW).cwes(), vec![693]);
        let sqli = rules::find("WSTG-INPV-05").unwrap();
        assert_eq!(finding_for(sqli, &fl, &e, NOW).cwes(), vec![89]);
        fl.cwe = None;
        assert!(finding_for(other, &fl, &e, NOW).cwes().is_empty());
    }

    #[test]
    fn large_bodies_are_left_out_and_no_response_is_said() {
        let mut e = entry("ex-1", "https://a.test/x");
        e.res_body = Some("x".repeat(MAX_INLINE_BODY + 1));
        let f = finding_for(
            rules::find("WSTG-INPV-05").unwrap(),
            &flag("WSTG-INPV-05", None),
            &e,
            NOW,
        );
        assert!(f.web_response.unwrap().body.is_none());
        e.status = None;
        let f = finding_for(
            rules::find("WSTG-INPV-05").unwrap(),
            &flag("WSTG-INPV-05", None),
            &e,
            NOW,
        );
        assert_eq!(f.web_response.unwrap().no_response_received, Some(true));
    }

    #[test]
    fn flags_are_validated() {
        let mut f = flag("r", Some(""));
        assert!(f.validate().is_err());
        f.parameter = Some("a\nb".into());
        assert!(f.validate().is_err());
        f.parameter = None;
        f.rank = Some(101.0);
        assert!(f.validate().is_err());
        f.rank = None;
        f.message = Some("  ".into());
        assert!(f.validate().is_err());
        f.message = None;
        assert!(f.validate().is_ok());
        assert!(serde_json::from_value::<Flag>(
            serde_json::json!({"exchange": "e", "rule": "r", "bogus": 1})
        )
        .is_err());
    }

    #[test]
    fn triage_steps() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut g = guids();
        store
            .add("eng", candidate("c-1", "https://a.test/x", Some("q")))
            .unwrap();
        store
            .add("eng", candidate("c-2", "https://a.test/y", Some("q")))
            .unwrap();
        assert_eq!(
            store
                .list("eng")
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["c-2", "c-1"]
        );
        assert!(store.list("other").is_empty());

        let (before, after) = store
            .apply("eng", "c-1", &Op::Confirm, "t1", &mut g)
            .unwrap();
        assert_eq!(before.finding.status(), Some(Status::Candidate));
        assert_eq!(after.finding.status(), Some(Status::Confirmed));
        assert_eq!(
            after.finding.correlation_guid.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(after.finding.properties.confirmed_by, ["spall"]);
        assert_eq!(after.updated, "t1");
        // Confirming again keeps the guid.
        let (_, again) = store
            .apply("eng", "c-1", &Op::Confirm, "t2", &mut g)
            .unwrap();
        assert_eq!(
            again.finding.correlation_guid,
            after.finding.correlation_guid
        );

        let dup = Op::Duplicate {
            of: "c-1".into(),
            justification: "same builder".into(),
        };
        let (_, d) = store.apply("eng", "c-2", &dup, "t3", &mut g).unwrap();
        assert!(d.finding.is_suppressed());
        assert_eq!(
            d.finding.duplicate_of(),
            after.finding.correlation_guid.as_deref()
        );

        // Rejecting replaces the duplicate decision; reopening clears it.
        let (_, r) = store
            .apply(
                "eng",
                "c-2",
                &Op::Reject {
                    justification: "encoded".into(),
                },
                "t4",
                &mut g,
            )
            .unwrap();
        assert_eq!(r.finding.suppressions.len(), 1);
        assert_eq!(
            r.finding.suppressions[0].properties.disposition,
            Some(Disposition::FalsePositive)
        );
        let (_, o) = store
            .apply("eng", "c-2", &Op::Reopen, "t5", &mut g)
            .unwrap();
        assert!(!o.finding.is_suppressed());
        assert!(matches!(
            store.apply("eng", "c-2", &Op::Reopen, "t6", &mut g),
            Err(Error::Invalid(_))
        ));

        let (_, a) = store
            .apply(
                "eng",
                "c-2",
                &Op::Accept {
                    justification: "internal only".into(),
                },
                "t7",
                &mut g,
            )
            .unwrap();
        assert_eq!(
            a.finding.suppressions[0].properties.disposition,
            Some(Disposition::AcceptedRisk)
        );

        let (_, ev) = store
            .apply(
                "eng",
                "c-1",
                &Op::Evidence {
                    exchanges: vec!["ex-1".into(), "ex-9".into(), "ex-8".into()],
                },
                "t8",
                &mut g,
            )
            .unwrap();
        assert_eq!(
            ev.exchanges(),
            ["ex-1", "ex-9", "ex-8"],
            "appended in order, not repeated"
        );
        assert_eq!(
            ev.finding
                .provenance
                .unwrap()
                .last_detection_time_utc
                .as_deref(),
            Some("t8")
        );

        let edit = Op::Edit {
            level: Some(Level::Error),
            rank: Some(90.0),
            message: Some("SQLi in q".into()),
        };
        let (_, e) = store.apply("eng", "c-1", &edit, "t9", &mut g).unwrap();
        assert_eq!(
            (
                e.finding.level,
                e.finding.rank,
                e.finding.message.text.as_str()
            ),
            (Some(Level::Error), Some(90.0), "SQLi in q")
        );

        // c-1 confirmed, c-2 accepted: nothing left to triage.
        assert_eq!(store.list("eng").iter().filter(|c| to_triage(c)).count(), 0);
        store
            .add("eng", candidate("c-3", "https://a.test/z", None))
            .unwrap();
        assert_eq!(store.list("eng").iter().filter(|c| to_triage(c)).count(), 1);

        // Survives a reopen of the store, and stays valid flux.
        let again = Store::open(dir.path()).unwrap();
        assert_eq!(again.list("eng"), store.list("eng"));
        let findings: Vec<Finding> = again.list("eng").into_iter().map(|c| c.finding).collect();
        let mut driver = flux::ToolComponent::tool("spall", "0.4.0");
        driver.rules = rules::flux_rules(["WSTG-INPV-05"]);
        assert_eq!(
            flux::validate::validate(&flux::Report::new(driver, findings)),
            vec![]
        );
    }

    #[test]
    fn bad_steps_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut g = guids();
        store
            .add("eng", candidate("c-1", "https://a.test/x", None))
            .unwrap();
        let before = store.list("eng");
        for op in [
            Op::Reject {
                justification: " ".into(),
            },
            Op::Duplicate {
                of: "c-1".into(),
                justification: "self".into(),
            },
            Op::Duplicate {
                of: "c-9".into(),
                justification: "x".into(),
            },
            Op::Edit {
                level: None,
                rank: None,
                message: None,
            },
            Op::Edit {
                level: None,
                rank: Some(-1.0),
                message: None,
            },
            Op::Evidence { exchanges: vec![] },
            Op::Evidence {
                exchanges: vec!["../x".into()],
            },
        ] {
            assert!(
                store.apply("eng", "c-1", &op, "t", &mut g).is_err(),
                "{op:?}"
            );
            assert_eq!(store.list("eng"), before, "{op:?}");
        }
        assert!(matches!(
            store.apply("eng", "c-9", &Op::Confirm, "t", &mut g),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            store.apply("nope", "c-1", &Op::Confirm, "t", &mut g),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn files_are_private_and_bad_files_refuse_to_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidates");
        let store = Store::open(&path).unwrap();
        store
            .add("eng-1", candidate("c-1", "https://a.test/x", None))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.join("eng-1.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for bad in [
            "nope".to_string(),
            r#"{"schema_version": 2, "engagement": "eng-1", "candidates": []}"#.into(),
            r#"{"schema_version": 1, "engagement": "other", "candidates": []}"#.into(),
        ] {
            fs::write(path.join("eng-1.json"), &bad).unwrap();
            assert!(Store::open(&path).is_err(), "{bad}");
        }
    }

    #[test]
    fn promotion_needs_a_confirmed_undecided_candidate_and_a_decision_undoes_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut g = guids();
        store
            .add("eng", candidate("c-1", "https://a.test/x", Some("q")))
            .unwrap();
        assert!(
            matches!(
                store.apply("eng", "c-1", &Op::Promote, "t", &mut g),
                Err(Error::Invalid(_))
            ),
            "not confirmed"
        );
        store
            .apply("eng", "c-1", &Op::Confirm, "t", &mut g)
            .unwrap();
        let (_, p) = store
            .apply("eng", "c-1", &Op::Promote, "t2", &mut g)
            .unwrap();
        assert_eq!(p.promoted.as_deref(), Some("t2"));
        let (_, u) = store
            .apply("eng", "c-1", &Op::Unpromote, "t3", &mut g)
            .unwrap();
        assert_eq!(u.promoted, None);
        assert!(store
            .apply("eng", "c-1", &Op::Unpromote, "t", &mut g)
            .is_err());
        store
            .apply("eng", "c-1", &Op::Promote, "t4", &mut g)
            .unwrap();
        let (_, a) = store
            .apply(
                "eng",
                "c-1",
                &Op::Accept {
                    justification: "internal".into(),
                },
                "t5",
                &mut g,
            )
            .unwrap();
        assert_eq!(
            a.promoted, None,
            "a decision against it takes it out of the export"
        );
        assert!(
            store
                .apply("eng", "c-1", &Op::Promote, "t", &mut g)
                .is_err(),
            "suppressed"
        );
    }

    #[test]
    fn exports_are_valid_flux_redacted_or_marked() {
        let mut c = candidate("c-1", "https://a.test/x?token=abcdef123456", Some("q"));
        c.finding.properties.status = Some(Status::Confirmed);
        c.promoted = Some(NOW.into());
        let not_promoted = candidate("c-2", "https://a.test/y", None);
        let list = vec![c.clone(), not_promoted];

        let ex = export(&list, true, "0.5.0").unwrap();
        assert_eq!(ex.findings, 1);
        assert!(
            ex.redactions >= 2,
            "cookie header and the token in the target"
        );
        let f = ex.report.findings().next().unwrap();
        assert_eq!(f.properties.redacted, Some(true));
        let req = f.web_request.as_ref().unwrap();
        assert_eq!(req.headers["cookie"], "[REDACTED:cookie]");
        assert!(req
            .target
            .as_deref()
            .unwrap()
            .ends_with("token=[REDACTED:token]"));
        assert_eq!(ex.report.runs[0].tool.driver.name, "spall");
        assert_eq!(ex.report.runs[0].tool.driver.rules.len(), 1);
        assert_eq!(flux::validate::validate(&ex.report), vec![]);

        let raw = export(&list, false, "0.5.0").unwrap();
        let f = raw.report.findings().next().unwrap();
        assert_eq!(f.properties.redacted, Some(false));
        assert_eq!(
            f.web_request.as_ref().unwrap().headers["cookie"],
            "a=1, b=2"
        );
        assert_eq!(raw.redactions, 0);

        // Suppressed since promotion: left out.
        let mut sup = c;
        sup.finding
            .suppressions
            .push(Suppression::false_positive("no"));
        assert_eq!(export(&[sup], true, "0.5.0").unwrap().findings, 0);
    }

    #[test]
    fn guids_and_ids() {
        let g = new_guid();
        assert_eq!(g.len(), 36);
        assert_eq!(&g[14..15], "4");
        assert_ne!(g, new_guid());
        let id = new_id();
        assert!(id.starts_with("c-") && id.len() == 10);
    }
}
