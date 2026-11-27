//! Roles (#64): Scoper, Mapper, Tester, Reporter, each a least-privilege
//! permission set over the registry's risk classes, plus a prompt and the
//! skills it loads.
//!
//! Permissions are code, here: [`Role::may`] is what the agent's tools
//! (`agent::Tools::call`) and the gate (`replay::gated_send`) consult. The
//! role and skill files under `src/content/` are text: they say how to work
//! and are compiled in (spall-shipped, vetted), and their frontmatter must
//! agree with this module or a test fails. Text never grants.
//!
//! A role caps a task below its constraints and the RoE: a send the role
//! can't make is refused, never paused, since no approval widens a role.
//! The content-pack loader (sources, licenses, packs) is #31's (M9); M6
//! ships built-ins only.

use crate::commands::{Command, Risk};
use crate::tasks::Role;

/// A built-in skill: `SKILL.md` with `name` and `description` frontmatter.
pub struct Skill {
    pub name: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

/// A built-in role file: `name`, `description`, `risks`, `skills`
/// frontmatter, then the prompt.
pub struct RoleFile {
    pub name: &'static str,
    pub description: &'static str,
    pub risks: Vec<&'static str>,
    pub skills: Vec<&'static str>,
    pub prompt: &'static str,
}

const ROLE_FILES: [(Role, &str); 4] = [
    (Role::Scoper, include_str!("content/roles/scoper.md")),
    (Role::Mapper, include_str!("content/roles/mapper.md")),
    (Role::Tester, include_str!("content/roles/tester.md")),
    (Role::Reporter, include_str!("content/roles/reporter.md")),
];

const SKILL_FILES: [&str; 7] = [
    include_str!("content/skills/scope-review/SKILL.md"),
    include_str!("content/skills/map-surface/SKILL.md"),
    include_str!("content/skills/test-authz/SKILL.md"),
    include_str!("content/skills/test-injection/SKILL.md"),
    include_str!("content/skills/retest/SKILL.md"),
    include_str!("content/skills/writeup/SKILL.md"),
    include_str!("content/skills/proof-of-concept/SKILL.md"),
];

/// `---\nkey: value\n...\n---\nbody`, as (fields, body). Only the simple
/// one-line `key: value` form the built-ins use.
fn front(src: &'static str) -> (Vec<(&'static str, &'static str)>, &'static str) {
    let Some(rest) = src.strip_prefix("---\n") else {
        return (Vec::new(), src);
    };
    let Some(end) = rest.find("\n---\n") else {
        return (Vec::new(), src);
    };
    let fields = rest[..end]
        .lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    (fields, rest[end + 5..].trim())
}

fn field(fields: &[(&'static str, &'static str)], k: &str) -> &'static str {
    fields
        .iter()
        .find(|(n, _)| *n == k)
        .map(|(_, v)| *v)
        .unwrap_or("")
}

