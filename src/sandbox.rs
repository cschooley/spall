//! The execution sandbox (#13, docs/sandbox.md): the third sensor. An agent
//! runs real tools (`curl`, `jq`, `nmap`, Python) in a disposable container
//! under gVisor, never on the host. **Containment, not filtering:** there is
//! no list of forbidden commands. The host is untouched because the sandbox
//! can't reach it, and nothing leaves because it has no route.
//!
//! Slice 1: the primitive, its gate seam and its stop path. Each task gets
//! one container (`--runtime=runsc`, read-only, a tmpfs `/scratch`, uid
//! 10001, capped CPU, memory and pids) started on its first command and
//! removed when the task ends or the e-stop is engaged. **No network yet**
//! (`--network none`): HTTP through taphttp on a per-task credential is
//! slice 2, and until then a sandbox reaches nothing at all.
//!
//! **No silent weaker mode.** Where `runsc` isn't a Docker runtime, Docker
//! isn't reachable, or the image isn't built, the sandbox refuses and says
//! why; it never falls back to a plain container.
//!
//! Commands are argv lists, never a shell string, so the record says
//! exactly what ran. (A tool may still be `sh -c`; the container is the
//! boundary, not the argv.)

use crate::model::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// The toolset image spall starts (`sandbox/Dockerfile`).
pub const IMAGE_DEFAULT: &str = "spall-sandbox:dev";
/// A command's wall clock when the agent doesn't say, and the most it may ask.
pub const DEFAULT_TIMEOUT: u64 = 60;
pub const MAX_TIMEOUT: u64 = 300;
pub const MAX_ARGS: usize = 64;
pub const MAX_ARG: usize = 4096;
/// Kept inline per stream; the rest is cut (full output as an artifact is slice 3).
pub const KEEP: usize = 16 * 1024;
/// Read at most this much of a stream.
const CAPTURE: usize = 1 << 20;
/// The label on every sandbox container, so leftovers can be found.
pub const LABEL: &str = "spall.sandbox";

/// One command: an argv list and a wall clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub argv: Vec<String>,
    pub timeout: Duration,
}

impl Cmd {
    /// A checked command: 1-64 arguments, none empty-named, no NULs, a
    /// timeout of 1-300 seconds (60 if not given).
    pub fn new(argv: Vec<String>, timeout_s: Option<u64>) -> Result<Cmd, String> {
        let Some(first) = argv.first() else {
            return Err("argv is empty: give the program and its arguments as a list".into());
        };
        if first.is_empty() {
            return Err("argv[0] is empty: name the program".into());
        }
        if argv.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments"));
        }
        if let Some(a) = argv.iter().find(|a| a.len() > MAX_ARG || a.contains('\0')) {
            return Err(format!(
                "an argument is too long or holds a NUL ({} bytes; at most {MAX_ARG})",
                a.len()
            ));
        }
        let secs = timeout_s.unwrap_or(DEFAULT_TIMEOUT);
        if !(1..=MAX_TIMEOUT).contains(&secs) {
            return Err(format!("timeout_seconds: 1-{MAX_TIMEOUT}"));
        }
        Ok(Cmd {
            argv,
            timeout: Duration::from_secs(secs),
        })
    }
}

/// What a command did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub argv: Vec<String>,
    /// Its exit code; None when it was killed or never ran.
    pub exit: Option<i32>,
    pub ms: u64,
    pub stdout: String,
    pub stderr: String,
    /// Output was longer than [`KEEP`] and was cut.
    pub truncated: bool,
    /// It ran out of its wall clock and was killed.
    pub timed_out: bool,
}

/// Which task a sandbox belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub engagement: String,
    pub task: String,
}

/// Network for one command (slice 2): a proxy URL carrying a per-command
/// taphttp credential, and taphttp's CA so HTTPS through it verifies.
pub struct Net {
    pub proxy_url: String,
    pub ca_pem: String,
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL holds the credential's password: never printed.
        f.write_str("Net { proxy_url: <secret>, .. }")
    }
}

