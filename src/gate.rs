//! The action gate (#22): one server-side check for every action that
//! sends traffic to a target. Replay is the first (#17); agent actions,
//! sandbox egress and retests reuse it. The rules live here, not in the
//! UI, so a human and an agent are held to the same ones. The UI's
//! safeguards (two presses for an unsafe method) are how a human satisfies
//! a rule, not the rule itself.

use std::net::IpAddr;

/// Hosts the engagement allows traffic to. Empty means nothing is in scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    /// A hostname, lowercased, no trailing dot.
    Host(String),
    /// `*.example.test`: any subdomain of `example.test`, not the apex.
    Subdomains(String),
    /// An IP literal or a network.
    Net(IpAddr, u8),
}

impl Scope {
    /// Parse comma-separated entries: `example.test`, `*.example.test`,
    /// `10.0.0.5`, `10.0.0.0/8`, `fd00::/8`, `[::1]`. Whitespace and empty
    /// entries are ignored.
    pub fn parse(spec: &str) -> Result<Scope, String> {
        let mut rules = Vec::new();
        for raw in spec.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            rules.push(parse_rule(entry).map_err(|e| format!("scope entry {entry:?}: {e}"))?);
        }
        Ok(Scope { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Whether a host (from a URL or a Host header, port allowed) is in scope.
    pub fn contains(&self, host: &str) -> bool {
        let Some(host) = normalize_host(host) else {
            return false;
        };
        let ip: Option<IpAddr> = host.parse().ok();
        self.rules.iter().any(|rule| match (rule, ip) {
            (Rule::Net(net, bits), Some(ip)) => in_network(ip, *net, *bits),
            (Rule::Host(h), None) => *h == host,
            (Rule::Subdomains(apex), None) => host
                .strip_suffix(apex.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
            _ => false,
        })
    }

    /// The entries, for display (`scope` in the console, audit records).
    pub fn entries(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|r| match r {
                Rule::Host(h) => h.clone(),
                Rule::Subdomains(a) => format!("*.{a}"),
                Rule::Net(ip, bits) if *bits == max_bits(*ip) => ip.to_string(),
                Rule::Net(ip, bits) => format!("{ip}/{bits}"),
            })
            .collect()
    }
}

fn parse_rule(entry: &str) -> Result<Rule, String> {
    if let Some((addr, bits)) = entry.split_once('/') {
        let ip: IpAddr = addr.parse().map_err(|_| "not an IP network")?;
        let bits: u8 = bits.parse().map_err(|_| "bad prefix length")?;
        if bits > max_bits(ip) {
            return Err("prefix length too long".into());
        }
        return Ok(Rule::Net(ip, bits));
    }
    let bare = entry.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(Rule::Net(ip, max_bits(ip)));
    }
    let (wild, name) = match entry.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, entry),
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if name.is_empty() || !name.split('.').all(label_ok) {
        return Err("not a hostname, *.wildcard, IP or CIDR".into());
    }
    Ok(if wild {
        Rule::Subdomains(name)
    } else {
        Rule::Host(name)
    })
}

fn max_bits(ip: IpAddr) -> u8 {
    if ip.is_ipv4() {
        32
    } else {
        128
    }
}

fn in_network(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    let (ip, net, width) = match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (u32::from(a) as u128, u32::from(b) as u128, 32),
        (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), 128),
        _ => return false,
    };
    if bits == 0 {
        return true;
    }
    let shift = width - u32::from(bits);
    (ip >> shift) == (net >> shift)
}

/// Lowercase, without port, brackets or a trailing dot. `None` if empty.
fn normalize_host(host: &str) -> Option<String> {
    let h = host.trim();
    let h = if let Some(rest) = h.strip_prefix('[') {
        rest.split_once(']').map(|(inside, _)| inside)?
    } else {
        match h.rsplit_once(':') {
            Some((name, port))
                if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) =>
            {
                name
            }
            _ => h,
        }
    };
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    (!h.is_empty()).then_some(h)
}

