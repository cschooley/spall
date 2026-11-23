//! The command registry (#53): one server-side list of what can be done,
//! as data. The palette and console (ui/src/commands/registry.ts), the MCP
//! server (#58) and spall's own agent host (#63) all take their commands
//! from here: stable name, argument schema (JSON Schema, which MCP reuses
//! verbatim as a tool's `inputSchema`), description, risk class and
//! whether an agent may be offered it.
//!
//! The UI keeps its own `run`/`complete` code but takes everything else
//! from `ui/src/commands/specs.gen.ts`, which this module writes (see the
//! `ui_specs_are_current` test); a UI test fails if a command exists on one
//! side and not the other.
//!
//! The `mcp` flag says what is *offered* to an agent. What an agent may
//! *run* is decided by [`admit`], which refuses the agent-forbidden risk
//! classes (policy, export: promotion included) whatever the flag says, so
//! flipping a flag by mistake can't hand an agent the rules or the export.

use crate::audit::{AuditLog, Outcome, Record};
use crate::roe::{Actor, Class};
use serde::Serialize;
use serde_json::{json, Value};

/// What running a command can do. Every command has exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads spall's state or changes only the UI.
    Read,
    /// Changes spall's own records (candidates); sends nothing.
    Record,
    /// Sends traffic to a target, through the gate.
    Send,
    /// Sends traffic that could change state there (an unsafe method).
    Unsafe,
    /// Fuzzing, brute force, anything that could break the target.
    Destructive,
    /// Changes the scope, the engagement or its rules. Never an agent's.
    Policy,
    /// Promotes findings or writes them out of spall. Never an agent's.
    Export,
}

impl Risk {
    pub const ALL: [Risk; 7] = [
        Risk::Read,
        Risk::Record,
        Risk::Send,
        Risk::Unsafe,
        Risk::Destructive,
        Risk::Policy,
        Risk::Export,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Record => "record",
            Risk::Send => "send",
            Risk::Unsafe => "unsafe",
            Risk::Destructive => "destructive",
            Risk::Policy => "policy",
            Risk::Export => "export",
        }
    }

    /// The gate's class for a command that reaches a target; `None` for one
    /// that doesn't. Unsafe is a send whose method the rules of engagement
    /// treat as unsafe, so the gate's method policy decides confirmation.
    pub fn class(self) -> Option<Class> {
        match self {
            Risk::Send | Risk::Unsafe => Some(Class::Send),
            Risk::Destructive => Some(Class::Destructive),
            Risk::Read | Risk::Record | Risk::Policy | Risk::Export => None,
        }
    }

    /// Whether an agent may ever run a command of this class. The agent
    /// proposes, a human promotes; and only a human sets the rules.
    pub fn agent_may(self) -> bool {
        !matches!(self, Risk::Policy | Risk::Export)
    }
}

/// One command, as data.
pub struct Command {
    /// Stable identifier, shared by the console, MCP and the agent host.
    pub name: &'static str,
    /// The console syntax.
    pub usage: &'static str,
    pub desc: &'static str,
    pub risk: Risk,
    /// May be offered to an agent (see [`admit`] for what it may run).
    pub mcp: bool,
    /// Left out of help, completion and anything offered to an agent.
    pub hidden: bool,
    /// The palette and console have it. Off only for an agent's own tool
    /// (`propose_finding`; a human flags instead).
    pub ui: bool,
    /// The arguments as a JSON Schema object.
    args: fn() -> Value,
}

impl Command {
    pub fn args(&self) -> Value {
        (self.args)()
    }

    /// What `GET /api/commands` and the generated UI specs carry.
    pub fn spec(&self) -> Value {
        json!({
            "name": self.name,
            "usage": self.usage,
            "desc": self.desc,
            "risk": self.risk,
            "mcp": self.mcp,
            "hidden": self.hidden,
            "ui": self.ui,
            "args": self.args(),
        })
    }
}

