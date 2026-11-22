//! The action gate (#22): one server-side check for every action that
//! sends traffic to a target. Replay is the first (#17); agent actions,
//! sandbox egress and retests reuse it. The rules live here, not in the
//! UI, so a human and an agent are held to the same ones. The UI's
//! safeguards (two presses for an unsafe method) are how a human satisfies
//! a rule, not the rule itself.
//!
//! The scope engine here is spall's only one (#34): the gate, `scope
//! check`, and host states in traffic all ask [`Scope::explain`].

use serde::Serialize;
use std::net::IpAddr;

/// Hosts the engagement allows traffic to. Empty means nothing is in
/// scope; `*` means every host (opt-in: `SPALL_SCOPE=*`, or `scope all on`
/// in the console for the running process). Exclusions (`!entry`) win over
/// both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    rules: Vec<Rule>,
    excludes: Vec<Rule>,
    all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    /// A hostname: lowercased ASCII (IDN labels as punycode), no trailing dot.
    Host(String),
    /// `*.example.test`: any subdomain of `example.test`, not the apex.
    Subdomains(String),
    /// An IP literal or a network (IPv4-mapped IPv6 stored as IPv4).
    Net(IpAddr, u8),
}

impl Rule {
    fn matches(&self, host: &str, ip: Option<IpAddr>) -> bool {
        match (self, ip) {
            (Rule::Net(net, bits), Some(ip)) => in_network(ip, *net, *bits),
            (Rule::Host(h), None) => h == host,
            (Rule::Subdomains(apex), None) => host
                .strip_suffix(apex.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
            _ => false,
        }
    }

    fn entry(&self) -> String {
        match self {
            Rule::Host(h) => h.clone(),
            Rule::Subdomains(a) => format!("*.{a}"),
            Rule::Net(ip, bits) if *bits == max_bits(*ip) => ip.to_string(),
            Rule::Net(ip, bits) => format!("{ip}/{bits}"),
        }
    }
}

/// Where a host stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostState {
    /// Matches an in-scope entry (or every host is allowed) and no exclusion.
    In,
    /// Matches an exclusion.
    Out,
    /// Matches nothing: not in scope, and nobody has decided.
    Unknown,
}

/// [`Scope::explain`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    /// The host as compared: lowercase ASCII, no port, brackets or trailing dot.
    pub host: String,
    pub state: HostState,
    /// The entry that decided it (`!entry` for an exclusion, `*` for all).
    pub by: Option<String>,
}

/// How to widen the scope, for denials.
const WIDEN: &str = "add hosts to SPALL_SCOPE, or run `scope all on` in the console";

const NO_STAR: &str = "use `scope all on` to allow every host";

