//! `sweep` and checks (#75): many requests from a list against one
//! in-scope base URL, run server-side and summarized, so an agent doesn't
//! make 200 tool calls and the tester doesn't read 200 exchanges.
//!
//! A **check** is data (docs/architecture.md, *Building blocks*): a name,
//! the WSTG rule a hit is about, a method (GET or HEAD), paths relative to
//! the base (never a host), and simple matchers (status, differs from the
//! soft-404 baseline, contains). It sends nothing by itself. spall ships a
//! vetted set in `src/content/checks/`; an engagement can add its own, as
//! the tester's `policy` change.
//!
//! **Sweep** is the primitive that runs one: every request goes through the
//! same gate as a fetch (scope, the never-test list, the RoE, the role,
//! the task's budget and rate, destructive-looking requests), is recorded
//! in taphttp and attributed. Bounded by the RoE's `sweep_max`; a soft-404
//! baseline first; stops on halt or the e-stop. This module parses and
//! judges; `Replayer::sweep` runs.

use serde::Serialize;

/// Paths one check may hold.
pub const MAX_PATHS: usize = 500;
/// Engagement-local checks one engagement may hold.
pub const MAX_LOCAL: usize = 50;
/// Characters of one check's text.
pub const MAX_TEXT: usize = 32 * 1024;

const BUILT_IN: [&str; 3] = [
    include_str!("content/checks/metafiles.check"),
    include_str!("content/checks/backup-files.check"),
    include_str!("content/checks/admin-interfaces.check"),
];

/// Where a check comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    BuiltIn,
    Engagement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub rule: String,
    pub title: String,
    pub method: String,
    /// A hit's status is one of these (empty: any).
    pub statuses: Vec<u16>,
    /// A hit differs from the soft-404 baseline (status or size).
    pub differs: bool,
    /// A hit's body contains each of these (case-insensitive).
    pub contains: Vec<String>,
    pub paths: Vec<String>,
    pub source: Source,
}

fn path_ok(p: &str) -> Result<(), String> {
    if !p.starts_with('/') || p.starts_with("//") {
        return Err(format!(
            "path {p:?}: starts with one / (relative to the base, never a host)"
        ));
    }
    if p.len() > 512 || p.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!(
            "path {p:?}: at most 512 characters, no spaces or control characters"
        ));
    }
    if p.contains("://") || p.contains('#') || p.contains('\\') {
        return Err(format!("path {p:?}: a path, not a URL"));
    }
    if p.split('/')
        .any(|s| s == ".." || s.eq_ignore_ascii_case("%2e%2e"))
    {
        return Err(format!("path {p:?}: no .. segments"));
    }
    Ok(())
}

/// Parse a check from its text (see the files in `src/content/checks/`).
pub fn parse(text: &str, source: Source) -> Result<Check, String> {
    if text.len() > MAX_TEXT {
        return Err(format!("a check is at most {MAX_TEXT} characters"));
    }
    let mut c = Check {
        name: String::new(),
        rule: String::new(),
        title: String::new(),
        method: "GET".into(),
        statuses: Vec::new(),
        differs: false,
        contains: Vec::new(),
        paths: Vec::new(),
        source,
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |e: String| format!("line {}: {e}", n + 1);
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match key {
            "name" => {
                if !(1..=48).contains(&rest.len())
                    || !rest
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(at("name: lowercase letters, digits and -".into()));
                }
                c.name = rest.into();
            }
            "rule" => {
                let r = crate::rules::find(rest)
                    .ok_or_else(|| at(format!("rule {rest:?} isn't in the catalog (see rules)")))?;
                c.rule = r.id.into();
            }
            "title" => c.title = rest.chars().take(120).collect(),
            "method" => {
                let m = rest.to_ascii_uppercase();
                if m != "GET" && m != "HEAD" {
                    return Err(at(
                        "method: GET or HEAD (a check never changes the target)".into()
                    ));
                }
                c.method = m;
            }
            "hit" => {
                let (what, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                match what {
                    "status" => {
                        for s in args.split_whitespace() {
                            let v: u16 = s
                                .parse()
                                .ok()
                                .filter(|v| (100..600).contains(v))
                                .ok_or_else(|| at(format!("status {s:?}: 100-599")))?;
                            c.statuses.push(v);
                        }
                    }
                    "differs" => c.differs = true,
                    "contains" if !args.trim().is_empty() => {
                        c.contains.push(args.trim().to_lowercase())
                    }
                    _ => {
                        return Err(at(format!(
                            "hit {what:?}: status <codes>, differs, or contains <text>"
                        )))
                    }
                }
            }
            "path" => {
                path_ok(rest).map_err(at)?;
                if !c.paths.iter().any(|p| p == rest) {
                    c.paths.push(rest.into());
                }
            }
            other => {
                return Err(at(format!(
                    "{other:?}: name, rule, title, method, hit or path"
                )))
            }
        }
    }
    if c.name.is_empty() || c.rule.is_empty() {
        return Err("a check needs a name and a rule".into());
    }
    if c.paths.is_empty() || c.paths.len() > MAX_PATHS {
        return Err(format!("a check has 1 to {MAX_PATHS} paths"));
    }
    if c.statuses.is_empty() && !c.differs && c.contains.is_empty() {
        return Err("a check needs at least one hit matcher (status, differs or contains)".into());
    }
    if c.title.is_empty() {
        c.title = c.name.clone();
    }
    Ok(c)
}