/// An object schema: `props` as `(name, schema)`, `required` by name.
fn obj(props: &[(&str, Value)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = props
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn none() -> Value {
    obj(&[], &[])
}

fn text(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn flag(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

fn one_of(values: &[&str], desc: &str) -> Value {
    json!({"type": "string", "enum": values, "description": desc})
}

/// An exchange or candidate id, as the UI's `ID` checks it.
fn id(desc: &str) -> Value {
    json!({"type": "string", "pattern": "^[A-Za-z0-9_-]{1,128}$", "description": desc})
}

const VIEWS: [&str; 5] = ["engagement", "traffic", "surface", "tasks", "findings"];
const THEMES: [&str; 4] = ["rust", "rust-light", "blued", "blued-light"];

/// Every command, in help order.
pub static COMMANDS: &[Command] = &[
    Command {
        name: "help",
        usage: "help",
        desc: "this list",
        // tools/list is an agent's help.
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "filter",
        usage: "filter [query]",
        desc: "filter traffic (host: method: status: text); empty clears",
        risk: Risk::Read,
        mcp: true,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[(
                    "query",
                    text("filter syntax: host: method: status: and free text"),
                )],
                &[],
            )
        },
    },
    Command {
        name: "open",
        usage: "open <id>",
        desc: "open an exchange in the inspector",
        risk: Risk::Read,
        mcp: true,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("id", id("the exchange id")),
                    (
                        "body_offset",
                        json!({"type": "integer", "minimum": 0, "description": "agents: the first body byte to show (bodies show 4 KiB by default)"}),
                    ),
                    (
                        "body_length",
                        json!({"type": "integer", "minimum": 1, "maximum": 65536, "description": "agents: how many body bytes to show"}),
                    ),
                ],
                &["id"],
            )
        },
    },
    Command {
        name: "curl",
        usage: "curl",
        desc: "the selected exchange as a curl command (also copied)",
        // The clipboard is the tester's; an agent reads exchanges with open.
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[("id", id("the exchange id; default: the selected one"))],
                &[],
            )
        },
    },
    Command {
        name: "replay",
        usage: "replay <id> [--yes]",
        desc: "resend an exchange (edits: --method M --header 'N: v' --drop-header N --body B; unsafe methods: run twice, or --yes)",
        // Agents replay inside a task's constraints (#55); the gate holds
        // them to the same scope, method policy and audit as a human.
        risk: Risk::Send,
        mcp: true,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("id", id("the exchange to resend")),
                    ("method", text("replace the method")),
                    (
                        "headers",
                        json!({
                            "type": "array",
                            "description": "replace every value of each named header",
                            "items": obj(&[("name", text("header name")), ("value", text("header value"))], &["name", "value"]),
                        }),
                    ),
                    (
                        "drop_headers",
                        json!({"type": "array", "items": {"type": "string"}, "description": "remove every value of each named header"}),
                    ),
                    ("body", text("replace the body")),
                    (
                        "yes",
                        flag("confirm an unsafe method now instead of arming it"),
                    ),
                ],
                &["id"],
            )
        },
    },
    Command {
        name: "scope",
        usage: "scope [add|exclude|remove <entry> | check <url|host> | paste | import | all on|off]",
        desc: "hosts replay may send to; exclusions (!entry) win; changes last until restart",
        // Changing the scope changes the rules of engagement: humans only.
        // Agents read scope state through `hosts`.
        risk: Risk::Policy,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    (
                        "op",
                        one_of(
                            &[
                                "show", "add", "exclude", "remove", "check", "paste", "import",
                                "all",
                            ],
                            "default: show",
                        ),
                    ),
                    (
                        "entry",
                        text("a host, *.domain, URL prefix or CIDR; a URL or host for check"),
                    ),
                    ("on", flag("for all: allow every host")),
                ],
                &[],
            )
        },
    },
    Command {
        name: "engagement",
        usage: "engagement [list | new <name> | use <id|name> | set <field> <value>]",
        desc: "the engagement you're testing under: details, and switching between engagements",
        // Creating, switching and editing engagements set the rules: humans only.
        risk: Risk::Policy,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    (
                        "op",
                        one_of(&["show", "list", "new", "use", "set"], "default: show"),
                    ),
                    ("name", text("for new: the engagement's name")),
                    ("which", text("for use: an engagement id or name")),
                    (
                        "field",
                        one_of(
                            &["name", "client", "starts", "ends", "expires", "notes"],
                            "for set",
                        ),
                    ),
                    ("value", text("for set; empty clears a date")),
                ],
                &[],
            )
        },
    },
    Command {
        name: "roe",
        usage: "roe [set <field> <value> | window add|remove|clear | blackout add|remove|clear]",
        desc: "rules of engagement: methods, confirmation, rate, time zone, testing and no-test windows, agents",
        // The rules bound every action, a human's or an agent's: humans only.
        risk: Risk::Policy,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    (
                        "op",
                        one_of(&["show", "set", "window", "blackout"], "default: show"),
                    ),
                    (
                        "field",
                        one_of(
                            &["methods", "safe", "rate", "tz", "agents", "destructive"],
                            "for set",
                        ),
                    ),
                    ("value", text("for set")),
                    (
                        "action",
                        one_of(&["add", "remove", "clear"], "for window and blackout"),
                    ),
                    ("days", text("for window add: like mon-fri")),
                    ("hours", text("for window add: HH:MM-HH:MM")),
                    ("from", text("for blackout add: YYYY-MM-DDTHH:MM")),
                    ("to", text("for blackout add: YYYY-MM-DDTHH:MM")),
                    ("note", text("for blackout add")),
                    (
                        "n",
                        json!({"type": "integer", "minimum": 1, "description": "for remove: which one, from 1"}),
                    ),
                ],
                &[],
            )
        },
    },
    Command {
        name: "estop",
        usage: "estop [reason] | estop clear",
        desc: "agent e-stop (Shift+Esc): halt every agent task for good; clear lets new tasks run",
        // Clearing it is a human's call; an agent is never offered either.
        risk: Risk::Policy,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("clear", flag("clear the stop instead (halted tasks stay halted)")),
                    ("reason", text("why, for the audit log")),
                ],
                &[],
            )
        },
    },
    Command {
        name: "hosts",
        usage: "hosts [--unknown]",
        desc: "hosts seen in traffic and whether each is in scope, excluded or undecided",
        // Reading scope state is fine for an agent; changing it never is.
        risk: Risk::Read,
        mcp: true,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[(
                    "unknown",
                    flag("only hosts neither in scope nor excluded"),
                )],
                &[],
            )
        },
    },
    Command {
        name: "flag",
        usage: "flag <exchange-id> --rule <id> [--param P] [--level L] [--rank N] [--message M] [--replay <id>]... [--cwe N]",
        desc: "flag an exchange as a candidate finding (rules lists rule ids); it waits in Findings for triage",
        // Agents propose through propose_finding (#60), never flag.
        risk: Risk::Record,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("exchange_id", id("the exchange the finding is about")),
                    ("rule", text("a rule id from rules, like WSTG-INPV-05")),
                    ("param", text("the parameter or location")),
                    (
                        "level",
                        one_of(&["error", "warning", "note", "none"], "SARIF level"),
                    ),
                    (
                        "rank",
                        json!({"type": "number", "minimum": 0, "maximum": 100, "description": "SARIF rank"}),
                    ),
                    ("message", text("what was found")),
                    (
                        "replay",
                        json!({"type": "array", "items": id("a replay of the exchange"), "description": "replays as evidence"}),
                    ),
                    (
                        "cwe",
                        json!({"type": "integer", "minimum": 1, "description": "override the rule's CWE"}),
                    ),
                ],
                &["exchange_id", "rule"],
            )
        },
    },
    Command {
        name: "promote",
        usage: "promote <candidate-id> [--undo]",
        desc: "mark a confirmed candidate for export (a human's call), or take it out",
        risk: Risk::Export,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("candidate_id", id("the candidate")),
                    ("undo", flag("take it out of the next export")),
                ],
                &["candidate_id"],
            )
        },
    },
    Command {
        name: "export",
        usage: "export [--unredacted]",
        desc: "save the promoted findings as a flux (SARIF) report, redacted; --unredacted needs spall's --allow-unredacted-export",
        risk: Risk::Export,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[(
                    "unredacted",
                    flag("skip redaction (needs --allow-unredacted-export)"),
                )],
                &[],
            )
        },
    },
    Command {
        name: "rules",
        usage: "rules [text]",
        desc: "the rule catalog (WSTG v4.2 tests, with CWE): every rule, or those matching text",
        risk: Risk::Read,
        mcp: true,
        hidden: false,
        ui: true,
        args: || obj(&[("text", text("match id, title or CWE"))], &[]),
    },
    Command {
        name: "task",
        usage: "task [id]",
        desc: "a task: goal, status, constraints, spend, and what it waits on (an agent sees only its own)",
        risk: Risk::Read,
        mcp: true,
        hidden: false,
        ui: true,
        args: || obj(&[("id", text("the task, like T-3; an agent's default and only choice is its own"))], &[]),
    },
    Command {
        name: "halt",
        usage: "halt <T-n> [reason]",
        desc: "stop one agent task for good (estop stops them all)",
        // A human's stop; an agent never halts another task.
        risk: Risk::Record,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[
                    ("id", text("the task, like T-3")),
                    ("reason", text("why, kept with the task")),
                ],
                &["id"],
            )
        },
    },
    Command {
        name: "propose_finding",
        usage: "propose_finding",
        desc: "propose a candidate finding with evidence from this task's own traffic; a human triages and promotes it",
        // The agent's one write into findings (#60): it proposes, a human
        // decides. The tester flags instead.
        risk: Risk::Record,
        mcp: true,
        hidden: false,
        ui: false,
        args: || {
            obj(
                &[
                    ("exchange_id", id("the exchange that shows it: one this task sent or read")),
                    ("rule", text("a rule id from rules, like WSTG-INPV-05")),
                    ("message", text("what was found, for the tester")),
                    ("parameter", text("the parameter or location, if any")),
                    (
                        "level",
                        one_of(&["error", "warning", "note", "none"], "SARIF level"),
                    ),
                    (
                        "evidence",
                        json!({"type": "array", "items": id("an exchange"), "description": "more exchanges (replays) backing it: ones this task sent or read"}),
                    ),
                ],
                &["exchange_id", "rule", "message"],
            )
        },
    },
    Command {
        name: "ca",
        usage: "ca",
        desc: "taphttp's CA certificate: fingerprint, download, and browser setup steps",
        // Browser setup is a human's job.
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "audit",
        usage: "audit [--last N]",
        desc: "recent replay attempts, denials included",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[(
                    "last",
                    json!({"type": "integer", "minimum": 1, "description": "how many, newest first"}),
                )],
                &[],
            )
        },
    },
    Command {
        name: "newest",
        usage: "newest",
        desc: "jump to the newest exchange and follow",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "clear-table",
        usage: "clear-table",
        desc: "hide what's in the table; new traffic still arrives",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "goto",
        usage: "goto <view>",
        desc: "engagement | traffic | surface | tasks | findings",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: || obj(&[("view", one_of(&VIEWS, "the view"))], &["view"]),
    },
    Command {
        name: "theme",
        usage: "theme <name>",
        desc: "rust | rust-light | blued | blued-light",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: || obj(&[("name", one_of(&THEMES, "the theme"))], &["name"]),
    },
    Command {
        name: "keys",
        usage: "keys",
        desc: "keyboard shortcuts",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "console-key",
        usage: "console-key <char>",
        desc: "key that toggles this console (default `)",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: || {
            obj(
                &[(
                    "char",
                    json!({"type": "string", "minLength": 1, "maxLength": 1, "description": "one character"}),
                )],
                &["char"],
            )
        },
    },
    Command {
        name: "clear",
        usage: "clear",
        desc: "clear the console",
        risk: Risk::Read,
        mcp: false,
        hidden: false,
        ui: true,
        args: none,
    },
    Command {
        name: "god",
        usage: "god",
        desc: "",
        risk: Risk::Read,
        mcp: false,
        hidden: true,
        ui: true,
        args: none,
    },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// What `GET /api/commands` serves.
