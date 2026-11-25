//! Scope lists (#38): text pasted from a statement of work or an email,
//! read one entry per line, and previewed before anything changes. The
//! OpenAPI import (#40) produces the same preview. Parsing is the scope
//! engine's own (`gate::Scope`), never a second copy in the UI.

use crate::gate::Scope;
use serde::Serialize;

/// Most text accepted, and most lines read.
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_LINES: usize = 5000;

/// What one entry on a line becomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    /// 1-based line number in the pasted text.
    pub line: usize,
    /// The piece of the line this came from, trimmed.
    pub source: String,
    /// The scope entry it becomes (normal form, without `!`), if valid.
    pub entry: Option<String>,
    pub exclude: bool,
    pub error: Option<String>,
    /// Things worth knowing: a dropped port or path, a broad entry, already there.
    pub notes: Vec<String>,
    /// Already in the scope (same list): nothing to do.
    pub present: bool,
    /// Too wide to add without the override (#83); the note says why.
    pub wide: bool,
}

/// Why an in-scope entry is too wide to add without the override (#83),
/// if it is: the gate's own check. Exclusions only narrow, so never.
pub fn broad(entry: &str) -> Option<String> {
    if entry.starts_with('!') {
        return None;
    }
    crate::gate::too_wide(entry).ok().flatten()
}

/// Strip a list marker: `- `, `* `, `• `, `1. `, `2) `.
fn unbullet(s: &str) -> &str {
    let t = s.trim_start();
    for m in ["- ", "* ", "• ", "+ ", "– "] {
        if let Some(rest) = t.strip_prefix(m) {
            return rest.trim_start();
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits < 4 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return r.trim_start();
        }
    }
    t
}

/// Drop parenthesized asides: `pay.acme.test (third party)`.
fn without_asides(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// A section header like "Out of scope:" or "In scope:" switches mode.
fn header(line: &str) -> Option<bool> {
    let l = line.trim().trim_end_matches(':').to_ascii_lowercase();
    if !line.trim().ends_with(':') {
        return None;
    }
    if l.contains("out of scope")
        || l.contains("out-of-scope")
        || l.contains("exclu")
        || l.contains("do not test")
    {
        Some(true)
    } else if l.contains("in scope") || l.contains("in-scope") || l.contains("targets") {
        Some(false)
    } else {
        None
    }
}

/// A URL or `host:port` down to its host; notes say what was dropped.
fn host_of(piece: &str, notes: &mut Vec<String>) -> String {
    let mut s = piece.trim();
    if let Some((scheme, rest)) = s.split_once("://") {
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
        {
            s = rest;
            notes.push(format!("{scheme}:// dropped: scope is by host"));
        }
    }
    if let Some(i) = s.find(['/', '?', '#']) {
        // A CIDR's `/` stays: only cut a path after a hostname.
        let (head, tail) = s.split_at(i);
        let is_cidr = tail.starts_with('/')
            && tail[1..].bytes().all(|b| b.is_ascii_digit())
            && !tail[1..].is_empty();
        if !is_cidr {
            if tail != "/" {
                notes.push(format!("path {tail:?} dropped: scope is by host"));
            }
            s = head;
        }
    }
    if let Some((_, host)) = s.rsplit_once('@') {
        s = host;
        notes.push("credentials dropped".into());
    }
    // A port: `host:443` or `[v6]:443`, never part of a bare IPv6 address.
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((inside, after)) = rest.split_once(']') {
            if let Some(port) = after.strip_prefix(':') {
                notes.push(format!("port {port} ignored: scope matches every port"));
            }
            return format!("[{inside}]");
        }
    } else if let Some((name, port)) = s.rsplit_once(':') {
        if !name.contains(':') && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            notes.push(format!("port {port} ignored: scope matches every port"));
            return name.to_string();
        }
    }
    s.to_string()
}