/// The env vars a command gets with a network: every proxy spelling tools
/// read, and taphttp's CA as the trust root.
pub const PROXY_VARS: [&str; 5] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "ALL_PROXY",
];
pub const CA_VARS: [&str; 3] = ["SSL_CERT_FILE", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE"];
pub const CA_PATH: &str = "/tmp/spall-ca.pem";

pub trait Sandbox: Send + Sync {
    /// Run `cmd` in `key`'s sandbox, starting it first if needed; with
    /// `net`, through taphttp on that command's credential.
    fn exec<'a>(
        &'a self,
        key: &'a Key,
        cmd: &'a Cmd,
        net: Option<&'a Net>,
    ) -> BoxFuture<'a, Result<Run, String>>;
    /// Kill and remove `key`'s sandbox and its scratch; nothing if it has none.
    fn teardown<'a>(&'a self, key: &'a Key) -> BoxFuture<'a, ()>;
    /// The sandboxes running now.
    fn live(&self) -> Vec<Key>;
    /// Whether this sandbox was given a network to check (slice 2).
    fn has_network(&self) -> bool;
    /// Check that network (see [`check_network`]); once it passes, new
    /// sandboxes join it. `Ok(Some(proxy))`: ready, through taphttp's
    /// credential-only listener at `proxy`; `Ok(None)`: no network
    /// configured; `Err`: not safe yet, and why.
    fn check_network<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, String>>;
}

/// Where a sandbox network is, and taphttp's two listeners as seen from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetCfg {
    pub network: String,
    pub proxy: String,
    pub fence: String,
}

/// Where spall stands on the sandbox.
pub enum State {
    /// Not asked for (`--sandbox-image` unset).
    Off,
    /// Asked for, but it won't start here: why.
    Unavailable(String),
    Ready(std::sync::Arc<dyn Sandbox>),
}

impl State {
    /// For `/api/status`.
    pub fn summary(&self) -> serde_json::Value {
        match self {
            State::Off => serde_json::json!({"state": "off"}),
            State::Unavailable(why) => serde_json::json!({"state": "unavailable", "reason": why}),
            State::Ready(s) => serde_json::json!({"state": "ready", "live": s.live().len()}),
        }
    }
}

/// Whether sandboxes get a network (slice 2).
pub enum NetState {
    /// No `--sandbox-network`: sandboxes have none.
    Off,
    /// Configured, but not safe yet: why (rechecked every minute).
    Waiting(String),
    /// Checked: sandboxes join the network and each command gets its own
    /// credential on taphttp's credential-only listener at `proxy`.
    Ready { proxy: String },
}

impl NetState {
    pub fn summary(&self) -> serde_json::Value {
        match self {
            NetState::Off => serde_json::json!({"state": "off"}),
            NetState::Waiting(why) => serde_json::json!({"state": "waiting", "reason": why}),
            NetState::Ready { .. } => serde_json::json!({"state": "ready"}),
        }
    }
}

/// A command's outcome with its network: what it could reach and what
/// taphttp counted on its credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub run: Run,
    /// One line for the agent and the log: the hosts, or why none.
    pub network: String,
    pub sent: Option<u32>,
    pub refused: Option<u32>,
}

/// `user:pass` for a URL's userinfo: everything but unreserved escaped.
pub fn userinfo(user: &str, pass: &str) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    };
    format!("{}:{}", enc(user), enc(pass))
}

/// The container name for a task: `spall-sbx-<engagement>-<t-n>`.
pub fn container_name(key: &Key) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(48)
            .collect()
    };
    format!("spall-sbx-{}-{}", clean(&key.engagement), clean(&key.task))
}

/// `docker run` flags for a sandbox: gVisor, no network, read-only, a tmpfs
/// scratch, an unprivileged user, capped resources. The agent's sandbox and
/// `spall sandbox-shell` share them, so the shell is what the agent gets.
pub fn run_flags(name: &str, labels: &[(&str, &str)], network: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = [
        "--name",
        name,
        "--runtime=runsc",
        "--network",
        network.unwrap_or("none"),
        "--read-only",
        "--tmpfs",
        "/scratch:rw,size=256m,mode=1777",
        "--tmpfs",
        "/tmp:rw,size=64m,mode=1777",
        "--workdir",
        "/scratch",
        "--user",
        "10001:10001",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        "128",
        "--memory",
        "512m",
        "--memory-swap",
        "512m",
        "--cpus",
        "1",
        "--env",
        "HOME=/scratch",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push("--label".into());
    v.push(format!("{LABEL}=1"));
    for (k, val) in labels {
        v.push("--label".into());
        v.push(format!("{k}={val}"));
    }
    v
}

/// Cut `s` to [`KEEP`] bytes on a char boundary; whether it was cut.
fn keep(bytes: &[u8]) -> (String, bool) {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= KEEP {
        return (s.into_owned(), false);
    }
    let mut end = KEEP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}\n… (cut at {KEEP} bytes)", &s[..end]), true)
}

