//! The tool registry (#13 slice 4b, docs/sandbox.md "Tools: toolpacks, not a
//! distro"): which sandbox tools belong to which toolpack, and their
//! *category*, decided by the program's name, never by matching its
//! arguments. Containment (gVisor + taphttp on the command's credential) is
//! what keeps a tool safe; the category decides who says yes:
//!
//! - **recon / scanner** run under the task like any sandbox command.
//! - **exploitation** (sqlmap, hydra) pauses the task for the tester, who
//!   sees the exact argv and approves it once.
//! - a **destructive** tool (brute force) additionally needs the rules of
//!   engagement's `destructive_allowed`.
//!
//! Base-image tools (`curl`, `jq`, `python3`, `nmap`, `openssl`, `dig`) are
//! not listed: they need no pack and run as recon. This table is the vetted
//! set; a tester's own image is a separate, pinned opt-in (a `policy`
//! action, docs/sandbox.md "Bring your own pack").

use serde::Serialize;

/// Who says yes to a tool, by its family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Enumeration and discovery; runs under the task.
    Recon,
    /// Active scanning; runs under the task.
    Scanner,
    /// Exploitation; pauses the task for the tester.
    Exploitation,
}

/// One vetted tool: its program name, the toolpack image family that ships
/// it, its category, and whether it is destructive (needs the RoE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Tool {
    pub name: &'static str,
    /// The toolpack that ships it (`spall-pack-<pack>`), and the config name
    /// a tester enables with `--sandbox-pack`.
    pub pack: &'static str,
    pub category: Category,
    /// Destructive by nature (brute force): also needs `destructive_allowed`.
    pub destructive: bool,
}

/// The vetted registry. Cheapest-trust first within a pack.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "ffuf",
        pack: "fuzz",
        category: Category::Scanner,
        destructive: false,
    },
    Tool {
        name: "feroxbuster",
        pack: "fuzz",
        category: Category::Scanner,
        destructive: false,
    },
    Tool {
        name: "nuclei",
        pack: "scan",
        category: Category::Scanner,
        destructive: false,
    },
    Tool {
        name: "wpscan",
        pack: "scan",
        category: Category::Scanner,
        destructive: false,
    },
    Tool {
        name: "sqlmap",
        pack: "sqli",
        category: Category::Exploitation,
        destructive: false,
    },
    Tool {
        name: "hydra",
        pack: "creds",
        category: Category::Exploitation,
        destructive: true,
    },
    Tool {
        name: "ncrack",
        pack: "creds",
        category: Category::Exploitation,
        destructive: true,
    },
];

/// The vetted tool `arg0` names (its basename), if any. A path or an
/// absolute spelling resolves to the same tool, so `/usr/bin/sqlmap` and
/// `sqlmap` are one.
pub fn classify(arg0: &str) -> Option<&'static Tool> {
    let base = arg0.rsplit('/').next().unwrap_or(arg0);
    TOOLS.iter().find(|t| t.name == base)
}

/// Every distinct pack name in the registry (for help and config checks).
pub fn packs() -> Vec<&'static str> {
    let mut v: Vec<&str> = TOOLS.iter().map(|t| t.pack).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_is_by_basename_and_never_by_arguments() {
        // A known tool, however it's spelled.
        assert_eq!(classify("sqlmap").unwrap().category, Category::Exploitation);
        assert_eq!(classify("/usr/bin/sqlmap").unwrap().pack, "sqli");
        assert!(classify("hydra").unwrap().destructive);
        assert_eq!(classify("ffuf").unwrap().category, Category::Scanner);
        assert!(!classify("ffuf").unwrap().destructive);
        // Base-image tools and scripts are unlisted (run as recon).
        for t in [
            "curl", "jq", "python3", "nmap", "openssl", "dig", "sh", "./x.py",
        ] {
            assert!(classify(t).is_none(), "{t}");
        }
        // The name decides it, not an argument that mentions a tool.
        assert!(classify("curl sqlmap").is_none());
    }

    #[test]
    fn packs_are_the_distinct_families() {
        let p = packs();
        for t in TOOLS {
            assert!(p.contains(&t.pack), "{}", t.pack);
        }
        // Names are plausible pack slugs.
        assert!(p.iter().all(|s| s.chars().all(|c| c.is_ascii_lowercase())));
    }
}
