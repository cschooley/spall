use crate::gate::{MethodPolicy, Scope};
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "spall",
    version,
    about = "spall: DAST assessment workbench (M0 live traffic console)"
)]
pub struct Config {
    /// taphttp lever API base URL, e.g. http://taphttp:8081. When set,
    /// traffic and history come from the API and --log-file is not read.
    /// Plain http only: the API belongs on a private network.
    #[arg(long, env = "SPALL_TAPHTTP_API", value_parser = parse_api_base)]
    pub taphttp_api: Option<String>,

    /// Where clients reach taphttp's proxy, shown in the UI's status bar.
    /// Display only: spall never connects to it.
    #[arg(long, env = "SPALL_PROXY_ADDRESS", default_value = "127.0.0.1:8080")]
    pub proxy_address: String,

    /// File holding the lever API token (taphttp --api-token-file)
    #[arg(
        long,
        env = "SPALL_TAPHTTP_TOKEN_FILE",
        default_value = "/token/api-token"
    )]
    pub taphttp_token_file: PathBuf,

    /// taphttp traffic log (JSON lines) to tail when --taphttp-api is not set
    #[arg(long, env = "SPALL_LOG_FILE", default_value = "/data/traffic.jsonl")]
    pub log_file: PathBuf,

    /// Address for the web UI. Keep this on loopback: the UI shows raw
    /// traffic, including credentials, and has no authentication.
    #[arg(long, env = "SPALL_LISTEN", default_value = "127.0.0.1:3000")]
    pub listen: SocketAddr,

    /// Host header values the UI answers to (port ignored), comma-separated.
    /// Guards against DNS rebinding. "*" disables the check.
    #[arg(
        long = "allowed-host",
        env = "SPALL_ALLOWED_HOSTS",
        value_delimiter = ',',
        default_value = "localhost,127.0.0.1,[::1]"
    )]
    pub allowed_hosts: Vec<String>,

    /// Number of recent entries replayed to a newly connected browser
    #[arg(long, env = "SPALL_HISTORY", default_value_t = 1000)]
    pub history: usize,

    /// Engagement scope for replays (#22), comma-separated: exact hosts,
    /// `*.example.test` (subdomains only), IPs and CIDRs, `*` for every
    /// host, and `!entry` exclusions, which win over everything else. Both
    /// a replay's URL host and its Host header must match. Empty (the
    /// default) means nothing may be sent.
    #[arg(long, env = "SPALL_SCOPE", default_value = "", value_parser = Scope::parse)]
    pub scope: Scope,

    /// Methods that replay without confirmation, comma-separated. Any
    /// other method needs a second, confirmed request. Seeds a new
    /// engagement's rules of engagement (#36); after that the rules decide.
    #[arg(long, env = "SPALL_SAFE_METHODS", default_value = "GET,HEAD,OPTIONS", value_parser = parse_methods)]
    pub safe_methods: MethodPolicy,

    /// Append-only audit log of every replay attempt (JSON lines, mode
    /// 0600). Opened at startup with --taphttp-api; spall won't start if it
    /// can't write it.
    #[arg(long, env = "SPALL_AUDIT_LOG", default_value = "/data/audit.jsonl")]
    pub audit_log: PathBuf,

    /// Directory for engagements (#35): one JSON file each (mode 0600) and
    /// an `active` file. Created if missing. With none there yet, --scope
    /// seeds the first; after that the engagement's scope is used.
    #[arg(long, env = "SPALL_ENGAGEMENTS", default_value = "/data/engagements")]
    pub engagements: PathBuf,

    /// Allow exporting findings unredacted (#51), when an export also asks
    /// for it. Off by default: exports are redacted (flux), and an
    /// unredacted one is marked `flux/redacted: false` so consumers can
    /// refuse it. Captured credentials end up in that file.
    #[arg(long, env = "SPALL_ALLOW_UNREDACTED_EXPORT")]
    pub allow_unredacted_export: bool,

    /// How often to check the log file for new data, in milliseconds
    #[arg(long, env = "SPALL_POLL_MS", default_value_t = 250, value_parser = clap::value_parser!(u64).range(10..))]
    pub poll_ms: u64,
}