impl Scope {
    /// Parse comma-separated entries: `example.test`, `*.example.test`,
    /// `10.0.0.5`, `10.0.0.0/8`, `fd00::/8`, `[::1]`, `bücher.test`, and
    /// exclusions as `!entry`. Whitespace and empty entries are ignored.
    pub fn parse(spec: &str) -> Result<Scope, String> {
        let mut scope = Scope::default();
        for raw in spec.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                scope.all = true;
            } else if let Some(ex) = entry.strip_prefix('!') {
                scope.exclude(ex)?;
            } else {
                scope.add(entry)?;
            }
        }
        Ok(scope)
    }

    /// Nothing is in scope (exclusions alone allow nothing).
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && !self.all
    }

    /// Every host not excluded is in scope.
    pub fn allows_all(&self) -> bool {
        self.all
    }

    /// Add an in-scope entry (same syntax as `--scope`). `Ok(false)` if
    /// already there.
    pub fn add(&mut self, entry: &str) -> Result<bool, String> {
        let entry = entry.trim();
        if entry == "*" {
            return Err(NO_STAR.into());
        }
        if let Some(ex) = entry.strip_prefix('!') {
            return Err(format!(
                "use `scope exclude {}` to exclude a host",
                ex.trim()
            ));
        }
        Ok(push_new(&mut self.rules, parse_rule(entry)?))
    }

    /// Exclude an entry: hosts it matches are out of scope even when an
    /// in-scope entry or `*` matches them. `Ok(false)` if already excluded.
    pub fn exclude(&mut self, entry: &str) -> Result<bool, String> {
        let entry = entry.trim();
        let entry = entry.strip_prefix('!').unwrap_or(entry).trim();
        if entry == "*" {
            return Err("excluding every host would block everything; use `scope all off`, or remove entries".into());
        }
        Ok(push_new(&mut self.excludes, parse_rule(entry)?))
    }

    /// Remove an entry: `!entry` removes that exclusion only; a plain entry
    /// is removed from both lists. False if it wasn't there.
    pub fn remove(&mut self, entry: &str) -> bool {
        let entry = entry.trim();
        let (only_excludes, bare) = match entry.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, entry),
        };
        let Ok(rule) = parse_rule(bare) else {
            return false;
        };
        let before = self.rules.len() + self.excludes.len();
        if !only_excludes {
            self.rules.retain(|r| *r != rule);
        }
        self.excludes.retain(|r| *r != rule);
        self.rules.len() + self.excludes.len() != before
    }

    /// The same entries with every host allowed, or not.
    pub fn with_all(&self, all: bool) -> Scope {
        Scope {
            all,
            ..self.clone()
        }
    }

    /// Where a host (from a URL or a Host header, port allowed) stands, and
    /// which entry decided it. Exclusions first, then entries, then `*`.
    pub fn explain(&self, host: &str) -> Verdict {
        let Some(host) = normalize_host(host) else {
            return Verdict {
                host: String::new(),
                state: HostState::Unknown,
                by: None,
            };
        };
        let ip = parse_ip(&host);
        let (state, by) = if let Some(r) = self.excludes.iter().find(|r| r.matches(&host, ip)) {
            (HostState::Out, Some(format!("!{}", r.entry())))
        } else if let Some(r) = self.rules.iter().find(|r| r.matches(&host, ip)) {
            (HostState::In, Some(r.entry()))
        } else if self.all {
            (HostState::In, Some("*".into()))
        } else {
            (HostState::Unknown, None)
        };
        Verdict { host, state, by }
    }

    /// Whether a host is in scope.
    pub fn contains(&self, host: &str) -> bool {
        self.explain(host).state == HostState::In
    }

    /// The entries, for display and audit records: in-scope entries, then
    /// `!exclusions`, then `*` if every host is allowed. `parse` reads it back.
    pub fn entries(&self) -> Vec<String> {
        let mut out: Vec<String> = self.rules.iter().map(Rule::entry).collect();
        out.extend(self.excludes.iter().map(|r| format!("!{}", r.entry())));
        if self.all {
            out.push("*".into());
        }
        out
    }
}

fn push_new(list: &mut Vec<Rule>, rule: Rule) -> bool {
    if list.contains(&rule) {
        return false;
    }
    list.push(rule);
    true
}

fn parse_rule(entry: &str) -> Result<Rule, String> {
    parse_rule_inner(entry).map_err(|e| format!("scope entry {entry:?}: {e}"))
}

fn parse_rule_inner(entry: &str) -> Result<Rule, String> {
    if let Some((addr, bits)) = entry.split_once('/') {
        let ip = parse_ip(addr).ok_or("not an IP network")?;
        let bits: u8 = bits.parse().map_err(|_| "bad prefix length")?;
        // A mapped network (::ffff:10.0.0.0/104) becomes its IPv4 form.
        let bits = match addr.parse::<IpAddr>() {
            Ok(IpAddr::V6(_)) if ip.is_ipv4() => bits
                .checked_sub(96)
                .ok_or("prefix length too short for a mapped IPv4 network")?,
            _ => bits,
        };
        if bits > max_bits(ip) {
            return Err("prefix length too long".into());
        }
        return Ok(Rule::Net(ip, bits));
    }
    let bare = entry.trim_start_matches('[').trim_end_matches(']');
    if let Some(ip) = parse_ip(bare) {
        return Ok(Rule::Net(ip, max_bits(ip)));
    }
    let (wild, name) = match entry.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, entry),
    };
    let name = ascii_host(name.trim_end_matches(['.', '。', '．', '｡']))
        .ok_or("not a hostname, *.wildcard, IP or CIDR")?;
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