/// The real sandbox: Docker with gVisor's `runsc` runtime.
pub struct Runsc {
    image: String,
    live: Mutex<HashSet<Key>>,
    /// The internal network new sandboxes join once it checks out (slice
    /// 2); `None`: `--network none`. Fixed per container when it starts.
    network: Mutex<Option<String>>,
    /// Sandboxes that have taphttp's CA in place.
    with_ca: Mutex<HashSet<Key>>,
    /// The network to check and use, if one was configured.
    net_cfg: Option<NetCfg>,
}

/// Run `docker args`, its output, or why not.
async fn docker(args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "docker isn't available to spall: the sandbox runs where spall can start containers (spall on the host, not inside the compose image)".to_string()
            } else {
                format!("docker: {e}")
            }
        })
}

/// Whether the sandbox can run here, or why not: Docker reachable, `runsc`
/// a registered runtime, the image built. No side effects.
pub async fn check(image: &str) -> Result<(), String> {
    let out = docker(&["info", "--format", "{{json .Runtimes}}"]).await?;
    if !out.status.success() {
        return Err(format!(
            "Docker isn't reachable: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !String::from_utf8_lossy(&out.stdout).contains("\"runsc\"") {
        return Err("gVisor's runsc isn't a Docker runtime here: install it (https://gvisor.dev/docs/user_guide/install/), run `sudo runsc install`, restart Docker. spall won't fall back to a plain container".into());
    }
    let out = docker(&["image", "inspect", "--format", "{{.Id}}", image]).await?;
    if !out.status.success() {
        return Err(format!(
            "the sandbox image {image} isn't built: docker build -t {image} sandbox/"
        ));
    }
    Ok(())
}

/// Whether `network` is safe to put sandboxes on (slice 2), or why not:
/// it's `--internal` (no route out), and from a sandbox on it taphttp's
/// open listener (`fence`) doesn't answer while its credential-only one
/// (`proxy`) asks for a credential (407). Probed with a throwaway sandbox,
/// so what's checked is what a sandbox sees.
pub async fn check_network(
    image: &str,
    network: &str,
    proxy: &str,
    fence: &str,
) -> Result<(), String> {
    let out = docker(&["network", "inspect", "--format", "{{.Internal}}", network]).await?;
    if !out.status.success() {
        return Err(format!(
            "no Docker network {network}: create it with `docker network create --internal {network}` and attach taphttp"
        ));
    }
    if String::from_utf8_lossy(&out.stdout).trim() != "true" {
        return Err(format!(
            "the network {network} isn't --internal: a sandbox on it could route around taphttp"
        ));
    }
    let probe = format!(
        "curl -s -o /dev/null -m 5 -w '%{{http_code}}' -x http://{fence} http://probe.invalid/; echo \" $?\"; \
         curl -s -o /dev/null -m 5 -w '%{{http_code}}' -x http://{proxy} http://probe.invalid/; echo \" $?\""
    );
    let name = format!("spall-sbx-probe-{}", std::process::id());
    let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
    args.extend(run_flags(&name, &[("spall.probe", "1")], Some(network)));
    args.extend([image.to_string(), "sh".into(), "-c".into(), probe]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = docker(&refs).await?;
    judge_probe(&String::from_utf8_lossy(&out.stdout), network, proxy, fence).map_err(|e| {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.trim().is_empty() {
            e
        } else {
            format!("{e} ({})", err.trim())
        }
    })
}

/// The probe's two lines, each an HTTP code and curl's exit status. The
/// open listener must be shut (no HTTP answer, and a connection failure:
/// refused, timed out, closed or reset), the credential-only one must ask
/// for a credential (407). A name that doesn't resolve says nothing about
/// either, so it's its own error, never a shut door.
fn judge_probe(stdout: &str, network: &str, proxy: &str, fence: &str) -> Result<(), String> {
    let lines: Vec<(&str, &str)> = stdout
        .lines()
        .filter_map(|l| l.trim().split_once(' '))
        .collect();
    let [(open, open_exit), (auth, auth_exit)] = lines.as_slice() else {
        return Err("the network probe didn't run".into());
    };
    for (addr, exit) in [(fence, *open_exit), (proxy, *auth_exit)] {
        if exit == "5" {
            return Err(format!(
                "a sandbox on {network} can't resolve {addr}: give it an address there (gVisor sandboxes have no Docker DNS)"
            ));
        }
    }
    if *open != "000" {
        return Err(format!(
            "a sandbox on {network} reaches taphttp's open listener ({fence} answered {open}): fence it off (--allow-client) before sandboxes get a network"
        ));
    }
    if !matches!(*open_exit, "7" | "28" | "52" | "56") {
        return Err(format!(
            "the probe couldn't tell whether {fence} is shut to a sandbox on {network} (curl exit {open_exit})"
        ));
    }
    if *auth != "407" {
        return Err(format!(
            "taphttp's credential-only listener ({proxy}) answered {auth}, not 407: it must refuse a sandbox without a credential (taphttp#27 --auth-listen)"
        ));
    }
    Ok(())
}

/// `spall sandbox-shell`: a shell for the tester in a sandbox with exactly
/// the agent's flags (gVisor, no network, read-only, scratch, uid 10001,
/// the limits), to see what an agent's tools can and can't do. The host's
/// tool, never an agent's: an agent never gets a shell. Removed on exit.
pub async fn shell(image: &str, argv: &[String]) -> anyhow::Result<i32> {
    use std::io::IsTerminal;
    if let Err(why) = check(image).await {
        anyhow::bail!("sandbox-shell: {why}");
    }
    let name = format!("spall-sbx-shell-{}", std::process::id());
    let mut args: Vec<String> = vec!["run".into(), "--rm".into(), "-i".into()];
    if std::io::stdin().is_terminal() {
        args.push("-t".into());
    }
    args.extend(run_flags(&name, &[("spall.shell", "1")], None));
    args.push(image.to_string());
    if argv.is_empty() {
        args.push("sh".into());
    } else {
        args.extend(argv.iter().cloned());
    }
    eprintln!(
        "spall sandbox-shell: {image} under gVisor, as the agent gets it:\n  \
         no network (slice 1: HTTP through taphttp comes in slice 2), read-only root,\n  \
         /scratch (256 MB tmpfs, gone on exit), uid 10001, 1 CPU, 512 MB, 128 pids.\n  \
         Tools: curl jq python3 nmap openssl dig. exit to leave; the container is removed."
    );
    let status = tokio::process::Command::new("docker")
        .args(&args)
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("docker: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

impl Runsc {
    /// Check that the sandbox can run here, or say why not: Docker reachable,
    /// `runsc` a registered runtime, the image built. Clears leftovers from
    /// an earlier run (their tasks are gone with that run's process).
    pub async fn preflight(image: &str) -> Result<Runsc, String> {
        check(image).await?;
        let s = Runsc {
            image: image.to_string(),
            live: Mutex::new(HashSet::new()),
            network: Mutex::new(None),
            with_ca: Mutex::new(HashSet::new()),
            net_cfg: None,
        };
        s.clear_leftovers().await;
        Ok(s)
    }

    /// Give it a network to check (slice 2); without, it has none.
    pub fn with_network(mut self, cfg: Option<NetCfg>) -> Self {
        self.net_cfg = cfg;
        self
    }

    /// New sandboxes join `network` (checked by [`check_network`]), or none.
    fn set_network(&self, network: Option<String>) {
        *self.network.lock().unwrap_or_else(|p| p.into_inner()) = network;
    }

    fn ca_lock(&self) -> std::sync::MutexGuard<'_, HashSet<Key>> {
        self.with_ca.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Put taphttp's CA at [`CA_PATH`] in `key`'s sandbox, once.
    async fn install_ca(&self, key: &Key, name: &str, pem: &str) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        if self.ca_lock().contains(key) {
            return Ok(());
        }
        let mut child = tokio::process::Command::new("docker")
            .args(["exec", "-i", name, "sh", "-c", &format!("cat > {CA_PATH}")])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("docker: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(pem.as_bytes())
                .await
                .map_err(|e| format!("writing the CA: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| format!("docker: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "putting taphttp's CA in the sandbox: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        self.ca_lock().insert(key.clone());
        Ok(())
    }

    async fn clear_leftovers(&self) {
        let filter = format!("label={LABEL}");
        if let Ok(out) = docker(&["ps", "-aq", "--filter", &filter]).await {
            let ids: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .map(String::from)
                .collect();
            if !ids.is_empty() {
                let mut args = vec!["rm", "-f"];
                args.extend(ids.iter().map(String::as_str));
                let _ = docker(&args).await;
                tracing::info!("sandbox: removed {} leftover container(s)", ids.len());
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<Key>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start `key`'s container unless it's running.
    async fn ensure(&self, key: &Key) -> Result<String, String> {
        let name = container_name(key);
        if self.lock().contains(key) {
            return Ok(name);
        }
        let network = self
            .network
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut args: Vec<String> = vec!["run".into(), "-d".into()];
        args.extend(run_flags(
            &name,
            &[
                ("spall.engagement", &key.engagement),
                ("spall.task", &key.task),
            ],
            network.as_deref(),
        ));
        args.push(self.image.clone());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = docker(&refs).await?;
        if !out.status.success() {
            return Err(format!(
                "the sandbox didn't start: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        self.lock().insert(key.clone());
        Ok(name)
    }
}

impl Sandbox for Runsc {
    fn exec<'a>(
        &'a self,
        key: &'a Key,
        cmd: &'a Cmd,
        net: Option<&'a Net>,
    ) -> BoxFuture<'a, Result<Run, String>> {
        Box::pin(async move {
            let name = self.ensure(key).await?;
            if let Some(n) = net {
                self.install_ca(key, &name, &n.ca_pem).await?;
            }
            let secs = cmd.timeout.as_secs().to_string();
            // `timeout` runs inside the container, so a timed-out command
            // dies there (killing the outer `docker exec` wouldn't kill it).
            let mut args: Vec<&str> = vec!["exec", "-w", "/scratch"];
            let mut command = tokio::process::Command::new("docker");
            if let Some(n) = net {
                // `-e NAME` takes the value from docker's own environment:
                // the credential stays off the host's process list.
                for v in PROXY_VARS {
                    args.extend(["-e", v]);
                    command.env(v, &n.proxy_url);
                }
                for v in CA_VARS {
                    args.extend(["-e", v]);
                    command.env(v, CA_PATH);
                }
                args.extend(["-e", "NO_PROXY=", "-e", "no_proxy="]);
            }
            args.extend([name.as_str(), "timeout", "-s", "KILL", &secs]);
            args.extend(cmd.argv.iter().map(String::as_str));
            let start = Instant::now();
            let mut child = command
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("docker: {e}"))?;
            let (mut so, mut se) = (
                child.stdout.take().ok_or("no stdout")?,
                child.stderr.take().ok_or("no stderr")?,
            );
            let read = async {
                let (mut a, mut b) = (Vec::new(), Vec::new());
                let (mut so, mut se) = (
                    (&mut so).take(CAPTURE as u64),
                    (&mut se).take(CAPTURE as u64),
                );
                let _ = tokio::join!(so.read_to_end(&mut a), se.read_to_end(&mut b));
                (a, b)
            };
            // The outer clock is a backstop for a wedged `docker exec`.
            let outer = cmd.timeout + Duration::from_secs(10);
            let done = tokio::time::timeout(outer, async {
                let (a, b) = read.await;
                (child.wait().await, a, b)
            })
            .await;
            let ms = start.elapsed().as_millis() as u64;
            let (status, out, err) = match done {
                Ok((Ok(st), a, b)) => (Some(st), a, b),
                Ok((Err(e), _, _)) => return Err(format!("docker exec: {e}")),
                Err(_) => (
                    None,
                    Vec::new(),
                    b"(no answer from the sandbox; killed)".to_vec(),
                ),
            };
            let code = status.and_then(|s| s.code());
            // The sandbox itself died (a fork bomb or a memory blowup takes
            // the whole container down: contained, but gone). Forget it so
            // the next command starts a fresh one, and say so.
            let errs = String::from_utf8_lossy(&err);
            if code.is_some_and(|c| c != 0)
                && (errs.contains("is not running") || errs.contains("No such container"))
            {
                self.lock().remove(key);
                self.ca_lock().remove(key);
                let _ = docker(&["rm", "-f", &name]).await;
                return Ok(Run {
                    argv: cmd.argv.clone(),
                    exit: None,
                    ms,
                    stdout: String::new(),
                    stderr: "the sandbox died (it hit its memory or process limit); the next command starts a fresh one, and its scratch is gone".into(),
                    truncated: false,
                    timed_out: false,
                });
            }
            // `timeout -s KILL` ends a command with 137 (128 + SIGKILL).
            let timed_out = status.is_none()
                || (code == Some(137)
                    && start.elapsed() + Duration::from_millis(250) >= cmd.timeout);
            let (stdout, t1) = keep(&out);
            let (stderr, t2) = keep(&err);
            Ok(Run {
                argv: cmd.argv.clone(),
                exit: if timed_out { None } else { code },
                ms,
                stdout,
                stderr,
                truncated: t1 || t2,
                timed_out,
            })
        })
    }

    fn teardown<'a>(&'a self, key: &'a Key) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let had = self.lock().remove(key);
            self.ca_lock().remove(key);
            if had {
                let name = container_name(key);
                if let Err(e) = docker(&["rm", "-f", &name]).await {
                    tracing::warn!("sandbox: removing {name}: {e}");
                }
            }
        })
    }

    fn live(&self) -> Vec<Key> {
        let mut v: Vec<Key> = self.lock().iter().cloned().collect();
        v.sort();
        v
    }

    fn has_network(&self) -> bool {
        self.net_cfg.is_some()
    }

    fn check_network<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let Some(c) = &self.net_cfg else {
                return Ok(None);
            };
            check_network(&self.image, &c.network, &c.proxy, &c.fence).await?;
            self.set_network(Some(c.network.clone()));
            Ok(Some(c.proxy.clone()))
        })
    }
}

/// A scripted sandbox for tests: it honours the containment contract
/// without a kernel. `echo` prints its arguments; `fail` exits 1; writing
/// outside `/scratch` (`touch /etc/...`) is refused; `curl` has no route;
/// `block` runs until the sandbox is torn down; anything else isn't found.
#[derive(Default)]
pub struct Fake {
    pub ran: Mutex<Vec<(Key, Vec<String>)>>,
    /// The proxy URL each command got, if any.
    pub nets: Mutex<Vec<Option<String>>>,
    pub torn: Mutex<Vec<Key>>,
    live: Mutex<HashSet<Key>>,
    gone: tokio::sync::Notify,
}

impl Fake {
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Sandbox for Fake {
    fn exec<'a>(
        &'a self,
        key: &'a Key,
        cmd: &'a Cmd,
        net: Option<&'a Net>,
    ) -> BoxFuture<'a, Result<Run, String>> {
        Box::pin(async move {
            Self::lock(&self.live).insert(key.clone());
            Self::lock(&self.ran).push((key.clone(), cmd.argv.clone()));
            Self::lock(&self.nets).push(net.map(|n| n.proxy_url.clone()));
            let args = &cmd.argv[1..];
            let (exit, stdout, stderr) = match cmd.argv[0].as_str() {
                "echo" => (Some(0), format!("{}\n", args.join(" ")), String::new()),
                "fail" => (Some(1), String::new(), "failed\n".into()),
                "touch"
                    if args
                        .iter()
                        .any(|a| !a.starts_with("/scratch") && a.starts_with('/')) =>
                {
                    (
                        Some(1),
                        String::new(),
                        "touch: Read-only file system\n".into(),
                    )
                }
                "touch" => (Some(0), String::new(), String::new()),
                // With a network, through taphttp; without, no route.
                "curl" if net.is_some() => (Some(0), "via taphttp\n".into(), String::new()),
                "curl" => (
                    Some(6),
                    String::new(),
                    "curl: (6) Could not resolve host\n".into(),
                ),
                "block" => {
                    loop {
                        let wait = self.gone.notified();
                        if !Self::lock(&self.live).contains(key) {
                            break;
                        }
                        wait.await;
                    }
                    return Err("killed".into());
                }
                other => (Some(127), String::new(), format!("{other}: not found\n")),
            };
            Ok(Run {
                argv: cmd.argv.clone(),
                exit,
                ms: 1,
                stdout,
                stderr,
                truncated: false,
                timed_out: false,
            })
        })
    }

    fn teardown<'a>(&'a self, key: &'a Key) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if Self::lock(&self.live).remove(key) {
                Self::lock(&self.torn).push(key.clone());
            }
            self.gone.notify_waiters();
        })
    }

    fn live(&self) -> Vec<Key> {
        let mut v: Vec<Key> = Self::lock(&self.live).iter().cloned().collect();
        v.sort();
        v
    }

    fn has_network(&self) -> bool {
        false
    }

    fn check_network<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async { Ok(None) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: &str) -> Key {
        Key {
            engagement: "acme-1a2b".into(),
            task: t.into(),
        }
    }

    #[test]
    fn commands_are_argv_lists_within_limits() {
        assert!(Cmd::new(vec![], None).is_err());
        assert!(Cmd::new(vec!["".into()], None).is_err());
        assert!(Cmd::new(vec!["jq".into(), "a\0b".into()], None).is_err());
        assert!(Cmd::new(vec!["x".into(); MAX_ARGS + 1], None).is_err());
        assert!(Cmd::new(vec!["sleep".into()], Some(0)).is_err());
        assert!(Cmd::new(vec!["sleep".into()], Some(MAX_TIMEOUT + 1)).is_err());
        let c = Cmd::new(vec!["jq".into(), ".".into()], None).unwrap();
        assert_eq!(c.timeout, Duration::from_secs(DEFAULT_TIMEOUT));
    }

    #[test]
    fn names_and_flags_isolate_the_sandbox() {
        assert_eq!(container_name(&key("T-3")), "spall-sbx-acme-1a2b-t-3");
        assert_eq!(
            container_name(&Key {
                engagement: "../x y".into(),
                task: "T-1".into()
            }),
            "spall-sbx----x-y-t-1"
        );
        let f = run_flags("n", &[("spall.task", "T-3")], None).join(" ");
        for must in [
            "--runtime=runsc",
            "--network none",
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--user 10001:10001",
            "--pids-limit 128",
            "--memory 512m",
            "--label spall.sandbox=1",
            "--label spall.task=T-3",
        ] {
            assert!(f.contains(must), "{must} missing from {f}");
        }
        assert!(!f.contains("-v ") && !f.contains("--volume") && !f.contains("--privileged"));
        let g = run_flags("n", &[], Some("spall-sandbox")).join(" ");
        assert!(g.contains("--network spall-sandbox") && !g.contains("none"));
    }

    #[test]
    fn the_network_probe_wants_the_open_door_shut_and_the_locked_one_asking() {
        let j = |out: &str| judge_probe(out, "spall-sandbox", "taphttp:8090", "taphttp:8080");
        // Each line: the HTTP code, then curl's exit status.
        assert_eq!(j("000 52\n407 0\n"), Ok(()));
        for shut in ["000 7", "000 28", "000 52", "000 56"] {
            assert_eq!(j(&format!("{shut}\n407 0\n")), Ok(()), "{shut}");
        }
        assert!(j("502 0\n407 0\n")
            .unwrap_err()
            .contains("reaches taphttp's open listener"));
        // No answer because the name doesn't resolve proves nothing about
        // the fence (gVisor sandboxes have no Docker DNS): its own error.
        assert!(j("000 5\n407 0\n")
            .unwrap_err()
            .contains("can't resolve taphttp:8080"));
        assert!(j("000 52\n000 5\n")
            .unwrap_err()
            .contains("can't resolve taphttp:8090"));
        // Some other failure isn't a shut door either.
        assert!(j("000 6\n407 0\n").unwrap_err().contains("curl exit 6"));
        assert!(j("000 52\n000 52\n")
            .unwrap_err()
            .contains("answered 000, not 407"));
        assert!(j("000 52\n502 0\n").unwrap_err().contains("not 407"));
        assert!(j("").is_err());
        assert!(j("000\n407\n").is_err());
        assert_eq!(
            format!(
                "{:?}",
                Net {
                    proxy_url: "http://u:secret@x".into(),
                    ca_pem: String::new()
                }
            ),
            "Net { proxy_url: <secret>, .. }"
        );
    }

    #[test]
    fn credentials_are_escaped_in_the_proxy_url() {
        assert_eq!(userinfo("c-1", "a:b@c/d"), "c-1:a%3Ab%40c%2Fd");
    }

    #[test]
    fn long_output_is_cut_on_a_char_boundary() {
        let (s, cut) = keep("é".repeat(KEEP).as_bytes());
        assert!(cut && s.ends_with(&format!("(cut at {KEEP} bytes)")));
        assert_eq!(keep(b"short"), ("short".into(), false));
    }

    #[tokio::test]
    async fn the_fake_keeps_the_contract_and_dies_on_teardown() {
        let f = std::sync::Arc::new(Fake::default());
        let k = key("T-1");
        let run =
            |argv: &[&str]| Cmd::new(argv.iter().map(|s| s.to_string()).collect(), None).unwrap();
        assert_eq!(
            f.exec(&k, &run(&["echo", "hi"]), None)
                .await
                .unwrap()
                .stdout,
            "hi\n"
        );
        assert_eq!(
            f.exec(&k, &run(&["touch", "/etc/x"]), None)
                .await
                .unwrap()
                .exit,
            Some(1)
        );
        assert_eq!(
            f.exec(&k, &run(&["curl", "http://x"]), None)
                .await
                .unwrap()
                .exit,
            Some(6)
        );
        let (f2, k2) = (f.clone(), k.clone());
        let blocked = tokio::spawn(async move { f2.exec(&k2, &run(&["block"]), None).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        f.teardown(&k).await;
        assert_eq!(blocked.await.unwrap(), Err("killed".into()));
        assert!(f.live().is_empty());
        assert_eq!(*Fake::lock(&f.torn), vec![k]);
    }

    /// The network check for real (slice 2): a non-internal network is
    /// refused; an internal one with no credential-only listener on it is
    /// refused because nothing answers 407. With gVisor and the image:
    /// SPALL_TEST_RUNSC=1 cargo test --lib sandbox -- --include-ignored
    #[tokio::test]
    #[ignore]
    async fn the_network_check_refuses_what_isnt_fenced_for_real() {
        if std::env::var("SPALL_TEST_RUNSC").is_err() {
            return;
        }
        let (open, internal) = (
            format!("spall-test-open-{}", std::process::id()),
            format!("spall-test-int-{}", std::process::id()),
        );
        docker(&["network", "create", &open]).await.unwrap();
        docker(&["network", "create", "--internal", &internal])
            .await
            .unwrap();
        let a = check_network(IMAGE_DEFAULT, &open, "taphttp:8090", "taphttp:8080").await;
        // By name: a gVisor sandbox has no Docker DNS, and that's said.
        let b = check_network(IMAGE_DEFAULT, &internal, "taphttp:8090", "taphttp:8080").await;
        // By address, with nothing there: shut, but nothing asks for a
        // credential either.
        let d = check_network(IMAGE_DEFAULT, &internal, "192.0.2.1:8090", "192.0.2.1:8080").await;
        let c = check_network(
            IMAGE_DEFAULT,
            "no-such-net-spall",
            "taphttp:8090",
            "taphttp:8080",
        )
        .await;
        docker(&["network", "rm", &open, &internal]).await.unwrap();
        assert!(a.unwrap_err().contains("isn't --internal"));
        let b = b.unwrap_err();
        assert!(b.contains("can't resolve taphttp:8080"), "{b}");
        let d = d.unwrap_err();
        assert!(d.contains("not 407"), "{d}");
        assert!(c.unwrap_err().contains("no Docker network"));
    }

    /// The real thing, on a host with gVisor and the image built:
    /// SPALL_TEST_RUNSC=1 cargo test --lib sandbox -- --ignored
    #[tokio::test]
    #[ignore]
    async fn runsc_contains_a_command_for_real() {
        if std::env::var("SPALL_TEST_RUNSC").is_err() {
            return;
        }
        let s = Runsc::preflight(IMAGE_DEFAULT).await.expect("preflight");
        let k = key("T-999");
        let run = |argv: &[&str], t: u64| {
            Cmd::new(argv.iter().map(|s| s.to_string()).collect(), Some(t)).unwrap()
        };
        let r = s.exec(&k, &run(&["uname", "-r"], 20), None).await.unwrap();
        assert!(r.stdout.contains("gvisor"), "{r:?}");
        let r = s
            .exec(
                &k,
                &run(&["sh", "-c", "echo hi > /scratch/a && cat /scratch/a"], 20),
                None,
            )
            .await
            .unwrap();
        assert_eq!((r.exit, r.stdout.as_str()), (Some(0), "hi\n"));
        let r = s
            .exec(&k, &run(&["touch", "/etc/x"], 20), None)
            .await
            .unwrap();
        assert_ne!(r.exit, Some(0), "wrote outside scratch: {r:?}");
        let r = s
            .exec(
                &k,
                &run(&["curl", "-sS", "-m", "3", "http://example.com"], 20),
                None,
            )
            .await
            .unwrap();
        assert_ne!(r.exit, Some(0), "reached the network: {r:?}");
        let r = s.exec(&k, &run(&["sleep", "30"], 2), None).await.unwrap();
        assert!(r.timed_out && r.exit.is_none(), "{r:?}");
        // A fork bomb backgrounds itself and keeps forking until it hits the
        // pids and memory caps; under gVisor that takes the sandbox down,
        // never the host. spall says it died and starts a fresh one.
        let _ = s
            .exec(&k, &run(&["sh", "-c", ":(){ :|:& };:"], 3), None)
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut alive = false;
        for _ in 0..3 {
            let r = s
                .exec(&k, &run(&["echo", "alive"], 20), None)
                .await
                .unwrap();
            if r.stdout == "alive\n" {
                alive = true;
                break;
            }
            assert!(
                r.stderr.contains("sandbox died") || r.exit != Some(0),
                "{r:?}"
            );
        }
        assert!(alive, "no fresh sandbox after the bomb");
        s.teardown(&k).await;
        let out = docker(&[
            "ps",
            "-aq",
            "--filter",
            &format!("name={}", container_name(&k)),
        ])
        .await
        .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            "container still there"
        );
    }
}
