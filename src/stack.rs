//! Agents never target the stack (#73). spall's own API, taphttp's proxy and
//! taphttp's lever API are refused as agent targets whatever the scope says:
//! a replay can set any method and headers, so an in-scope request to spall
//! itself could read unmasked traffic or, after one careless approval, clear
//! the e-stop or widen scope, which agents may never do (`commands::admit`).
//!
//! Two rules:
//!
//! - **Any port**: the host is one of the stack's names (`--stack-host`, the
//!   lever API's host) or resolves to the same address as one (container
//!   names, aliases, the container IP itself).
//! - **Stack ports only**: a loopback, unspecified or host-gateway address on
//!   a port the stack listens on (spall's `--listen` and `--self-address`,
//!   the proxy's `--proxy-address`, the lever API's). Any other port on this
//!   machine is a normal target: a dev app on `localhost:5173` stays
//!   testable.
//!
//! The check is on the URL (what taphttp connects to). Resolution runs in
//! spall's network, which in compose is taphttp's; a name spall can't
//! resolve is still checked by name and as a literal address. taphttp
//! refusing its own listeners (taphttp#22) is the second layer.

use crate::config::Config;
use std::collections::HashSet;
use std::net::IpAddr;
use std::time::Duration;

/// Names that mean "the machine running the containers".
const HOST_GATEWAY: &[&str] = &[
    "host.docker.internal",
    "gateway.docker.internal",
    "host.containers.internal",
];

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct Stack {
    /// Lowercase, no trailing dot.
    names: Vec<String>,
    ports: Vec<u16>,
}

fn norm(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// `host:port` (or `[v6]:port`) split; `None` without a port.
fn split_addr(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.trim().rsplit_once(':')?;
    Some((norm(host), port.parse().ok()?))
}

/// Host and port a URL connects to.
fn target(url: &str) -> Option<(String, u16)> {
    let uri: hyper::Uri = url.parse().ok()?;
    let host = norm(uri.host()?);
    let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
        Some("https") => 443,
        _ => 80,
    });
    (!host.is_empty()).then_some((host, port))
}

fn local_ip(ip: &IpAddr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_unspecified()))
}

/// A name or literal that means this machine (or its container).
fn local_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || HOST_GATEWAY.contains(&host)
        || host.parse::<IpAddr>().is_ok_and(|ip| local_ip(&ip))
}

async fn resolve(host: &str, port: u16) -> Vec<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }
    match tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port))).await {
        Ok(Ok(addrs)) => addrs.map(|a| a.ip()).collect(),
        _ => Vec::new(),
    }
}

impl Stack {
    pub fn from_config(c: &Config) -> Stack {
        let mut names: Vec<String> = c.stack_hosts.iter().map(|h| norm(h)).collect();
        let mut ports = vec![c.listen.port()];
        for a in c.self_address.iter().chain([&c.proxy_address]) {
            if let Some((_, port)) = split_addr(a) {
                ports.push(port);
            }
        }
        if let Some((host, port)) = c.taphttp_api.as_deref().and_then(target) {
            if !local_name(&host) {
                names.push(host);
            }
            ports.push(port);
        }
        names.retain(|n| !n.is_empty());
        names.sort();
        names.dedup();
        ports.sort_unstable();
        ports.dedup();
        Stack { names, ports }
    }

    /// The rules that don't need DNS: `Some(why)` if `host:port` is the stack.
    fn by_name(&self, host: &str, port: u16) -> Option<String> {
        if self.names.iter().any(|n| n == host) {
            return Some(format!("{host} is one of spall's own services"));
        }
        if local_name(host) && self.ports.contains(&port) {
            return Some(format!(
                "{host}:{port} is this machine on a port spall or taphttp listens on"
            ));
        }
        None
    }

    /// The addresses the stack's names resolve to from here (#13 slice 2): a
    /// sandbox's network allowlist mustn't hold any of them.
    pub async fn addresses(&self) -> Vec<IpAddr> {
        let mut v: Vec<IpAddr> = Vec::new();
        for n in &self.names {
            for ip in resolve(n, 80).await {
                if !v.contains(&ip) {
                    v.push(ip);
                }
            }
        }
        v
    }