/// Which methods go without confirmation. Everything else, unknown
/// methods included, needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodPolicy {
    safe: Vec<String>,
}

impl Default for MethodPolicy {
    fn default() -> Self {
        MethodPolicy::new(["GET", "HEAD", "OPTIONS"])
    }
}

impl MethodPolicy {
    pub fn new<S: AsRef<str>>(safe: impl IntoIterator<Item = S>) -> Self {
        MethodPolicy {
            safe: safe
                .into_iter()
                .map(|m| m.as_ref().trim().to_ascii_uppercase())
                .filter(|m| !m.is_empty())
                .collect(),
        }
    }

    fn is_safe(&self, method: &str) -> bool {
        self.safe.iter().any(|m| m.eq_ignore_ascii_case(method))
    }
}

/// An action that would send traffic.
#[derive(Debug, Clone)]
pub struct Action<'a> {
    pub method: &'a str,
    /// Absolute URL (scheme and host), as taphttp records it.
    pub url: &'a str,
    /// The effective `Host` header, if the request has one.
    pub host_header: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    NeedsConfirmation(String),
    Deny(String),
}

impl Decision {
    /// Short name for audit records and API responses.
    pub fn kind(&self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::NeedsConfirmation(_) => "needs_confirmation",
            Decision::Deny(_) => "deny",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Decision::Allow => None,
            Decision::NeedsConfirmation(r) | Decision::Deny(r) => Some(r),
        }
    }
}

/// The host of an absolute URL, if it has one.
pub fn url_host(url: &str) -> Option<String> {
    let uri: hyper::Uri = url.parse().ok()?;
    uri.authority()
        .map(|a| a.as_str().rsplit('@').next().unwrap_or("").to_string())
}

/// Scope first (a denial beats a confirmation), then the method policy.
pub fn check(scope: &Scope, policy: &MethodPolicy, action: &Action) -> Decision {
    if scope.is_empty() {
        return Decision::Deny(
            "no engagement scope is configured, so nothing may be sent (set --scope / SPALL_SCOPE)"
                .into(),
        );
    }
    let Some(target) = url_host(action.url).filter(|h| !h.is_empty()) else {
        return Decision::Deny("the request has no target host".into());
    };
    if !scope.contains(&target) {
        return Decision::Deny(format!("{} is not in scope", display_host(&target)));
    }
    if let Some(host) = action.host_header {
        if !scope.contains(host) {
            return Decision::Deny(format!(
                "the Host header {} is not in scope",
                display_host(host)
            ));
        }
    }
    if !policy.is_safe(action.method) {
        return Decision::NeedsConfirmation(format!(
            "{} can change state on the target; confirm to send",
            action.method
        ));
    }
    Decision::Allow
}