pub fn specs() -> Value {
    json!({
        "risks": Risk::ALL,
        "commands": COMMANDS.iter().map(Command::spec).collect::<Vec<_>>(),
    })
}

/// The commands an agent is offered (MCP `tools/list`, the agent host's
/// tool list): flagged `mcp`, not hidden, and of a class an agent may run.
pub fn offered() -> impl Iterator<Item = &'static Command> {
    COMMANDS
        .iter()
        .filter(|c| c.mcp && !c.hidden && c.risk.agent_may())
}

/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Unknown,
    /// Not offered to agents (the `mcp` flag is off, or it's hidden).
    NotOffered,
    /// Of a class no agent may run, whatever its flag says.
    Forbidden(Risk),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unknown => f.write_str("no such command"),
            Refusal::NotOffered => f.write_str("not offered to agents"),
            Refusal::Forbidden(r) => write!(
                f,
                "agents may never run {} commands: that's a human's call",
                r.as_str()
            ),
        }
    }
}

/// May `actor` run the command `name`? A human may run any command; an
/// agent only one it is offered, and never a policy or export one: that is
/// checked first and on its own, so a flag flipped by mistake still can't
/// hand an agent the rules, promotion or export.
pub fn admit(actor: Actor, name: &str) -> Result<&'static Command, Refusal> {
    admit_in(COMMANDS, actor, name)
}