/// An IP literal, zone id dropped, IPv4-mapped IPv6 as IPv4.
fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s.split_once('%').map_or(s, |(addr, _zone)| addr);
    s.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
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

/// Lowercase ASCII, without port, brackets, zone id or a trailing dot;
/// IDN labels as punycode. `None` if empty or not encodable.
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
    if let Some(ip) = parse_ip(h) {
        return Some(ip.to_string());
    }
    let h = ascii_host(h.trim_end_matches(['.', '。', '．', '｡']))?;
    (!h.is_empty()).then_some(h)
}

/// A hostname as lowercase ASCII: IDN dot variants as `.`, non-ASCII labels
/// as `xn--` punycode. Not full UTS #46 (no NFC or mapping tables): enough
/// that an entry and a host written either way compare equal.
fn ascii_host(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let labels: Option<Vec<String>> = lower
        .split(['.', '。', '．', '｡'])
        .map(|l| {
            if l.is_ascii() {
                Some(l.to_string())
            } else {
                punycode(l).map(|p| format!("xn--{p}"))
            }
        })
        .collect();
    Some(labels?.join("."))
}

/// RFC 3492 punycode encoding of one label. `None` on overflow.
fn punycode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    let adapt = |delta: u32, points: u32, first: bool| {
        let mut delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE - TMIN + 1) * delta / (delta + 38)
    };
    let digit = |d: u32| {
        char::from(if d < 26 {
            b'a' + d as u8
        } else {
            b'0' + (d - 26) as u8
        })
    };
    let cps: Vec<u32> = input.chars().map(u32::from).collect();
    let mut out: String = input.chars().filter(char::is_ascii).collect();
    let basic = out.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (128u32, 0u32, 72u32);
    while (handled as usize) < cps.len() {
        let m = cps.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
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
        return Decision::Deny(format!(
            "no engagement scope is configured, so nothing may be sent ({WIDEN})"
        ));
    }
    let Some(target) = url_host(action.url).filter(|h| !h.is_empty()) else {
        return Decision::Deny("the request has no target host".into());
    };
    if let Some(deny) = host_denial(scope, &target, "") {
        return deny;
    }
    if let Some(host) = action.host_header {
        if let Some(deny) = host_denial(scope, host, "the Host header ") {
            return deny;
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

/// Why a host may not be sent to, if it may not. `what` prefixes the host.
fn host_denial(scope: &Scope, host: &str, what: &str) -> Option<Decision> {
    let v = scope.explain(host);
    let shown = display_host(host);
    match v.state {
        HostState::In => None,
        HostState::Out => {
            let by = v.by.unwrap_or_default();
            Some(Decision::Deny(format!(
                "{what}{shown} is excluded from scope by {by} (exclusions win over in-scope entries; `scope remove {by}` lifts it)"
            )))
        }
        HostState::Unknown => Some(Decision::Deny(format!(
            "{what}{shown} is not in scope (scope: {}; {WIDEN})",
            scope.entries().join(", ")
        ))),
    }
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
            "**",
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
    fn star_means_every_host() {
        let all = scope("*");
        assert!(!all.is_empty());
        assert!(all.allows_all());
        for h in ["a.test", "cdn.other.test:443", "10.1.2.3", "[::1]"] {
            assert!(all.contains(h), "{h}");
        }
        assert!(!all.contains(""));
        assert_eq!(scope("a.test, *").entries(), ["a.test", "*"]);
        let d = check(
            &all,
            &MethodPolicy::default(),
            &action("GET", "http://x.test/", Some("y.test")),
        );
        assert_eq!(d, Decision::Allow);
        // It lifts the host check only; unsafe methods still need confirming.
        let d = check(
            &all,
            &MethodPolicy::default(),
            &action("DELETE", "http://x.test/", None),
        );
        assert!(matches!(d, Decision::NeedsConfirmation(_)), "{d:?}");
    }

    #[test]
    fn all_hosts_can_be_switched_on_and_off_over_a_scope() {
        let base = scope("a.test");
        assert!(!base.allows_all());
        let on = base.with_all(true);
        assert!(on.contains("b.test") && on.allows_all());
        assert_eq!(on.entries(), ["a.test", "*"]);
        let off = scope("*, a.test").with_all(false);
        assert!(!off.contains("b.test") && off.contains("a.test"));
        assert!(Scope::default().with_all(false).is_empty());
    }

    #[test]
    fn denials_name_the_scope_and_how_to_widen_it() {
        let d = check(
            &scope("a.test, *.b.test"),
            &MethodPolicy::default(),
            &action("GET", "http://c.test/", None),
        );
        let Decision::Deny(r) = d else {
            panic!("{d:?}")
        };
        assert!(r.contains("\"c.test\" is not in scope"), "{r}");
        assert!(r.contains("a.test, *.b.test"), "{r}");
        assert!(
            r.contains("SPALL_SCOPE") && r.contains("scope all on"),
            "{r}"
        );
        let Decision::Deny(r) = check(
            &Scope::default(),
            &MethodPolicy::default(),
            &action("GET", "http://c.test/", None),
        ) else {
            panic!()
        };
        assert!(
            r.contains("SPALL_SCOPE") && r.contains("scope all on"),
            "{r}"
        );
    }

    #[test]
    fn entries_can_be_added_and_removed_at_runtime() {
        let mut s = Scope::default();
        assert_eq!(s.add("*.Whatever.com"), Ok(true));
        assert_eq!(s.add("*.whatever.com"), Ok(false), "already there");
        assert_eq!(s.add("10.0.0.0/8"), Ok(true));
        assert!(s.contains("api.whatever.com") && s.contains("10.9.9.9"));
        assert!(s.add("http://x.test/").is_err());
        assert_eq!(
            s.add("*"),
            Err("use `scope all on` to allow every host".into())
        );
        assert!(
            s.remove("*.WHATEVER.com"),
            "removal normalizes like parsing"
        );
        assert!(!s.remove("*.whatever.com"));
        assert!(!s.remove("not an entry"));
        assert_eq!(s.entries(), ["10.0.0.0/8"]);
    }

    #[test]
    fn empty_scope_denies_everything() {
        let d = check(
            &Scope::default(),
            &MethodPolicy::default(),
            &action("GET", "http://a.test/", None),
        );
        assert!(
            matches!(d, Decision::Deny(ref r) if r.contains("SPALL_SCOPE")),
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
    fn punycode_matches_rfc_3492_vectors() {
        assert_eq!(punycode("bücher").as_deref(), Some("bcher-kva"));
        assert_eq!(punycode("münchen").as_deref(), Some("mnchen-3ya"));
        // RFC 3492 7.1 (B) Chinese (simplified) and (L) Japanese.
        assert_eq!(
            punycode("他们为什么不说中文").as_deref(),
            Some("ihqwcrb4cv8a8dqg056pqjye")
        );
        assert_eq!(
            punycode("3年B組金八先生").as_deref(),
            Some("3B-ww4c5e180e575a65lsy2b")
        );
    }

    #[test]
    fn idn_hosts_compare_as_punycode_both_ways() {
        let unicode = scope("bücher.test, *.münchen.test");
        for ok in [
            "xn--bcher-kva.test",
            "BÜCHER.test",
            "XN--BCHER-KVA.TEST:443",
            "a.xn--mnchen-3ya.test",
            "a.münchen.test",
        ] {
            assert!(unicode.contains(ok), "{ok}");
        }
        let ascii = scope("xn--bcher-kva.test");
        assert!(ascii.contains("bücher.test"));
        assert!(
            ascii.contains("bücher。test"),
            "ideographic full stop is a dot"
        );
        assert!(!ascii.contains("bucher.test"));
        assert_eq!(
            unicode.entries(),
            ["xn--bcher-kva.test", "*.xn--mnchen-3ya.test"]
        );
    }

    #[test]
    fn ipv6_zones_and_mapped_ipv4_are_normalized() {
        let s = scope("10.0.0.0/8, fe80::/10");
        for ok in [
            "[::ffff:10.1.2.3]",
            "::ffff:10.1.2.3",
            "[fe80::1%25eth0]:8080",
            "fe80::1%eth0",
        ] {
            assert!(s.contains(ok), "{ok}");
        }
        assert!(
            scope("::ffff:10.0.0.0/104").contains("10.9.9.9"),
            "mapped network"
        );
        assert!(Scope::parse("::ffff:10.0.0.0/64").is_err());
        assert_eq!(scope("::ffff:10.0.0.1").entries(), ["10.0.0.1"]);
    }

    #[test]
    fn exclusions_win_over_entries_and_over_all() {
        let mut s = scope("*.example.test, !sso.example.test, !10.0.0.0/8");
        assert!(s.contains("app.example.test"));
        assert!(!s.contains("sso.example.test"));
        assert!(!s.contains("SSO.Example.Test.:443"));
        assert!(
            s.contains("x.sso.example.test"),
            "an exact exclusion covers that host only"
        );
        assert_eq!(
            s.entries(),
            ["*.example.test", "!sso.example.test", "!10.0.0.0/8"]
        );
        s = s.with_all(true);
        assert!(s.contains("anything.test"));
        assert!(
            !s.contains("sso.example.test") && !s.contains("10.1.1.1"),
            "exclusions beat *"
        );
        // Exclusions alone put nothing in scope.
        assert!(scope("!a.test").is_empty());
        assert!(Scope::parse("!*").is_err());
        assert_eq!(
            Scope::parse(&s.entries().join(", ")),
            Ok(s),
            "entries round-trip"
        );
    }

    #[test]
    fn exclusions_are_added_and_removed() {
        let mut s = scope("*.a.test");
        assert_eq!(s.exclude("pay.a.test"), Ok(true));
        assert_eq!(s.exclude("!PAY.a.test"), Ok(false), "already excluded");
        assert!(s.exclude("*").is_err());
        assert!(s.add("!x.a.test").is_err(), "add points at exclude");
        assert!(!s.contains("pay.a.test"));
        assert!(s.remove("!pay.a.test"));
        assert!(s.contains("pay.a.test"));
        // A plain remove takes an entry out of both lists.
        s.exclude("*.a.test").unwrap();
        assert!(s.remove("*.a.test"));
        assert!(s.entries().is_empty());
        assert!(!s.remove("!nope.test"));
    }

    #[test]
    fn explain_says_which_entry_decided() {
        let s = scope("*.a.test, b.test, !x.a.test").with_all(true);
        let v = |h| s.explain(h);
        assert_eq!(
            v("y.a.test:443"),
            Verdict {
                host: "y.a.test".into(),
                state: HostState::In,
                by: Some("*.a.test".into())
            }
        );
        assert_eq!(v("x.a.test").state, HostState::Out);
        assert_eq!(v("x.a.test").by.as_deref(), Some("!x.a.test"));
        assert_eq!(v("c.test").by.as_deref(), Some("*"));
        let none = scope("a.test").explain("c.test");
        assert_eq!((none.state, none.by), (HostState::Unknown, None));
        assert_eq!(scope("a.test").explain("").state, HostState::Unknown);
        assert_eq!(serde_json::to_value(HostState::Unknown).unwrap(), "unknown");
    }

    #[test]
    fn an_excluded_target_or_host_header_is_denied_by_name() {
        let s = scope("*.a.test, !sso.a.test");
        let p = MethodPolicy::default();
        for a in [
            action("GET", "https://sso.a.test/login", None),
            action("GET", "https://app.a.test/", Some("sso.a.test")),
        ] {
            let d = check(&s, &p, &a);
            let Decision::Deny(r) = d else {
                panic!("{d:?}")
            };
            assert!(
                r.contains("\"sso.a.test\" is excluded from scope by !sso.a.test"),
                "{r}"
            );
            assert!(r.contains("scope remove !sso.a.test"), "{r}");
        }
        let all = s.with_all(true);
        assert!(matches!(
            check(&all, &p, &action("GET", "https://sso.a.test/", None)),
            Decision::Deny(_)
        ));
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