/// A host for a message: normalized, and quoted so odd bytes stay visible.
fn display_host(h: &str) -> String {
    format!("{:?}", normalize_host(h).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> Scope {
        Scope::parse(s).unwrap()
    }

    fn action<'a>(method: &'a str, url: &'a str, host: Option<&'a str>) -> Action<'a> {
        Action {
            method,
            url,
            host_header: host,
        }
    }

    #[test]
    fn exact_hosts_match_case_port_and_trailing_dot_insensitively() {
        let s = scope("app.example.test");
        for ok in [
            "app.example.test",
            "APP.example.test",
            "app.example.test:8443",
            "app.example.test.",
        ] {
            assert!(s.contains(ok), "{ok}");
        }
        for bad in [
            "example.test",
            "x.app.example.test",
            "app.example.test.evil",
            "evil-app.example.test",
            "",
        ] {
            assert!(!s.contains(bad), "{bad}");
        }
    }

    #[test]
    fn wildcards_match_subdomains_only() {
        let s = scope("*.example.test");
        for ok in ["a.example.test", "a.b.example.test", "A.EXAMPLE.TEST:443"] {
            assert!(s.contains(ok), "{ok}");
        }
        for bad in [
            "example.test",
            "evil-example.test",
            "aexample.test",
            ".example.test",
            "example.test.evil",
        ] {
            assert!(!s.contains(bad), "{bad}");
        }
    }

    #[test]
    fn ips_and_networks_match_ip_literals_only() {
        let s = scope("10.0.0.0/8, 192.168.1.5, fd00::/8, [::1]");
        for ok in [
            "10.1.2.3",
            "10.1.2.3:8080",
            "192.168.1.5",
            "[fd00::1]:443",
            "[::1]",
            "::1",
        ] {
            assert!(s.contains(ok), "{ok}");
        }
        for bad in [
            "11.0.0.1",
            "192.168.1.6",
            "[fe80::1]",
            "10.example.test",
            "localhost",
        ] {
            assert!(!s.contains(bad), "{bad}");
        }
        // A hostname is never resolved to match a network.
        assert!(!scope("127.0.0.0/8").contains("localhost"));
    }

    #[test]
    fn bad_entries_are_rejected_and_blanks_ignored() {
        assert!(scope(" , ,").is_empty());
        for bad in [
            "*",
            "*.",
            "a b",
            "10.0.0.0/33",
            "fd00::/129",
            "x/8",
            "http://a.test",
            "a.test/path",
            "*.*.a.test",
        ] {
            assert!(Scope::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            scope("A.test, *.b.test, 10.0.0.0/8, 10.0.0.1").entries(),
            ["a.test", "*.b.test", "10.0.0.0/8", "10.0.0.1"]
        );
    }

    #[test]
    fn empty_scope_denies_everything() {
        let d = check(
            &Scope::default(),
            &MethodPolicy::default(),
            &action("GET", "http://a.test/", None),
        );
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("--scope")),
            "{d:?}"
        );
    }

    #[test]
    fn out_of_scope_target_or_host_header_is_denied() {
        let s = scope("app.example.test");
        let p = MethodPolicy::default();
        let d = check(&s, &p, &action("GET", "https://cdn.other.test/x.js", None));
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("cdn.other.test")),
            "{d:?}"
        );
        // Same IP, different virtual host: the Host header must be in scope too.
        let d = check(
            &s,
            &p,
            &action("GET", "https://app.example.test/", Some("admin.internal")),
        );
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("Host header")),
            "{d:?}"
        );
        let d = check(&s, &p, &action("GET", "/relative", None));
        assert!(matches!(d, Decision::Deny(_)), "{d:?}");
        // userinfo can't smuggle a different host past the check
        let d = check(
            &s,
            &p,
            &action("GET", "http://app.example.test@evil.test/", None),
        );
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("evil.test")),
            "{d:?}"
        );
    }

    #[test]
    fn method_policy_allows_safe_and_confirms_the_rest() {
        let s = scope("a.test");
        let p = MethodPolicy::default();
        for m in ["GET", "HEAD", "OPTIONS", "get"] {
            assert_eq!(
                check(&s, &p, &action(m, "http://a.test/", Some("a.test"))),
                Decision::Allow,
                "{m}"
            );
        }
        for m in ["POST", "PUT", "PATCH", "DELETE", "PROPFIND", "X-CUSTOM"] {
            let d = check(&s, &p, &action(m, "http://a.test/", None));
            assert!(matches!(d, Decision::NeedsConfirmation(_)), "{m}: {d:?}");
        }
        let lax = MethodPolicy::new(["GET", "POST"]);
        assert_eq!(
            check(&s, &lax, &action("POST", "http://a.test/", None)),
            Decision::Allow
        );
    }

    #[test]
    fn scope_is_checked_before_method() {
        let d = check(
            &scope("a.test"),
            &MethodPolicy::default(),
            &action("DELETE", "http://b.test/", None),
        );
        assert!(matches!(d, Decision::Deny(_)), "{d:?}");
    }

    #[test]
    fn hostile_hosts_are_quoted_in_reasons() {
        let d = check(
            &scope("a.test"),
            &MethodPolicy::default(),
            &action("GET", "http://a.test/", Some("x\n<b>")),
        );
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("\\n")),
            "{d:?}"
        );
    }
}
