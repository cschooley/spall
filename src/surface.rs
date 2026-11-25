//! Surface (#95, #94): the endpoints spall has seen, organized for the
//! question an assessment turns on: *what haven't we tested?* (docs/ux.md,
//! Surface).
//!
//! Endpoints come from traffic (fed by the hub, like `hosts`): a method and
//! a path **template**, ids folded (`/orders/1043` is `/orders/{id}`), with
//! who touched it (you, an agent's task), what it answered, and the
//! parameter names seen. Imported API descriptions add the documented ones
//! nobody has touched yet. Coverage (`coverage.rs`) and candidates say, per
//! WSTG category, what was tested and what was found.
//!
//! Counts are what spall saw: taphttp's backfill and the live feed, the
//! same limit as hosts (cschooley/taphttp#21).

use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use taphttp_types::TrafficEntry;

/// Endpoints kept; past this, new ones are counted as dropped.
pub const MAX_ENDPOINTS: usize = 20_000;
const MAX_PARAMS: usize = 50;
const MAX_STATUSES: usize = 12;
const MAX_TASKS: usize = 20;

/// The WSTG v4.2 categories, as coverage columns: code and name.
pub const CATEGORIES: [(&str, &str); 12] = [
    ("INFO", "Information gathering"),
    ("CONF", "Configuration and deployment"),
    ("IDNT", "Identity management"),
    ("ATHN", "Authentication"),
    ("ATHZ", "Authorization"),
    ("SESS", "Session management"),
    ("INPV", "Input validation"),
    ("ERRH", "Error handling"),
    ("CRYP", "Cryptography"),
    ("BUSL", "Business logic"),
    ("CLNT", "Client-side"),
    ("APIT", "API testing"),
];

/// The category of a rule id (`WSTG-ATHZ-04` is `ATHZ`), if it has one.
pub fn category_of(rule: &str) -> Option<&'static str> {
    let code = rule.strip_prefix("WSTG-")?.split('-').next()?;
    CATEGORIES.iter().find(|(c, _)| *c == code).map(|(c, _)| *c)
}

/// One path segment as a template part: ids and the like become
/// placeholders, words stay.
fn segment(s: &str) -> String {
    let hexish = s.len() >= 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let uuid = s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    let numeric = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    // A long token of letters and digits (a slug with digits in it, a key).
    let token = s.len() >= 20
        && s.bytes().any(|b| b.is_ascii_digit())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if numeric {
        "{id}".into()
    } else if uuid {
        "{uuid}".into()
    } else if hexish {
        "{hex}".into()
    } else if token {
        "{token}".into()
    } else if s.starts_with('{') && s.ends_with('}') {
        // Already a template part (an OpenAPI path).
        s.into()
    } else {
        s.into()
    }
}

