//! Requests that look destructive (#85). The gate classes a request as
//! safe or unsafe by its method, but plenty of apps change state on GET:
//! `GET /api/users/5/delete`, `GET /admin/purge-cache?all=1`, a query
//! carrying `'; DROP TABLE users--`. This marks such a request so the gate
//! treats it as unsafe (asked for, or refused for a role without unsafe).
//!
//! It only adds friction, never grants, and never denies on its own: a miss
//! falls back to the method, a false positive costs an approval. It reads
//! only what is sent to the target (path, query, text body), never an
//! agent's notes or reports. The patterns are reviewed data in
//! `src/content/destructive.txt`; a corpus test keeps false positives down.

use std::sync::OnceLock;

const PATTERNS: &str = include_str!("content/destructive.txt");
/// Body text looked at, from the start.
const MAX_BODY: usize = 64 * 1024;

struct Patterns {
    words: Vec<String>,
    actions: Vec<String>,
    phrases: Vec<Vec<String>>,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let mut p = Patterns {
            words: Vec::new(),
            actions: Vec::new(),
            phrases: Vec::new(),
        };
        for line in PATTERNS.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once(' ') {
                Some(("word", w)) => p.words.push(w.trim().to_string()),
                Some(("action", a)) => p.actions.push(a.trim().to_string()),
                Some(("phrase", ph)) => p.phrases.push(words(ph)),
                _ => {}
            }
        }
        p
    })
}

/// Lowercase words: split on anything not a letter or digit, and on
/// camelCase (`deleteUser` is `delete user`).
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
            cur.extend(c.to_lowercase());
        } else {
            prev_lower = false;
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `%XX` and (in a query) `+` decoded; bad escapes kept as they are.
fn decode(s: &str, plus: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' if plus => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn has_phrase(ws: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && ws.windows(phrase.len()).any(|w| w == phrase)
}

/// What made a request look destructive: where, and the pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// `path`, `query` or `body`.
    pub place: &'static str,
    pub pattern: String,
}

impl std::fmt::Display for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} in the {}", self.pattern, self.place)
    }
}