/// The vetted checks spall ships.
pub fn built_in() -> Vec<Check> {
    BUILT_IN
        .iter()
        .filter_map(|t| parse(t, Source::BuiltIn).ok())
        .collect()
}

/// What a nonexistent path answered: the soft-404 baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Baseline {
    pub status: Option<u16>,
    pub size: usize,
}

/// Whether an answer differs from the baseline: another status, or a size
/// off by more than 64 bytes and 5% (pages that echo the path vary a bit).
pub fn differs(b: &Baseline, status: Option<u16>, size: usize) -> bool {
    if status != b.status {
        return true;
    }
    let delta = size.abs_diff(b.size);
    delta > 64 && delta * 20 > b.size.max(size)
}

/// Whether an answer is a hit for `check`.
pub fn is_hit(
    check: &Check,
    base: &Baseline,
    status: Option<u16>,
    size: usize,
    body: Option<&str>,
) -> bool {
    if !check.statuses.is_empty() && !status.is_some_and(|s| check.statuses.contains(&s)) {
        return false;
    }
    if check.differs && !differs(base, status, size) {
        return false;
    }
    if !check.contains.is_empty() {
        let b = body.unwrap_or("").to_lowercase();
        if !check.contains.iter().all(|c| b.contains(c.as_str())) {
            return false;
        }
    }
    true
}

/// `base` + `path`: the base's own path kept, one slash between.
pub fn join(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// A hit, as the summary lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub path: String,
    pub exchange: String,
    pub status: Option<u16>,
    pub size: usize,
}

/// What a sweep did.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub check: String,
    pub rule: String,
    pub base: String,
    pub planned: usize,
    /// Requests sent, the baseline included.
    pub sent: usize,
    pub baseline: Option<Baseline>,
    pub hits: Vec<Hit>,
    pub misses: usize,
    /// Paths not sent, and why (refused, needs confirmation, error).
    pub skipped: Vec<(String, String)>,
    /// Why it stopped early, if it did.
    pub stopped: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_checks_parse_and_name_catalog_rules() {
        let all = built_in();
        assert_eq!(all.len(), BUILT_IN.len(), "every shipped check parses");
        for c in &all {
            assert!(crate::rules::find(&c.rule).is_some(), "{}", c.name);
            assert!(c.method == "GET" && !c.paths.is_empty(), "{}", c.name);
            for p in &c.paths {
                path_ok(p).unwrap();
            }
        }
        let names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["metafiles", "backup-files", "admin-interfaces"]);
    }

    #[test]
    fn checks_are_bounded_data() {
        let ok =
            "name x\nrule WSTG-CONF-04\nhit status 200\nhit contains Index Of\npath /a\npath /a\n";
        let c = parse(ok, Source::Engagement).unwrap();
        assert_eq!((c.paths.len(), c.contains[0].as_str()), (1, "index of"));
        for (bad, why) in [
            (
                "name x\nrule WSTG-CONF-04\nhit differs\npath https://evil.test/x",
                "never a host",
            ),
            (
                "name x\nrule WSTG-CONF-04\nhit differs\npath //evil.test/x",
                "never a host",
            ),
            ("name x\nrule WSTG-CONF-04\nhit differs\npath /a/../b", ".."),
            (
                "name x\nrule WSTG-CONF-04\nhit differs\nmethod POST\npath /a",
                "GET or HEAD",
            ),
            ("name x\nrule WSTG-NOPE-01\nhit differs\npath /a", "catalog"),
            ("name x\nrule WSTG-CONF-04\npath /a", "hit matcher"),
            (
                "name X!\nrule WSTG-CONF-04\nhit differs\npath /a",
                "lowercase",
            ),
            (
                "name x\nrule WSTG-CONF-04\nhit differs\nexec rm -rf /\npath /a",
                "name, rule",
            ),
            ("rule WSTG-CONF-04\nhit differs\npath /a", "name and a rule"),
        ] {
            let e = parse(bad, Source::Engagement).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    #[test]
    fn hits_are_judged_against_the_soft_404_baseline() {
        let c = parse(
            "name x\nrule WSTG-CONF-05\nhit status 200 401\nhit differs\npath /a",
            Source::BuiltIn,
        )
        .unwrap();
        // A site that answers 200 to everything with the same page.
        let soft = Baseline {
            status: Some(200),
            size: 5000,
        };
        assert!(
            !is_hit(&c, &soft, Some(200), 5010, None),
            "the same not-found page"
        );
        assert!(is_hit(&c, &soft, Some(200), 812, None), "a different page");
        assert!(is_hit(&c, &soft, Some(401), 5000, None), "a login wall");
        assert!(!is_hit(&c, &soft, Some(404), 100, None), "not a hit status");
        let real404 = Baseline {
            status: Some(404),
            size: 150,
        };
        assert!(is_hit(&c, &real404, Some(200), 150, None));
        let k = parse(
            "name y\nrule WSTG-CONF-04\nhit contains ref:\npath /.git/HEAD",
            Source::BuiltIn,
        )
        .unwrap();
        assert!(is_hit(
            &k,
            &real404,
            Some(200),
            23,
            Some("ref: refs/heads/main\n")
        ));
        assert!(!is_hit(&k, &real404, Some(200), 23, Some("<html>")));
        assert_eq!(
            join("https://app.test/shop/", "/admin"),
            "https://app.test/shop/admin"
        );
    }
}
