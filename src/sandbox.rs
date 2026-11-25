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
use serde::Serialize;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

pub trait Sandbox: Send + Sync {
    /// Run `cmd` in `key`'s sandbox, starting it first if needed.
    fn exec<'a>(&'a self, key: &'a Key, cmd: &'a Cmd) -> BoxFuture<'a, Result<Run, String>>;
    /// Kill and remove `key`'s sandbox and its scratch; nothing if it has none.
    fn teardown<'a>(&'a self, key: &'a Key) -> BoxFuture<'a, ()>;
    /// The sandboxes running now.
    fn live(&self) -> Vec<Key>;
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
pub fn run_flags(name: &str, labels: &[(&str, &str)]) -> Vec<String> {
    let mut v: Vec<String> = [
        "--name",
        name,
        "--runtime=runsc",
        "--network",
        "none",
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
    args.extend(run_flags(&name, &[("spall.shell", "1")]));
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
        };
        s.clear_leftovers().await;
        Ok(s)
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
        let mut args: Vec<String> = vec!["run".into(), "-d".into()];
        args.extend(run_flags(
            &name,
            &[
                ("spall.engagement", &key.engagement),
                ("spall.task", &key.task),
            ],
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
    fn exec<'a>(&'a self, key: &'a Key, cmd: &'a Cmd) -> BoxFuture<'a, Result<Run, String>> {
        Box::pin(async move {
            let name = self.ensure(key).await?;
            let secs = cmd.timeout.as_secs().to_string();
            // `timeout` runs inside the container, so a timed-out command
            // dies there (killing the outer `docker exec` wouldn't kill it).
            let mut args: Vec<&str> = vec![
                "exec", "-w", "/scratch", &name, "timeout", "-s", "KILL", &secs,
            ];
            args.extend(cmd.argv.iter().map(String::as_str));
            let start = Instant::now();
            let mut child = tokio::process::Command::new("docker")
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
}

/// A scripted sandbox for tests: it honours the containment contract
/// without a kernel. `echo` prints its arguments; `fail` exits 1; writing
/// outside `/scratch` (`touch /etc/...`) is refused; `curl` has no route;
/// `block` runs until the sandbox is torn down; anything else isn't found.
#[derive(Default)]
pub struct Fake {
    pub ran: Mutex<Vec<(Key, Vec<String>)>>,
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
    fn exec<'a>(&'a self, key: &'a Key, cmd: &'a Cmd) -> BoxFuture<'a, Result<Run, String>> {
        Box::pin(async move {
            Self::lock(&self.live).insert(key.clone());
            Self::lock(&self.ran).push((key.clone(), cmd.argv.clone()));
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
        let f = run_flags("n", &[("spall.task", "T-3")]).join(" ");
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
            f.exec(&k, &run(&["echo", "hi"])).await.unwrap().stdout,
            "hi\n"
        );
        assert_eq!(
            f.exec(&k, &run(&["touch", "/etc/x"])).await.unwrap().exit,
            Some(1)
        );
        assert_eq!(
            f.exec(&k, &run(&["curl", "http://x"])).await.unwrap().exit,
            Some(6)
        );
        let (f2, k2) = (f.clone(), k.clone());
        let blocked = tokio::spawn(async move { f2.exec(&k2, &run(&["block"])).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        f.teardown(&k).await;
        assert_eq!(blocked.await.unwrap(), Err("killed".into()));
        assert!(f.live().is_empty());
        assert_eq!(*Fake::lock(&f.torn), vec![k]);
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
        let r = s.exec(&k, &run(&["uname", "-r"], 20)).await.unwrap();
        assert!(r.stdout.contains("gvisor"), "{r:?}");
        let r = s
            .exec(
                &k,
                &run(&["sh", "-c", "echo hi > /scratch/a && cat /scratch/a"], 20),
            )
            .await
            .unwrap();
        assert_eq!((r.exit, r.stdout.as_str()), (Some(0), "hi\n"));
        let r = s.exec(&k, &run(&["touch", "/etc/x"], 20)).await.unwrap();
        assert_ne!(r.exit, Some(0), "wrote outside scratch: {r:?}");
        let r = s
            .exec(
                &k,
                &run(&["curl", "-sS", "-m", "3", "http://example.com"], 20),
            )
            .await
            .unwrap();
        assert_ne!(r.exit, Some(0), "reached the network: {r:?}");
        let r = s.exec(&k, &run(&["sleep", "30"], 2)).await.unwrap();
        assert!(r.timed_out && r.exit.is_none(), "{r:?}");
        // A fork bomb backgrounds itself and keeps forking until it hits the
        // pids and memory caps; under gVisor that takes the sandbox down,
        // never the host. spall says it died and starts a fresh one.
        let _ = s.exec(&k, &run(&["sh", "-c", ":(){ :|:& };:"], 3)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let mut alive = false;
        for _ in 0..3 {
            let r = s.exec(&k, &run(&["echo", "alive"], 20)).await.unwrap();
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