fn admit_in<'a>(list: &'a [Command], actor: Actor, name: &str) -> Result<&'a Command, Refusal> {
    let cmd = list
        .iter()
        .find(|c| c.name == name)
        .ok_or(Refusal::Unknown)?;
    if actor == Actor::Human {
        return Ok(cmd);
    }
    if !cmd.risk.agent_may() {
        return Err(Refusal::Forbidden(cmd.risk));
    }
    if !cmd.mcp || cmd.hidden {
        return Err(Refusal::NotOffered);
    }
    Ok(cmd)
}

/// [`admit`], with an agent's refusal written to the audit log: a refused
/// agent call is evidence of a confused or hijacked agent, and is kept.
pub fn admit_audited(
    audit: &AuditLog,
    engagement: Option<String>,
    actor: Actor,
    name: &str,
) -> Result<&'static Command, Refusal> {
    let res = admit(actor, name);
    if let (Err(why), Actor::Agent) = (&res, actor) {
        let mut rec = Record::new("command", "", Outcome::Denied);
        rec.actor = "agent".into();
        rec.engagement = engagement;
        rec.decision = Some("deny");
        // The name came from the agent: bounded, and only ever data.
        let shown: String = name.chars().take(64).collect();
        rec.reason = Some(format!("{shown:?}: {why}"));
        if let Err(e) = audit.record(&rec) {
            tracing::error!(error = %e, "audit write failed for a refused agent command");
        }
    }
    res
}

