//! OpenAPI import (#40): read an OpenAPI 3.x or Swagger 2.0 file (JSON or
//! YAML) for its servers, which become proposed scope entries in the same
//! preview as a pasted list (#38), and its paths, kept on the engagement
//! for Surface later. Nothing is applied without the tester's click.
//!
//! The file is untrusted: size-capped, never fetched from or resolved
//! (`$ref`s, remote or local, are not followed), YAML aliases are
//! size-checked before loading (an alias bomb would expand exponentially),
//! and nesting is capped.

use crate::gate::Scope;
use crate::scope_list::{self, Item};
use serde::{Deserialize, Serialize};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;
use yaml_rust2::{Yaml, YamlLoader};

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Most nodes a document may expand to (aliases included).
const MAX_NODES: u64 = 2_000_000;
const MAX_DEPTH: usize = 64;
pub const MAX_PATHS: usize = 5000;

/// What an import keeps on the engagement (for Surface, later).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSpec {
    pub title: String,
    pub version: String,
    /// `openapi 3.1.0` or `swagger 2.0`.
    pub format: String,
    /// Server URLs, variables expanded to their defaults.
    pub servers: Vec<String>,
    /// Path templates, sorted.
    pub paths: Vec<String>,
    #[serde(default)]
    pub imported: String,
}