    /// `Some(reason)` when an agent may not send to `url`.
    pub async fn refuses(&self, url: &str) -> Option<String> {
        let (host, port) = target(url)?;
        let why = match self.by_name(&host, port) {
            Some(why) => Some(why),
            None => self.by_address(&host, port).await,
        }?;
        Some(format!(
            "{url} is spall's own stack ({why}): agents never send to spall, taphttp's proxy or its lever API, whatever the scope says, and this can't be approved"
        ))
    }

    async fn by_address(&self, host: &str, port: u16) -> Option<String> {
        let ips: HashSet<IpAddr> = resolve(host, port).await.into_iter().collect();
        if ips.is_empty() {
            return None;
        }
        if ips.iter().any(local_ip) && self.ports.contains(&port) {
            return Some(format!(
                "{host} resolves to this machine, on a port spall or taphttp listens on"
            ));
        }
        for name in &self.names {
            if resolve(name, port).await.iter().any(|ip| ips.contains(ip)) {
                return Some(format!("{host} resolves to the same address as {name}"));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn stack(extra: &[&str]) -> Stack {
        let mut args = vec![
            "spall",
            "--listen",
            "0.0.0.0:3000",
            "--proxy-address",
            "127.0.0.1:18080",
            "--taphttp-api",
            "http://taphttp:8081",
        ];
        args.extend_from_slice(extra);
        Stack::from_config(&Config::try_parse_from(args).unwrap())
    }

    #[test]
    fn names_ports_and_addresses() {
        let s = stack(&["--self-address", "127.0.0.1:13000"]);
        assert_eq!(s.names, vec!["spall", "taphttp"]);
        assert_eq!(s.ports, vec![3000, 8081, 13000, 18080]);
        assert_eq!(split_addr("[::1]:3000"), Some(("::1".into(), 3000)));
        assert_eq!(target("http://Spall./x"), Some(("spall".into(), 80)));
        assert_eq!(target("https://[::1]/"), Some(("::1".into(), 443)));
        assert_eq!(target("not a url"), None);
    }

    #[test]
    fn the_stack_by_name_on_any_port_and_this_machine_on_stack_ports() {
        let s = stack(&["--self-address", "127.0.0.1:13000"]);
        for (host, port) in [
            ("spall", 3000),
            ("spall", 9999),
            ("taphttp", 8080),
            ("127.0.0.1", 3000),
            ("127.1.2.3", 13000),
            ("localhost", 18080),
            ("app.localhost", 8081),
            ("::1", 3000),
            ("0.0.0.0", 3000),
            ("::ffff:127.0.0.1", 3000),
            ("host.docker.internal", 13000),
        ] {
            assert!(s.by_name(host, port).is_some(), "{host}:{port}");
        }
        for (host, port) in [
            ("localhost", 5173),
            ("127.0.0.1", 8000),
            ("host.docker.internal", 8080),
            ("example.com", 3000),
            ("spall.example.com", 3000),
            ("10.0.0.5", 3000),
        ] {
            assert!(s.by_name(host, port).is_none(), "{host}:{port}");
        }
    }

    #[test]
    fn more_names_and_no_api() {
        let s = Stack::from_config(
            &Config::try_parse_from(["spall", "--stack-host", "spall,taphttp,spall-proxy"])
                .unwrap(),
        );
        assert!(s.by_name("spall-proxy", 1).is_some());
        assert_eq!(
            s.ports,
            vec![3000, 8080],
            "defaults: --listen and --proxy-address"
        );
        // A lever API on loopback adds its port, not a name.
        let s = Stack::from_config(
            &Config::try_parse_from(["spall", "--taphttp-api", "http://127.0.0.1:9081"]).unwrap(),
        );
        assert!(!s.names.iter().any(|n| n == "127.0.0.1"));
        assert!(s.by_name("127.0.0.1", 9081).is_some());
    }

    #[tokio::test]
    async fn addresses_resolve_to_the_stack() {
        let s = stack(&[]);
        assert!(s
            .refuses("http://localhost:3000/api/status")
            .await
            .unwrap()
            .contains("can't be approved"));
        // A literal address resolves to itself.
        assert!(s.by_address("127.0.0.1", 8081).await.is_some());
        assert!(s.by_address("127.0.0.1", 5173).await.is_none());
        assert!(s.refuses("http://localhost:5173/").await.is_none());
    }
}