fn list(v: &'static str) -> Vec<&'static str> {
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Every built-in skill.
pub fn skills() -> Vec<Skill> {
    SKILL_FILES
        .iter()
        .map(|src| {
            let (f, body) = front(src);
            Skill {
                name: field(&f, "name"),
                description: field(&f, "description"),
                body,
            }
        })
        .collect()
}

pub fn skill(name: &str) -> Option<Skill> {
    skills().into_iter().find(|s| s.name == name)
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Scoper, Role::Mapper, Role::Tester, Role::Reporter];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Scoper => "scoper",
            Role::Mapper => "mapper",
            Role::Tester => "tester",
            Role::Reporter => "reporter",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// The risk classes this role may use. Policy and export are never
    /// among them (and `commands::admit` refuses them first anyway).
    pub fn risks(self) -> &'static [Risk] {
        match self {
            Role::Scoper | Role::Reporter => &[Risk::Read, Risk::Record],
            Role::Mapper => &[Risk::Read, Risk::Record, Risk::Send],
            Role::Tester => &[
                Risk::Read,
                Risk::Record,
                Risk::Send,
                Risk::Unsafe,
                Risk::Destructive,
            ],
        }
    }

    /// Whether this role may use `risk` at all.
    pub fn may(self, risk: Risk) -> bool {
        risk.agent_may() && self.risks().contains(&risk)
    }

    /// Whether this role may run `cmd`; `Err` says why not, for the model.
    /// A Scoper reads and proposes scope, so it doesn't propose findings.
    pub fn may_run(self, cmd: &Command) -> Result<(), String> {
        if !self.may(cmd.risk) {
            return Err(format!(
                "a {} can't run {} ({} commands aren't in its role): leave it for another role and say so in your report",
                self.as_str(),
                cmd.name,
                cmd.risk.as_str()
            ));
        }
        if self == Role::Scoper && cmd.name == "propose_finding" {
            return Err(
                "a scoper doesn't propose findings: put what you saw in your report for a Tester"
                    .into(),
            );
        }
        Ok(())
    }

    /// Whether this role may send a request whose method the engagement's
    /// rules treat as safe (`safe`) or not; `Err` is a refusal no approval
    /// changes.
    pub fn may_send(self, method: &str, safe: bool) -> Result<(), String> {
        if !self.may(Risk::Send) {
            return Err(format!(
                "a {} sends nothing: that's another role's work",
                self.as_str()
            ));
        }
        if !safe && !self.may(Risk::Unsafe) {
            return Err(format!(
                "a {} sends only the methods the rules of engagement treat as safe, not {method}: that's a Tester's work",
                self.as_str()
            ));
        }
        Ok(())
    }

    /// Whether a task for this role may be given destructive actions.
    pub fn may_destroy(self) -> bool {
        self.may(Risk::Destructive)
    }

    /// The role's file: its prompt and skills.
    pub fn file(self) -> RoleFile {
        let src = ROLE_FILES
            .iter()
            .find(|(r, _)| *r == self)
            .map(|(_, s)| *s)
            .unwrap_or("");
        let (f, prompt) = front(src);
        RoleFile {
            name: field(&f, "name"),
            description: field(&f, "description"),
            risks: list(field(&f, "risks")),
            skills: list(field(&f, "skills")),
            prompt,
        }
    }

    /// What the agent is told about its role: the prompt, then its skills
    /// by name and description (the `skill` tool loads one).
    pub fn brief(self) -> String {
        let f = self.file();
        let mut s = format!("Your role: {}.\n{}\n\nYour skills (load one with skill <name> before you start that kind of work):", self.as_str(), f.prompt);
        for name in &f.skills {
            if let Some(k) = skill(name) {
                s.push_str(&format!("\n- {}: {}", k.name, k.description));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_against_every_risk_class() {
        use Risk::*;
        // The table in #64 and docs/ux.md (Roles).
        let table: [(Role, [bool; 7]); 4] = [
            //               read  record send   unsafe destr. policy export
            (
                Role::Scoper,
                [true, true, false, false, false, false, false],
            ),
            (Role::Mapper, [true, true, true, false, false, false, false]),
            (Role::Tester, [true, true, true, true, true, false, false]),
            (
                Role::Reporter,
                [true, true, false, false, false, false, false],
            ),
        ];
        let order = [Read, Record, Send, Unsafe, Destructive, Policy, Export];
        for (role, row) in table {
            for (risk, want) in order.iter().zip(row) {
                assert_eq!(role.may(*risk), want, "{role:?} {risk:?}");
            }
        }
    }

    #[test]
    fn sends_by_role() {
        assert!(Role::Scoper.may_send("GET", true).is_err());
        assert!(Role::Reporter.may_send("GET", true).is_err());
        assert!(Role::Mapper.may_send("GET", true).is_ok());
        let e = Role::Mapper.may_send("POST", false).unwrap_err();
        assert!(e.contains("Tester"), "{e}");
        assert!(Role::Tester.may_send("POST", false).is_ok());
        assert!(Role::Tester.may_destroy() && !Role::Mapper.may_destroy());
    }

    #[test]
    fn commands_by_role() {
        let find = |n| crate::commands::find(n).unwrap();
        for r in Role::ALL {
            assert!(r.may_run(find("filter")).is_ok(), "{r:?}");
            assert!(r.may_run(find("report")).is_ok(), "{r:?}");
            assert!(r.may_run(find("export")).is_err(), "{r:?}");
            assert!(r.may_run(find("scope")).is_err(), "{r:?}");
        }
        assert!(Role::Scoper.may_run(find("propose_finding")).is_err());
        assert!(Role::Reporter.may_run(find("propose_finding")).is_ok());
        assert!(Role::Reporter.may_run(find("replay")).is_err());
        assert!(Role::Scoper.may_run(find("fetch")).is_err());
        assert!(Role::Mapper.may_run(find("fetch")).is_ok());
    }

    #[test]
    fn role_files_agree_with_the_code() {
        for r in Role::ALL {
            let f = r.file();
            assert_eq!(f.name, r.as_str());
            assert!(!f.description.is_empty() && !f.prompt.is_empty(), "{r:?}");
            let risks: Vec<&str> = r.risks().iter().map(|k| k.as_str()).collect();
            assert_eq!(f.risks, risks, "{r:?}: the file's risks aren't the code's");
            assert!(!f.skills.is_empty());
            for s in &f.skills {
                assert!(
                    skill(s).is_some(),
                    "{r:?} names a skill that isn't built in: {s}"
                );
            }
            assert!(r.brief().contains(f.skills[0]));
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn skills_have_names_descriptions_and_bodies() {
        let all = skills();
        assert_eq!(all.len(), SKILL_FILES.len());
        for s in &all {
            assert!(
                !s.name.is_empty() && !s.description.is_empty() && s.body.starts_with('#'),
                "{}",
                s.name
            );
        }
        let mut names: Vec<&str> = all.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "skill names are unique");
        // Every skill is some role's.
        for s in &all {
            assert!(
                Role::ALL.iter().any(|r| r.file().skills.contains(&s.name)),
                "{} belongs to no role",
                s.name
            );
        }
    }
}