/// A URL path (no query) as (display template, matching key). The key makes
/// every placeholder `{}`, so `/pets/{petId}` (a spec) matches `/pets/{id}`
/// (traffic).
pub fn template(path: &str) -> (String, String) {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let parts: Vec<String> = path.split('/').map(segment).collect();
    let display = if parts.iter().all(String::is_empty) {
        "/".to_string()
    } else {
        parts.join("/")
    };
    let key = display
        .split('/')
        .map(|p| {
            if p.starts_with('{') && p.ends_with('}') {
                "{}"
            } else {
                p
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    (display, key)
}

/// An endpoint's identity: host, method, template key.
pub fn key(host: &str, method: &str, path_key: &str) -> String {
    format!("{host}|{}|{path_key}", method.to_ascii_uppercase())
}

/// (host, path) from a URL.
pub fn split_url(url: &str) -> (String, String) {
    let (host, path) = crate::candidates::host_and_path(url);
    (host, path.to_string())
}

#[derive(Debug, Clone)]
struct Ep {
    host: String,
    method: String,
    template: String,
    path_key: String,
    count: u64,
    statuses: BTreeSet<u16>,
    params: BTreeSet<String>,
    first: String,
    last: String,
    example: String,
    by_you: u64,
    tasks: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct Inner {
    eps: HashMap<String, Ep>,
    dropped: u64,
}

/// Endpoints seen in traffic.
#[derive(Debug, Default)]
pub struct Surface {
    inner: Mutex<Inner>,
}

/// An endpoint as the Surface view and the agent see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub key: String,
    pub host: String,
    pub method: String,
    pub template: String,
    pub count: u64,
    pub statuses: Vec<u16>,
    pub params: Vec<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// The newest exchange seen on it: open it, or run a verb on it.
    pub example: Option<String>,
    /// Requests you (or your browser through the proxy) made.
    pub by_you: u64,
    /// Agent tasks that sent to it.
    pub tasks: Vec<String>,
    /// In an imported API description (#40).
    pub documented: bool,
}

/// Parameter names a request carries: the query's, and a form or JSON
/// body's top-level ones.
fn params_of(e: &TrafficEntry) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((_, q)) = e.url.split_once('?') {
        for pair in q.split('#').next().unwrap_or("").split('&') {
            let name = pair.split('=').next().unwrap_or("");
            if !name.is_empty() && name.len() <= 64 {
                out.push(name.to_string());
            }
        }
    }
    let ctype = e
        .req_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_ascii_lowercase())
        .unwrap_or_default();
    if let (Some(body), taphttp_types::BodyEncoding::Utf8) = (&e.req_body, e.req_body_encoding) {
        if body.len() <= 64 * 1024 {
            if ctype.contains("json") {
                if let Ok(serde_json::Value::Object(m)) =
                    serde_json::from_str::<serde_json::Value>(body)
                {
                    out.extend(m.keys().filter(|k| k.len() <= 64).cloned());
                }
            } else if ctype.contains("x-www-form-urlencoded") {
                for pair in body.split('&') {
                    let name = pair.split('=').next().unwrap_or("");
                    if !name.is_empty() && name.len() <= 64 {
                        out.push(name.to_string());
                    }
                }
            }
        }
    }
    out
}

impl Surface {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Count an exchange on its endpoint.
    pub fn observe(&self, e: &TrafficEntry) {
        let (host, path) = split_url(&e.url);
        if host.is_empty() || host.len() > 255 || e.method.len() > 16 {
            return;
        }
        let (template, path_key) = template(&path);
        if template.len() > 1024 {
            return;
        }
        let k = key(&host, &e.method, &path_key);
        let ts = e.ts.to_string();
        let agent_task = e
            .meta
            .get("actor")
            .filter(|a| a.as_str() == "agent")
            .and_then(|_| e.meta.get("task"))
            .map(|t| t.to_string());
        let params = params_of(e);
        let mut inner = self.lock();
        if !inner.eps.contains_key(&k) {
            if inner.eps.len() >= MAX_ENDPOINTS {
                inner.dropped += 1;
                return;
            }
            inner.eps.insert(
                k.clone(),
                Ep {
                    host,
                    method: e.method.to_ascii_uppercase(),
                    template,
                    path_key,
                    count: 0,
                    statuses: BTreeSet::new(),
                    params: BTreeSet::new(),
                    first: ts.clone(),
                    last: ts.clone(),
                    example: e.id.clone(),
                    by_you: 0,
                    tasks: BTreeSet::new(),
                },
            );
        }
        let Some(ep) = inner.eps.get_mut(&k) else {
            return;
        };
        ep.count += 1;
        if ts >= ep.last {
            ep.last.clone_from(&ts);
            ep.example.clone_from(&e.id);
        }
        if ts < ep.first {
            ep.first.clone_from(&ts);
        }
        if let Some(s) = e.status {
            if ep.statuses.len() < MAX_STATUSES {
                ep.statuses.insert(s);
            }
        }
        for p in params {
            if ep.params.len() < MAX_PARAMS {
                ep.params.insert(p);
            }
        }
        match agent_task {
            Some(t) if ep.tasks.len() < MAX_TASKS => {
                ep.tasks.insert(t);
            }
            Some(_) => {}
            None => ep.by_you += 1,
        }
    }

    /// Every endpoint seen, plus documented ones (`specs`: host, path
    /// template) nobody has touched, as method `ANY`.
    pub fn list(&self, specs: &[(String, String)]) -> Vec<Endpoint> {
        let inner = self.lock();
        let documented: BTreeSet<(String, String)> = specs
            .iter()
            .map(|(h, p)| (h.to_ascii_lowercase(), template(p).1))
            .collect();
        let mut out: Vec<Endpoint> = inner
            .eps
            .iter()
            .map(|(k, e)| Endpoint {
                key: k.clone(),
                host: e.host.clone(),
                method: e.method.clone(),
                template: e.template.clone(),
                count: e.count,
                statuses: e.statuses.iter().copied().collect(),
                params: e.params.iter().cloned().collect(),
                first_seen: e.first.clone(),
                last_seen: e.last.clone(),
                example: Some(e.example.clone()),
                by_you: e.by_you,
                tasks: e.tasks.iter().cloned().collect(),
                documented: documented.contains(&(e.host.clone(), e.path_key.clone())),
            })
            .collect();
        for (h, p) in specs {
            let (display, path_key) = template(p);
            let h = h.to_ascii_lowercase();
            if inner
                .eps
                .values()
                .any(|e| e.host == h && e.path_key == path_key)
            {
                continue;
            }
            let k = key(&h, "ANY", &path_key);
            if out.iter().any(|e| e.key == k) {
                continue;
            }
            out.push(Endpoint {
                key: k,
                host: h,
                method: "ANY".into(),
                template: display,
                count: 0,
                statuses: Vec::new(),
                params: Vec::new(),
                first_seen: String::new(),
                last_seen: String::new(),
                example: None,
                by_you: 0,
                tasks: Vec::new(),
                documented: true,
            });
        }
        out.sort_by(|a, b| {
            (&a.host, &a.template, &a.method).cmp(&(&b.host, &b.template, &b.method))
        });
        out
    }

    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(method: &str, url: &str, ts: &str, extra: serde_json::Value) -> TrafficEntry {
        let mut v = serde_json::json!({
            "schema_version": 8, "id": format!("x-{ts}"), "ts": ts, "host": "h", "method": method, "url": url,
            "req_headers": [], "req_body": null, "req_body_encoding": "utf8", "status": 200,
            "res_headers": [], "res_body": null, "res_body_encoding": "utf8", "duration_ms": 1, "replay_of": null
        });
        if let (Some(m), serde_json::Value::Object(x)) = (v.as_object_mut(), extra) {
            m.extend(x);
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn paths_become_templates_ids_folded_words_kept() {
        assert_eq!(template("/orders/1043/items?x=1").0, "/orders/{id}/items");
        assert_eq!(
            template("/u/3fa85f64-5717-4562-b3fc-2c963f66afa6").0,
            "/u/{uuid}"
        );
        assert_eq!(template("/blob/deadbeefdeadbeefcafe").0, "/blob/{hex}");
        assert_eq!(template("/s/aB3dE5fG7hI9jK1lM3nO5").0, "/s/{token}");
        assert_eq!(
            template("/api/v2/users/me").0,
            "/api/v2/users/me",
            "words and versions stay"
        );
        assert_eq!(template("/").0, "/");
        assert_eq!(
            template("/pets/{petId}").1,
            template("/pets/77").1,
            "a spec's placeholder matches"
        );
        assert_eq!(category_of("WSTG-ATHZ-04"), Some("ATHZ"));
        assert_eq!(category_of("spall/other"), None);
    }

    #[test]
    fn endpoints_count_who_touched_them_and_what_they_take() {
        let s = Surface::default();
        s.observe(&entry(
            "GET",
            "https://app.test/orders/1?expand=items",
            "2026-11-25T10:00:00Z",
            serde_json::json!({}),
        ));
        s.observe(&entry(
            "get",
            "https://app.test/orders/2",
            "2026-11-25T10:01:00Z",
            serde_json::json!({"status": 404}),
        ));
        s.observe(&entry(
            "POST",
            "https://app.test/orders",
            "2026-11-25T10:02:00Z",
            serde_json::json!({"req_headers": [["content-type", "application/json"]], "req_body": "{\"sku\":1,\"qty\":2}",
                "origin": "replay", "meta": {"actor": "agent", "task": "T-4"}}),
        ));
        let specs = vec![
            ("app.test".to_string(), "/orders/{orderId}".to_string()),
            ("app.test".to_string(), "/admin/users".to_string()),
        ];
        let eps = s.list(&specs);
        let get = eps.iter().find(|e| e.method == "GET").unwrap();
        assert_eq!(
            (get.template.as_str(), get.count, get.by_you),
            ("/orders/{id}", 2, 2)
        );
        assert_eq!(
            (get.statuses.clone(), get.params.clone()),
            (vec![200, 404], vec!["expand".to_string()])
        );
        assert_eq!(get.example.as_deref(), Some("x-2026-11-25T10:01:00Z"));
        assert!(get.documented, "the spec's /orders/{{orderId}}");
        let post = eps.iter().find(|e| e.method == "POST").unwrap();
        assert_eq!(
            (post.tasks.clone(), post.params.clone(), post.by_you),
            (
                vec!["T-4".to_string()],
                vec!["qty".to_string(), "sku".to_string()],
                0
            )
        );
        let any = eps.iter().find(|e| e.method == "ANY").unwrap();
        assert_eq!(
            (any.template.as_str(), any.count, any.documented),
            ("/admin/users", 0, true),
            "documented, untouched"
        );
        assert_eq!(eps.len(), 3);
    }
}

/// One coverage cell: found (a candidate under this category), or tested
/// (a coverage record), with who and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cell {
    /// `found` or `tested`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub how: Vec<String>,
}

/// A Surface row: the endpoint, where it stands in scope, and its
/// coverage by WSTG category.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub endpoint: Endpoint,
    pub scope: crate::gate::HostState,
    pub coverage: std::collections::BTreeMap<&'static str, Cell>,
}

/// The Surface: endpoints with scope state and coverage. `in_scope_only`
/// drops the rest (ad networks, CDNs, the noise of browsing).
pub fn view(
    endpoints: Vec<Endpoint>,
    scope: &crate::gate::Scope,
    candidates: &[crate::candidates::Candidate],
    coverage: &[crate::coverage::Record],
    in_scope_only: bool,
) -> Vec<Row> {
    use std::collections::BTreeMap;
    // Same endpoint: the same key, or a documented ANY row on the same
    // host and template.
    let matches = |ep: &Endpoint, k: &str| {
        if ep.key == k {
            return true;
        }
        let mut a = ep.key.splitn(3, '|');
        let mut b = k.splitn(3, '|');
        let (ah, am, ap) = (a.next(), a.next(), a.next());
        let (bh, bm, bp) = (b.next(), b.next(), b.next());
        ah == bh && ap == bp && (am == Some("ANY") || bm == Some("ANY"))
    };
    let found: Vec<(String, &'static str, String)> = candidates
        .iter()
        .filter(|c| !c.finding.is_suppressed())
        .filter_map(|c| {
            let (m, u) = c.finding.endpoint()?;
            let cat = category_of(&c.finding.rule_id)?;
            let (h, p) = split_url(u);
            Some((key(&h, m, &template(&p).1), cat, c.id.clone()))
        })
        .collect();
    endpoints
        .into_iter()
        .filter_map(|ep| {
            let state = scope.explain(&ep.host).state;
            if in_scope_only && state != crate::gate::HostState::In {
                return None;
            }
            let mut cov: BTreeMap<&'static str, Cell> = BTreeMap::new();
            for r in coverage.iter().filter(|r| matches(&ep, &r.key)) {
                let Some((cat, _)) = CATEGORIES.iter().find(|(c, _)| *c == r.category) else {
                    continue;
                };
                let cell = cov.entry(cat).or_insert(Cell {
                    state: "tested",
                    candidates: Vec::new(),
                    by: Vec::new(),
                    how: Vec::new(),
                });
                if !cell.by.contains(&r.by) {
                    cell.by.push(r.by.clone());
                }
                if !cell.how.contains(&r.how) {
                    cell.how.push(r.how.clone());
                }
            }
            for (k, cat, id) in &found {
                if matches(&ep, k) {
                    let cell = cov.entry(cat).or_insert(Cell {
                        state: "found",
                        candidates: Vec::new(),
                        by: Vec::new(),
                        how: Vec::new(),
                    });
                    cell.state = "found";
                    cell.candidates.push(id.clone());
                }
            }
            Some(Row {
                endpoint: ep,
                scope: state,
                coverage: cov,
            })
        })
        .collect()
}