/// `ui/src/commands/specs.gen.ts`, which the UI registry is built on.
pub fn ui_specs() -> String {
    let risks: Vec<Value> = Risk::ALL.iter().map(|r| json!(r.as_str())).collect();
    let specs: Vec<Value> = COMMANDS.iter().map(Command::spec).collect();
    // Serializing a Value can't fail; an empty file would fail the UI's tests.
    let pretty = |v: Vec<Value>| serde_json::to_string_pretty(&v).unwrap_or_default();
    format!(
        "// Generated from src/commands.rs (#53): the server's command registry is the\n\
         // source of truth. Do not edit; regenerate with\n\
         //   SPALL_BLESS=1 cargo test --lib commands::tests::ui_specs_are_current\n\
         export const RISKS = {} as const;\n\n\
         export const SPECS = {} as const;\n",
        pretty(risks),
        pretty(specs),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_and_stable_identifiers() {
        let mut seen = HashSet::new();
        for c in COMMANDS {
            assert!(seen.insert(c.name), "duplicate {}", c.name);
            assert!(
                c.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_'),
                "{}",
                c.name
            );
            assert!(c.usage.starts_with(c.name), "{}", c.name);
        }
    }

    #[test]
    fn every_schema_is_a_closed_object_whose_required_fields_exist() {
        for c in COMMANDS {
            let s = c.args();
            assert_eq!(s["type"], "object", "{}", c.name);
            assert_eq!(s["additionalProperties"], false, "{}", c.name);
            let props = s["properties"].as_object().expect("properties");
            for r in s["required"].as_array().expect("required") {
                assert!(
                    props.contains_key(r.as_str().unwrap_or("")),
                    "{} requires {r}",
                    c.name
                );
            }
            for (k, p) in props {
                assert!(p["type"].is_string(), "{}.{k} has a type", c.name);
                assert!(p["description"].is_string(), "{}.{k} is described", c.name);
            }
        }
    }

    #[test]
    fn risk_classes_map_onto_the_gate() {
        assert_eq!(Risk::Send.class(), Some(Class::Send));
        assert_eq!(Risk::Unsafe.class(), Some(Class::Send));
        assert_eq!(Risk::Destructive.class(), Some(Class::Destructive));
        for r in [Risk::Read, Risk::Record, Risk::Policy, Risk::Export] {
            assert_eq!(r.class(), None, "{r:?} sends nothing");
        }
        assert_eq!(
            serde_json::to_value(Risk::ALL).unwrap(),
            json!([
                "read",
                "record",
                "send",
                "unsafe",
                "destructive",
                "policy",
                "export"
            ])
        );
        for r in Risk::ALL {
            assert_eq!(serde_json::to_value(r).unwrap(), r.as_str());
        }
    }

    #[test]
    fn the_registry_holds_what_ships() {
        let risk = |n: &str| find(n).map(|c| c.risk);
        assert_eq!(risk("replay"), Some(Risk::Send));
        assert_eq!(risk("flag"), Some(Risk::Record));
        for n in ["scope", "engagement", "roe", "estop"] {
            assert_eq!(risk(n), Some(Risk::Policy), "{n}");
        }
        for n in ["promote", "export"] {
            assert_eq!(risk(n), Some(Risk::Export), "{n}");
        }
        let god = find("god").unwrap();
        assert!(god.hidden && !god.mcp);
        let p = find("propose_finding").unwrap();
        assert_eq!(
            (p.risk, p.mcp, p.ui),
            (Risk::Record, true, false),
            "the agent's, not the console's"
        );
        assert!(COMMANDS
            .iter()
            .filter(|c| !c.ui)
            .all(|c| c.name == "propose_finding"));
    }

    #[test]
    fn agents_are_offered_only_what_they_may_run() {
        let offered: Vec<&str> = offered().map(|c| c.name).collect();
        assert_eq!(
            offered,
            [
                "filter",
                "open",
                "replay",
                "hosts",
                "rules",
                "task",
                "propose_finding"
            ]
        );
        for c in COMMANDS.iter().filter(|c| !c.risk.agent_may()) {
            assert!(!c.mcp, "{} is {:?} and flagged mcp", c.name, c.risk);
        }
    }

    #[test]
    fn a_human_may_run_anything_an_agent_only_what_it_is_offered() {
        assert!(admit(Actor::Human, "roe").is_ok());
        assert!(admit(Actor::Human, "export").is_ok());
        assert!(admit(Actor::Agent, "replay").is_ok());
        assert_eq!(admit(Actor::Agent, "ca").err(), Some(Refusal::NotOffered));
        assert_eq!(admit(Actor::Agent, "god").err(), Some(Refusal::NotOffered));
        assert_eq!(admit(Actor::Agent, "nope").err(), Some(Refusal::Unknown));
        assert_eq!(admit(Actor::Human, "nope").err(), Some(Refusal::Unknown));
        assert_eq!(
            admit(Actor::Agent, "promote").err(),
            Some(Refusal::Forbidden(Risk::Export))
        );
        assert_eq!(
            admit(Actor::Agent, "scope").err(),
            Some(Refusal::Forbidden(Risk::Policy))
        );
    }

    #[test]
    fn a_flipped_mcp_flag_still_cannot_hand_an_agent_policy_or_export() {
        let forged: Vec<Command> = COMMANDS
            .iter()
            .map(|c| Command {
                mcp: true,
                hidden: false,
                ..*c
            })
            .collect();
        for c in &forged {
            let got = admit_in(&forged, Actor::Agent, c.name);
            match c.risk {
                Risk::Policy | Risk::Export => {
                    assert_eq!(got.err(), Some(Refusal::Forbidden(c.risk)), "{}", c.name)
                }
                _ => assert!(got.is_ok(), "{}", c.name),
            }
        }
    }

    #[test]
    fn an_agent_refusal_is_audited_and_a_human_call_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::open(&dir.path().join("audit.jsonl")).unwrap();
        assert!(admit_audited(&log, None, Actor::Human, "export").is_ok());
        assert!(admit_audited(&log, None, Actor::Agent, "replay").is_ok());
        assert!(
            log.tail(10).unwrap().is_empty(),
            "admissions aren't refusals"
        );

        let forged = format!("export\n{}", "x".repeat(500));
        assert!(admit_audited(&log, Some("e-1".into()), Actor::Agent, "export").is_err());
        assert!(admit_audited(&log, None, Actor::Agent, &forged).is_err());
        let recs = log.tail(10).unwrap();
        assert_eq!(recs.len(), 2);
        let export = recs
            .iter()
            .find(|r| r["engagement"] == "e-1")
            .expect("the export refusal");
        assert_eq!(export["actor"], "agent");
        assert_eq!(export["action"], "command");
        assert_eq!(export["outcome"], "denied");
        assert_eq!(
            export["reason"],
            "\"export\": agents may never run export commands: that's a human's call"
        );
        let unknown = recs.iter().find(|r| r["engagement"].is_null()).unwrap();
        let reason = unknown["reason"].as_str().unwrap();
        assert!(
            reason.ends_with("no such command") && reason.len() < 120,
            "{reason}"
        );
        assert!(!reason.contains('\n'), "quoted, so one line");
    }

    #[test]
    fn ui_specs_are_current() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/src/commands/specs.gen.ts");
        let want = ui_specs();
        if std::env::var_os("SPALL_BLESS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{} is stale: regenerate with SPALL_BLESS=1 cargo test --lib commands::tests::ui_specs_are_current",
            path.display()
        );
    }
}