impl ApiSpec {
    pub fn validate(&self) -> Result<(), String> {
        let short = |s: &str| s.chars().count() <= 2048 && !s.chars().any(char::is_control);
        if !short(&self.title)
            || !short(&self.version)
            || !short(&self.format)
            || !short(&self.imported)
        {
            return Err("api spec: fields at most 2048 characters, one line".into());
        }
        if self.servers.len() > 100 || self.paths.len() > MAX_PATHS {
            return Err(format!(
                "api spec: at most 100 servers and {MAX_PATHS} paths"
            ));
        }
        if !self.servers.iter().chain(&self.paths).all(|s| short(s)) {
            return Err("api spec: servers and paths at most 2048 characters, one line".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Import {
    pub spec: ApiSpec,
    /// The proposed scope entries, as a pasted list would be previewed.
    pub items: Vec<Item>,
    pub add: Vec<String>,
    pub exclude: Vec<String>,
    /// Things the tester should know (relative servers, unexpanded variables).
    pub notes: Vec<String>,
}

/// Counts what a YAML document expands to before it's loaded.
#[derive(Default)]
struct Guard {
    /// Expanded size of each anchored node, by anchor id.
    sizes: std::collections::HashMap<usize, u64>,
    /// Open containers: (anchor id, nodes so far).
    stack: Vec<(usize, u64)>,
    total: u64,
    error: Option<String>,
}

impl Guard {
    fn add(&mut self, n: u64) {
        match self.stack.last_mut() {
            Some((_, c)) => *c = c.saturating_add(n),
            None => self.total = self.total.saturating_add(n),
        }
        let open: u64 = self.stack.iter().map(|(_, c)| *c).sum();
        if self.total.saturating_add(open) > MAX_NODES && self.error.is_none() {
            self.error = Some("the document expands to too many nodes (YAML aliases?)".into());
        }
    }
}

impl MarkedEventReceiver for Guard {
    fn on_event(&mut self, ev: Event, _mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match ev {
            Event::Scalar(_, _, aid, _) => {
                if aid > 0 {
                    self.sizes.insert(aid, 1);
                }
                self.add(1);
            }
            Event::SequenceStart(aid, _) | Event::MappingStart(aid, _) => {
                self.stack.push((aid, 1));
                if self.stack.len() > MAX_DEPTH {
                    self.error = Some(format!("nested deeper than {MAX_DEPTH} levels"));
                }
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some((aid, c)) = self.stack.pop() {
                    if aid > 0 {
                        self.sizes.insert(aid, c);
                    }
                    self.add(c);
                }
            }
            Event::Alias(aid) => {
                let n = self.sizes.get(&aid).copied().unwrap_or(1);
                self.add(n);
            }
            _ => {}
        }
    }
}

fn yaml_to_json(y: &Yaml, depth: usize) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    if depth > MAX_DEPTH {
        return Err(format!("nested deeper than {MAX_DEPTH} levels"));
    }
    Ok(match y {
        Yaml::Real(s) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::String(s.clone()), Value::Number),
        Yaml::Integer(i) => Value::from(*i),
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Boolean(b) => Value::Bool(*b),
        Yaml::Array(a) => Value::Array(
            a.iter()
                .map(|x| yaml_to_json(x, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Hash(h) => {
            let mut m = serde_json::Map::new();
            for (k, v) in h {
                let key = match k {
                    Yaml::String(s) | Yaml::Real(s) => s.clone(),
                    Yaml::Integer(i) => i.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    _ => continue,
                };
                m.insert(key, yaml_to_json(v, depth + 1)?);
            }
            Value::Object(m)
        }
        Yaml::Null | Yaml::BadValue | Yaml::Alias(_) => Value::Null,
    })
}

/// The document as JSON: JSON if it parses as JSON, else YAML (guarded).
fn load(text: &str) -> Result<serde_json::Value, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("at most {} MiB", MAX_BYTES / (1024 * 1024)));
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"));
    }
    let mut guard = Guard::default();
    Parser::new_from_str(text)
        .load(&mut guard, false)
        .map_err(|e| format!("not valid YAML: {e}"))?;
    if let Some(e) = guard.error {
        return Err(e);
    }
    let docs = YamlLoader::load_from_str(text).map_err(|e| format!("not valid YAML: {e}"))?;
    let doc = docs.first().ok_or("empty document")?;
    yaml_to_json(doc, 0)
}

/// `{var}` placeholders replaced by their default (or first enum value).
fn expand(url: &str, vars: Option<&serde_json::Value>, notes: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = url;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let name = &rest[i + 1..i + j];
        let var = vars.and_then(|v| v.get(name));
        let value = var
            .and_then(|v| v.get("default"))
            .and_then(|d| d.as_str())
            .or_else(|| {
                var.and_then(|v| v.get("enum"))
                    .and_then(|e| e.get(0))
                    .and_then(|d| d.as_str())
            });
        match value {
            Some(v) => out.push_str(v),
            None => {
                notes.push(format!(
                    "server variable {{{name}}} has no default: left as is"
                ));
                out.push_str(&rest[i..=i + j]);
            }
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn str_at<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str()
}

/// Read a spec against the current scope.
pub fn import(text: &str, current: &Scope, now: &str) -> Result<Import, String> {
    let doc = load(text)?;
    let mut notes = Vec::new();
    let (format, servers) = if let Some(v) = str_at(&doc, &["openapi"]) {
        let mut servers = Vec::new();
        // Top-level servers, then any declared on paths or operations.
        let mut lists = vec![doc.get("servers")];
        if let Some(paths) = doc.get("paths").and_then(|p| p.as_object()) {
            for item in paths.values() {
                lists.push(item.get("servers"));
                if let Some(ops) = item.as_object() {
                    lists.extend(ops.values().map(|op| op.get("servers")));
                }
            }
        }
        for s in lists
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_array())
            .flatten()
        {
            if let Some(url) = s.get("url").and_then(|u| u.as_str()) {
                let url = expand(url, s.get("variables"), &mut notes);
                if !servers.contains(&url) {
                    servers.push(url);
                }
            }
        }
        if servers.is_empty() {
            notes.push("no servers listed: the spec doesn't say where the API lives".into());
        }
        (format!("openapi {v}"), servers)
    } else if let Some(v) = str_at(&doc, &["swagger"]) {
        let base = str_at(&doc, &["basePath"]).unwrap_or("");
        let servers = match str_at(&doc, &["host"]) {
            Some(host) => {
                let schemes: Vec<&str> = doc
                    .get("schemes")
                    .and_then(|s| s.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                    .unwrap_or_default();
                let scheme = schemes.first().copied().unwrap_or("https");
                vec![format!("{scheme}://{host}{base}")]
            }
            None => {
                notes.push("no host: the spec is served from the API's own host".into());
                Vec::new()
            }
        };
        (format!("swagger {v}"), servers)
    } else {
        return Err("not an OpenAPI or Swagger document (no `openapi` or `swagger` field)".into());
    };

    let mut paths: Vec<String> = doc
        .get("paths")
        .and_then(|p| p.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if paths.len() > MAX_PATHS {
        notes.push(format!("{} paths; kept the first {MAX_PATHS}", paths.len()));
        paths.truncate(MAX_PATHS);
    }

    // Absolute servers go through the pasted-list reader, one per line;
    // relative ones have no host to scope.
    let absolute: Vec<&String> = servers.iter().filter(|s| s.contains("://")).collect();
    for s in servers.iter().filter(|s| !s.contains("://")) {
        notes.push(format!(
            "server {s:?} is relative: its host is wherever the spec is served from"
        ));
    }
    let list = absolute
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let items = scope_list::parse(&list, current)?;
    let (add, exclude) = scope_list::to_apply(&items);

    let one_line = |s: &str| {
        s.chars()
            .filter(|c| !c.is_control())
            .take(2048)
            .collect::<String>()
    };
    let spec = ApiSpec {
        title: one_line(str_at(&doc, &["info", "title"]).unwrap_or("untitled")),
        version: one_line(str_at(&doc, &["info", "version"]).unwrap_or("")),
        format: one_line(&format),
        servers: servers.iter().take(100).map(|s| one_line(s)).collect(),
        paths: paths.iter().map(|p| one_line(p)).collect(),
        imported: now.to_string(),
    };
    Ok(Import {
        spec,
        items,
        add,
        exclude,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Import {
        import(
            text,
            &Scope::parse("api.acme.test").unwrap(),
            "2026-11-22T00:00:00Z",
        )
        .unwrap()
    }

    #[test]
    fn openapi_3_yaml_with_server_variables() {
        let i = run(r#"
openapi: 3.1.0
info: { title: ACME API, version: "2.4" }
servers:
  - url: https://{env}.acme.test/v2
    variables:
      env: { default: staging, enum: [staging, prod] }
  - url: https://api.acme.test
  - url: /relative
paths:
  /users: { get: {} }
  /users/{id}:
    servers: [{ url: "https://files.acme.test" }]
    get: {}
"#);
        assert_eq!(i.spec.title, "ACME API");
        assert_eq!(i.spec.format, "openapi 3.1.0");
        assert_eq!(
            i.spec.servers,
            [
                "https://staging.acme.test/v2",
                "https://api.acme.test",
                "/relative",
                "https://files.acme.test"
            ]
        );
        assert_eq!(i.spec.paths, ["/users", "/users/{id}"]);
        assert_eq!(i.add, ["staging.acme.test", "files.acme.test"]);
        assert!(
            i.items
                .iter()
                .any(|x| x.entry.as_deref() == Some("api.acme.test") && x.present),
            "already in scope"
        );
        assert!(
            i.notes
                .iter()
                .any(|n| n.contains("\"/relative\" is relative")),
            "{:?}",
            i.notes
        );
    }

    #[test]
    fn openapi_3_json_and_swagger_2() {
        let i = run(
            r#"{"openapi":"3.0.3","info":{"title":"J","version":"1"},"servers":[{"url":"https://{tenant}.acme.test"}],"paths":{}}"#,
        );
        assert!(i
            .notes
            .iter()
            .any(|n| n.contains("{tenant} has no default")));
        assert!(
            i.items[0].error.is_some(),
            "an unexpanded variable isn't a host: {:?}",
            i.items
        );
        let s = run("swagger: '2.0'\ninfo: {title: Old, version: '1'}\nhost: legacy.acme.test:8443\nbasePath: /api\nschemes: [http]\npaths:\n  /a: {}\n");
        assert_eq!(s.spec.servers, ["http://legacy.acme.test:8443/api"]);
        assert_eq!(s.add, ["legacy.acme.test"]);
        assert_eq!(s.spec.format, "swagger 2.0");
        let none = run("swagger: '2.0'\ninfo: {title: X, version: '1'}\npaths: {}\n");
        assert!(none.add.is_empty() && none.notes[0].contains("no host"));
    }

    #[test]
    fn refs_are_never_followed() {
        let i = run(
            r#"{"openapi":"3.0.0","info":{"title":"R","version":"1"},"servers":[{"$ref":"https://evil.test/servers.json"}],"paths":{"/x":{"$ref":"http://evil.test/p"}}}"#,
        );
        assert!(i.add.is_empty());
        assert_eq!(i.spec.paths, ["/x"]);
    }

    #[test]
    fn hostile_documents_are_refused() {
        // A billion-laughs alias bomb: refused before it's expanded.
        let mut bomb = String::from("a: &a [x, x, x, x, x, x, x, x, x, x]\n");
        for (n, prev) in ["b", "c", "d", "e", "f", "g", "h", "i"]
            .iter()
            .zip(["a", "b", "c", "d", "e", "f", "g", "h"])
        {
            bomb.push_str(&format!("{n}: &{n} [*{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}]\n"));
        }
        let err = import(&bomb, &Scope::default(), "").unwrap_err();
        assert!(err.contains("too many nodes"), "{err}");
        let deep = format!("openapi: 3.0.0\nx: {}{}", "[".repeat(200), "]".repeat(200));
        assert!(import(&deep, &Scope::default(), "")
            .unwrap_err()
            .contains("nested deeper"));
        assert!(import(&"x".repeat(MAX_BYTES + 1), &Scope::default(), "").is_err());
        assert!(import("title: not a spec\n", &Scope::default(), "")
            .unwrap_err()
            .contains("not an OpenAPI"));
        assert!(import("{ broken", &Scope::default(), "")
            .unwrap_err()
            .contains("not valid JSON"));
        assert!(import("a: [unclosed\n", &Scope::default(), "")
            .unwrap_err()
            .contains("not valid YAML"));
        // A small, legitimate use of anchors still loads.
        let ok = "openapi: 3.0.0\ninfo: {title: A, version: '1'}\ns: &s {url: 'https://a.acme.test'}\nservers: [*s]\npaths: {}\n";
        assert_eq!(
            import(ok, &Scope::default(), "").unwrap().add,
            ["a.acme.test"]
        );
    }

    #[test]
    fn specs_are_validated_for_storage() {
        let mut s = run("openapi: 3.0.0\ninfo: {title: T, version: '1'}\npaths: {}\n").spec;
        s.validate().unwrap();
        s.paths = vec!["/a\nb".into()];
        assert!(s.validate().is_err());
    }
}
