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
    /// taphttp traffic log (JSON lines) to tail
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

    /// How often to check the log file for new data, in milliseconds
    #[arg(long, env = "SPALL_POLL_MS", default_value_t = 250, value_parser = clap::value_parser!(u64).range(10..))]
    pub poll_ms: u64,
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
    fn rejects_bad_values() {
        assert!(Config::try_parse_from(["spall", "--listen", "nope"]).is_err());
        assert!(Config::try_parse_from(["spall", "--poll-ms", "0"]).is_err());
    }
}
