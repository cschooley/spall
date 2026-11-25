//! The Public Suffix List (#83), vendored in `src/psl/` (see SOURCE.md):
//! whether a name is one under which anyone can register (`com`, `co.uk`,
//! `github.io`), so `*.<it>` would put strangers' sites in scope.

use std::collections::HashSet;
use std::sync::OnceLock;

const LIST: &str = include_str!("psl/public_suffix_list.dat");

struct Rules {
    exact: HashSet<String>,
    /// `*.ck` is stored as `ck`: every direct child of it is a suffix.
    wild: HashSet<String>,
    /// `!www.ck`: not a suffix, despite a wildcard.
    except: HashSet<String>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let mut r = Rules {
            exact: HashSet::new(),
            wild: HashSet::new(),
            except: HashSet::new(),
        };
        for line in LIST.lines() {
            let rule = line.split_whitespace().next().unwrap_or("");
            if rule.is_empty() || rule.starts_with("//") {
                continue;
            }
            // The list is in Unicode; hosts are compared in ASCII (punycode).
            let (set, name) = if let Some(n) = rule.strip_prefix('!') {
                (&mut r.except, n)
            } else if let Some(n) = rule.strip_prefix("*.") {
                (&mut r.wild, n)
            } else {
                (&mut r.exact, rule)
            };
            if let Some(a) = crate::gate::ascii_host(name) {
                set.insert(a);
            }
        }
        r
    })
}

/// Whether `name` (lowercase ASCII, as scope compares hosts) is a public
/// suffix. Any single label is (the list's implicit `*` rule).
pub fn is_public_suffix(name: &str) -> bool {
    let name = name.trim_end_matches('.');
    if name.is_empty() || !name.contains('.') {
        return true;
    }
    let r = rules();
    if r.except.contains(name) {
        return false;
    }
    if r.exact.contains(name) {
        return true;
    }
    name.split_once('.')
        .is_some_and(|(_, parent)| r.wild.contains(parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psl_knows_registries_and_shared_hosting_from_sites() {
        for s in [
            "com",
            "uk",
            "co.uk",
            "github.io",
            "s3.amazonaws.com",
            "xn--fiqs8s",
            "ck",
            "foo.ck",
        ] {
            assert!(is_public_suffix(s), "{s}");
        }
        for s in [
            "example.com",
            "bbc.co.uk",
            "acme.github.io",
            "www.ck",
            "app.example.test",
        ] {
            assert!(!is_public_suffix(s), "{s}");
        }
        // The vendored file is the one SOURCE.md names.
        assert!(LIST.contains("// COMMIT: cb13e39ae72a79e17022b313f510c585751b5334"));
        assert!(rules().exact.len() > 5000);
    }
}