/// Read a pasted list against the current scope. Every non-empty piece
/// gets an item: an entry, or an error saying why not.
pub fn parse(text: &str, current: &Scope) -> Result<Vec<Item>, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("at most {} KiB of text", MAX_BYTES / 1024));
    }
    let existing = current.entries();
    let mut out: Vec<Item> = Vec::new();
    let mut excluding = false;
    for (i, raw) in text.lines().enumerate() {
        if i >= MAX_LINES {
            return Err(format!("at most {MAX_LINES} lines"));
        }
        let line = raw.split_once('#').map_or(raw, |(before, _)| before);
        if let Some(mode) = header(line) {
            excluding = mode;
            continue;
        }
        let line = without_asides(unbullet(line));
        let pieces: Vec<&str> = line
            .split([',', ';', '\t', ' '])
            .map(|p| p.trim().trim_matches(['"', '\'', '`', '<', '>', '(', ')']))
            .filter(|p| !p.is_empty())
            .collect();
        let alone = pieces.len() == 1;
        for piece in pieces {
            let mut notes = Vec::new();
            let (exclude, body) = match piece.strip_prefix('!').or_else(|| piece.strip_prefix('-'))
            {
                Some(rest) => (true, rest),
                None => (excluding, piece),
            };
            let host = host_of(body, &mut notes);
            let mut probe = Scope::default();
            let parsed = if host == "*" {
                Err(
                    "`*` (every host) isn't a list entry: use `scope all on` for this run"
                        .to_string(),
                )
            } else if exclude {
                probe.exclude(&host)
            } else {
                // Width is a note here (and `wide` below): applying it
                // needs the override.
                probe.add_wide(&host)
            };
            let (mut entry, mut error) = match parsed {
                Ok(_) => {
                    let e = probe
                        .entries()
                        .into_iter()
                        .next()
                        .map(|e| e.trim_start_matches('!').to_string());
                    (e, None)
                }
                Err(e) => (
                    None,
                    Some(e.replace(&format!("scope entry {host:?}: "), "")),
                ),
            };
            // A dot-less name in a line of words is prose, not a host.
            let single = entry
                .as_ref()
                .is_some_and(|e| !e.contains(['.', ':']) && e.parse::<std::net::IpAddr>().is_err());
            if single && !alone {
                entry = None;
                error = Some("a word, not a host (a single-label name among other words)".into());
            } else if single {
                notes.push("single-label name: only matches that exact name".into());
            }
            let mut wide = false;
            if let (Some(e), false) = (&entry, exclude) {
                if let Some(w) = broad(e) {
                    notes.push(w);
                    wide = true;
                }
            }
            let key = entry
                .as_ref()
                .map(|e| if exclude { format!("!{e}") } else { e.clone() });
            let present = key.as_ref().is_some_and(|k| existing.contains(k));
            let repeated = key.as_ref().is_some_and(|k| {
                out.iter().any(|o| {
                    o.entry
                        .as_ref()
                        .map(|e| {
                            if o.exclude {
                                format!("!{e}")
                            } else {
                                e.clone()
                            }
                        })
                        .as_ref()
                        == Some(k)
                })
            });
            if present {
                notes.push("already in the scope".into());
            } else if repeated {
                notes.push("listed earlier".into());
            }
            out.push(Item {
                line: i + 1,
                source: piece.to_string(),
                entry,
                exclude,
                error,
                notes,
                present: present || repeated,
                wide,
            });
        }
    }
    Ok(out)
}