fn parse_methods(s: &str) -> Result<MethodPolicy, String> {
    let methods: Vec<&str> = s
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect();
    if let Some(bad) = methods
        .iter()
        .find(|m| !m.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-'))
    {
        return Err(format!("{bad:?} is not an HTTP method"));
    }
    Ok(MethodPolicy::new(methods))
}

/// Accept `http://host[:port]` with no path, query or credentials, and
/// normalize away a trailing slash.
fn parse_api_base(s: &str) -> Result<String, String> {
    let rest = s.strip_prefix("http://").ok_or_else(|| {
        "must start with http:// (the lever API is plain HTTP on a private network)".to_string()
    })?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err("must be http://host[:port] with no path, query or credentials".into());
    }
    Ok(format!("http://{authority}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_localhost_only() {
        let c = Config::try_parse_from(["spall"]).unwrap();
        assert!(c.listen.ip().is_loopback());
        assert_eq!(c.listen.port(), 3000);
        assert_eq!(c.log_file, PathBuf::from("/data/traffic.jsonl"));
        assert_eq!(c.allowed_hosts, vec!["localhost", "127.0.0.1", "[::1]"]);
        assert!(!c.allowed_hosts.iter().any(|h| h == "*"));
    }

    #[test]
    fn flags_override_defaults() {
        let c = Config::try_parse_from([
            "spall",
            "--log-file",
            "/tmp/t.jsonl",
            "--listen",
            "0.0.0.0:4000",
            "--allowed-host",
            "spall.test,localhost",
            "--history",
            "5",
            "--poll-ms",
            "50",
        ])
        .unwrap();
        assert_eq!(c.log_file, PathBuf::from("/tmp/t.jsonl"));
        assert_eq!(c.listen, "0.0.0.0:4000".parse().unwrap());
        assert_eq!(c.allowed_hosts, vec!["spall.test", "localhost"]);
        assert_eq!(c.history, 5);
        assert_eq!(c.poll_ms, 50);
    }

    #[test]
    fn proxy_address_is_display_only_and_defaults_to_local() {
        let c = Config::try_parse_from(["spall"]).unwrap();
        assert_eq!(c.proxy_address, "127.0.0.1:8080");
        let c = Config::try_parse_from(["spall", "--proxy-address", "127.0.0.1:18080"]).unwrap();
        assert_eq!(c.proxy_address, "127.0.0.1:18080");
    }

    #[test]
    fn api_mode_is_off_by_default() {
        let c = Config::try_parse_from(["spall"]).unwrap();
        assert_eq!(c.taphttp_api, None);
        assert_eq!(c.taphttp_token_file, PathBuf::from("/token/api-token"));
    }

    #[test]
    fn api_base_is_validated_and_normalized() {
        for (input, want) in [
            ("http://taphttp:8081", "http://taphttp:8081"),
            ("http://taphttp:8081/", "http://taphttp:8081"),
            ("http://127.0.0.1:8081", "http://127.0.0.1:8081"),
        ] {
            let c = Config::try_parse_from(["spall", "--taphttp-api", input]).unwrap();
            assert_eq!(c.taphttp_api.as_deref(), Some(want), "{input}");
        }
        for bad in [
            "https://taphttp:8081",
            "taphttp:8081",
            "http://",
            "http://taphttp:8081/api",
            "http://user:pw@taphttp:8081",
            "http://taphttp:8081?x=1",
        ] {
            assert!(
                Config::try_parse_from(["spall", "--taphttp-api", bad]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn replay_defaults_deny_everything_and_audit_to_data() {
        let c = Config::try_parse_from(["spall"]).unwrap();
        assert!(c.scope.is_empty(), "empty scope: no replay may be sent");
        assert_eq!(c.audit_log, PathBuf::from("/data/audit.jsonl"));
        assert_eq!(c.engagements, PathBuf::from("/data/engagements"));
        assert_eq!(c.safe_methods, MethodPolicy::default());
    }

    #[test]
    fn scope_and_method_policy_are_parsed() {
        let c = Config::try_parse_from([
            "spall",
            "--scope",
            "app.example.test, *.api.example.test,10.0.0.0/8",
            "--safe-methods",
            "GET,HEAD",
            "--audit-log",
            "/tmp/a.jsonl",
        ])
        .unwrap();
        assert!(c.scope.contains("x.api.example.test"));
        assert!(c.scope.contains("10.2.3.4"));
        assert!(!c.scope.contains("example.test"));
        assert_eq!(c.safe_methods, MethodPolicy::new(["GET", "HEAD"]));
        assert_eq!(c.audit_log, PathBuf::from("/tmp/a.jsonl"));
        assert!(Config::try_parse_from(["spall", "--scope", "http://x.test/"]).is_err());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::try_parse_from(["spall", "--listen", "nope"]).is_err());
        assert!(Config::try_parse_from(["spall", "--poll-ms", "0"]).is_err());
    }
}