/// Whether a request to `url` with this text `body` looks destructive.
pub fn check(url: &str, body: Option<&str>) -> Option<Match> {
    let p = patterns();
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path_q = rest.find('/').map_or("", |i| &rest[i..]);
    let path_q = path_q.split('#').next().unwrap_or("");
    let (path, query) = path_q.split_once('?').unwrap_or((path_q, ""));
    let word_hit = |ws: &[String]| p.words.iter().find(|w| ws.contains(w)).cloned();

    if let Some(w) = word_hit(&words(&decode(path, false))) {
        return Some(Match {
            place: "path",
            pattern: format!("\"{w}\""),
        });
    }
    let mut values = Vec::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (decode(k, true), decode(v, true));
        if let Some(w) = word_hit(&words(&k)) {
            return Some(Match {
                place: "query",
                pattern: format!("\"{w}\" (a parameter name)"),
            });
        }
        if p.actions.iter().any(|a| a.eq_ignore_ascii_case(k.trim())) {
            if let Some(w) = word_hit(&words(&v)) {
                return Some(Match {
                    place: "query",
                    pattern: format!("{}={w}", k.trim()),
                });
            }
        }
        values.push(v);
    }
    let qw = words(&values.join(" "));
    if let Some(ph) = p.phrases.iter().find(|ph| has_phrase(&qw, ph)) {
        return Some(Match {
            place: "query",
            pattern: format!("\"{}\"", ph.join(" ")),
        });
    }
    if let Some(b) = body {
        let cut: String = b.chars().take(MAX_BODY).collect();
        let bw = words(&cut);
        if let Some(ph) = p.phrases.iter().find(|ph| has_phrase(&bw, ph)) {
            return Some(Match {
                place: "body",
                pattern: format!("\"{}\"", ph.join(" ")),
            });
        }
        // An action-like field in a JSON or form body: {"action": "delete"}.
        for a in &p.actions {
            for w in &p.words {
                if has_phrase(&bw, &[a.clone(), w.clone()]) {
                    return Some(Match {
                        place: "body",
                        pattern: format!("{a}={w}"),
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_looking_requests_are_caught_with_what_matched() {
        let cases = [
            (
                "https://app.test/api/users/5/delete",
                None,
                "path",
                "\"delete\"",
            ),
            (
                "https://app.test/admin/purge-cache?all=1",
                None,
                "path",
                "\"purge\"",
            ),
            (
                "https://app.test/api/deleteUser?id=5",
                None,
                "path",
                "\"delete\"",
            ),
            (
                "https://app.test/api/users/5?_method=x&remove_all=1",
                None,
                "query",
                "\"remove\" (a parameter name)",
            ),
            (
                "https://app.test/api?action=delete&id=5",
                None,
                "query",
                "action=delete",
            ),
            ("https://app.test/api?op=Wipe", None, "query", "op=wipe"),
            (
                "https://app.test/search?q=x%27%3B+DROP+TABLE+users--",
                None,
                "query",
                "\"drop table\"",
            ),
            (
                "https://app.test/admin?step=factory-reset",
                None,
                "query",
                "\"factory reset\"",
            ),
            (
                "https://app.test/api/items",
                Some(r#"{"filter":"1; DELETE FROM items"}"#),
                "body",
                "\"delete from\"",
            ),
            (
                "https://app.test/rpc",
                Some(r#"{"action":"delete","id":5}"#),
                "body",
                "action=delete",
            ),
            (
                "https://app.test/mongo",
                Some("db.dropDatabase()"),
                "body",
                "\"drop database\"",
            ),
            (
                "https://app.test/mongo",
                Some("db.dropdatabase()"),
                "body",
                "\"dropdatabase\"",
            ),
            (
                "https://app.test/cache",
                Some("FLUSHALL"),
                "body",
                "\"flushall\"",
            ),
        ];
        for (url, body, place, pattern) in cases {
            let m = check(url, body).unwrap_or_else(|| panic!("{url} {body:?}"));
            assert_eq!((m.place, m.pattern.as_str()), (place, pattern), "{url}");
        }
    }

    /// Ordinary requests that must not ask: the false-positive corpus.
    #[test]
    fn ordinary_requests_pass() {
        let ok = [
            ("https://app.test/search?q=how+to+delete+my+account", None),
            ("https://app.test/help/deleting-your-account", None),
            ("https://app.test/password-reset", None),
            ("https://app.test/account/reset-password?token=abc", None),
            ("https://app.test/api/drops/latest", None),
            ("https://app.test/ui/dropzone.js", None),
            ("https://app.test/api/dropdown-options", None),
            (
                "https://app.test/api/products?sort=-created&status=removed",
                None,
            ),
            ("https://app.test/static/app.js?v=3", None),
            ("https://app.test/api/users/5", None),
            ("https://app.test/blog/how-we-dropped-latency", None),
            ("https://app.test/docs/sql/select", None),
            (
                "https://app.test/api/comments",
                Some(r#"{"text":"please delete this spam, thanks"}"#),
            ),
            (
                "https://app.test/api/notes",
                Some(r#"{"body":"drop by the office"}"#),
            ),
            (
                "https://app.test/api/report",
                Some(r#"{"title":"Truncated output"}"#),
            ),
            ("https://app.test/%zz/odd%", None),
            ("https://app.test/caf%C3%A9/%€x", None),
        ];
        for (url, body) in ok {
            assert_eq!(check(url, body), None, "{url} {body:?}");
        }
    }

    #[test]
    fn words_split_on_separators_and_camel_case() {
        assert_eq!(
            words("deleteUserNow/remove_all-x.y"),
            ["delete", "user", "now", "remove", "all", "x", "y"]
        );
        assert_eq!(decode("a%20b+c", true), "a b c");
        assert_eq!(decode("%zz%", false), "%zz%");
        assert!(
            patterns().words.contains(&"delete".to_string())
                && !patterns().words.contains(&"drop".to_string())
        );
    }
}