/// The entries a preview would apply: valid and not already there.
pub fn to_apply(items: &[Item]) -> (Vec<String>, Vec<String>) {
    let mut add = Vec::new();
    let mut exclude = Vec::new();
    for it in items {
        if let (Some(e), false) = (&it.entry, it.present) {
            if it.exclude {
                exclude.push(e.clone());
            } else {
                add.push(e.clone());
            }
        }
    }
    (add, exclude)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(text: &str) -> Vec<Item> {
        parse(text, &Scope::parse("app.acme.test").unwrap()).unwrap()
    }

    fn summary(text: &str) -> Vec<String> {
        items(text)
            .iter()
            .map(|i| match (&i.entry, &i.error) {
                (Some(e), _) => format!(
                    "{}{}{e}{}",
                    i.line,
                    if i.exclude { " !" } else { " " },
                    if i.present { " (present)" } else { "" }
                ),
                (None, Some(err)) => format!("{} error: {err}", i.line),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn a_statement_of_work_paste() {
        let text = "In scope:\n\
            - https://www.acme.test/login\n\
            * *.api.acme.test, 10.20.0.0/16\n\
            1. app.acme.test:8443   # already there\n\
            \n\
            Out of scope:\n\
            • pay.acme.test (third party)\n\
            2) sso.acme.test\n\
            In scope:\n\
            !cdn.acme.test\n\
            bücher.test\n";
        assert_eq!(
            summary(text),
            [
                "2 www.acme.test",
                "3 *.api.acme.test",
                "3 10.20.0.0/16",
                "4 app.acme.test (present)",
                "7 !pay.acme.test",
                "8 !sso.acme.test",
                "10 !cdn.acme.test",
                "11 xn--bcher-kva.test",
            ]
        );
    }

    #[test]
    fn urls_and_ports_say_what_was_dropped() {
        let it = &items("https://user:pw@www.acme.test:8443/admin?x=1")[0];
        assert_eq!(it.entry.as_deref(), Some("www.acme.test"));
        let notes = it.notes.join(" | ");
        for want in [
            "https:// dropped",
            "path \"/admin?x=1\" dropped",
            "credentials dropped",
            "port 8443 ignored",
        ] {
            assert!(notes.contains(want), "{notes}");
        }
        assert_eq!(items("[fd00::1]:443")[0].entry.as_deref(), Some("fd00::1"));
        assert_eq!(items("fd00::/8")[0].entry.as_deref(), Some("fd00::/8"));
        assert!(items("-sso.acme.test")[0].exclude, "a leading - excludes");
    }

    #[test]
    fn too_wide_entries_are_noted_and_star_is_refused() {
        let it = items("*.com\n0.0.0.0/0\n10.0.0.0/8\n10.1.0.0/16\n*\n*.co.uk\n!*.com");
        let why = |i: usize| it[i].notes.join(" | ");
        assert!(
            it[0].wide && why(0).contains("com is a public suffix"),
            "{}",
            why(0)
        );
        assert!(
            it[1].wide && why(1).contains("wider than /16"),
            "{}",
            why(1)
        );
        assert!(it[2].wide && why(2).contains("10.0.0.0/8"), "{}", why(2));
        assert!(!it[3].wide && it[3].notes.is_empty());
        assert!(it[4].error.as_deref().unwrap().contains("scope all on"));
        assert!(it[5].wide, "public suffixes, not only TLDs");
        assert!(
            !it[6].wide && it[6].error.is_none(),
            "an exclusion only narrows"
        );
    }

    #[test]
    fn duplicates_and_what_applies() {
        let it = items("a.test\nA.TEST\n!b.test\napp.acme.test\nnope!\n");
        assert_eq!(it[1].notes, ["listed earlier"]);
        assert_eq!(
            to_apply(&it),
            (vec!["a.test".to_string()], vec!["b.test".to_string()])
        );
    }

    #[test]
    fn limits() {
        assert!(parse(&"a.test\n".repeat(MAX_LINES + 1), &Scope::default()).is_err());
        assert!(parse(&"x".repeat(MAX_BYTES + 1), &Scope::default()).is_err());
        assert!(items("   \n# only a comment\n").is_empty());
    }

    #[test]
    fn prose_words_are_not_hosts() {
        let it = items("not a host!\nintranet\nsee wiki.acme.test for details");
        assert!(
            it.iter().filter(|i| i.line == 1).all(|i| i.entry.is_none()),
            "{it:?}"
        );
        let lone = it.iter().find(|i| i.line == 2).unwrap();
        assert_eq!(lone.entry.as_deref(), Some("intranet"));
        assert!(lone.notes[0].starts_with("single-label"));
        let line3: Vec<_> = it
            .iter()
            .filter(|i| i.line == 3)
            .filter_map(|i| i.entry.clone())
            .collect();
        assert_eq!(line3, ["wiki.acme.test"]);
    }
}
