// Generated from src/commands.rs (#53): the server's command registry is the
// source of truth. Do not edit; regenerate with
//   SPALL_BLESS=1 cargo test --lib commands::tests::ui_specs_are_current
export const RISKS = [
  "read",
  "record",
  "send",
  "unsafe",
  "destructive",
  "policy",
  "export"
] as const;

export const SPECS = [
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "this list",
    "hidden": false,
    "mcp": false,
    "name": "help",
    "risk": "read",
    "ui": true,
    "usage": "help"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "query": {
          "description": "filter syntax: host: method: status: and free text",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "filter traffic (host: method: status: text); empty clears",
    "hidden": false,
    "mcp": true,
    "name": "filter",
    "risk": "read",
    "ui": true,
    "usage": "filter [query]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "body_length": {
          "description": "agents: how many body bytes to show",
          "maximum": 65536,
          "minimum": 1,
          "type": "integer"
        },
        "body_offset": {
          "description": "agents: the first body byte to show (bodies show 4 KiB by default)",
          "minimum": 0,
          "type": "integer"
        },
        "id": {
          "description": "the exchange id",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "open an exchange in the inspector",
    "hidden": false,
    "mcp": true,
    "name": "open",
    "risk": "read",
    "ui": true,
    "usage": "open <id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the exchange id; default: the selected one",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "the selected exchange as a curl command (also copied)",
    "hidden": false,
    "mcp": false,
    "name": "curl",
    "risk": "read",
    "ui": true,
    "usage": "curl"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "body": {
          "description": "replace the body",
          "type": "string"
        },
        "drop_headers": {
          "description": "remove every value of each named header",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "headers": {
          "description": "replace every value of each named header",
          "items": {
            "additionalProperties": false,
            "properties": {
              "name": {
                "description": "header name",
                "type": "string"
              },
              "value": {
                "description": "header value",
                "type": "string"
              }
            },
            "required": [
              "name",
              "value"
            ],
            "type": "object"
          },
          "type": "array"
        },
        "id": {
          "description": "the exchange to resend",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "method": {
          "description": "replace the method",
          "type": "string"
        },
        "yes": {
          "description": "confirm an unsafe method now instead of arming it",
          "type": "boolean"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "resend an exchange (edits: --method M --header 'N: v' --drop-header N --body B; unsafe methods: run twice, or --yes)",
    "hidden": false,
    "mcp": true,
    "name": "replay",
    "risk": "send",
    "ui": true,
    "usage": "replay <id> [--yes]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "headers": {
          "description": "the request's headers, in order (without them taphttp sends its own user-agent)",
          "items": {
            "additionalProperties": false,
            "properties": {
              "name": {
                "description": "header name",
                "type": "string"
              },
              "value": {
                "description": "header value",
                "type": "string"
              }
            },
            "required": [
              "name",
              "value"
            ],
            "type": "object"
          },
          "type": "array"
        },
        "method": {
          "description": "GET (the default) or HEAD",
          "enum": [
            "GET",
            "HEAD"
          ],
          "type": "string"
        },
        "url": {
          "description": "an absolute http or https URL",
          "type": "string"
        }
      },
      "required": [
        "url"
      ],
      "type": "object"
    },
    "desc": "send a new GET (or HEAD) to a URL in scope; crawling is fetch in a loop",
    "hidden": false,
    "mcp": true,
    "name": "fetch",
    "risk": "send",
    "ui": true,
    "usage": "fetch <url> [--head] [--header 'N: v']"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "entry": {
          "description": "a host, *.domain, URL prefix or CIDR; a URL or host for check",
          "type": "string"
        },
        "on": {
          "description": "for all: allow every host",
          "type": "boolean"
        },
        "op": {
          "description": "default: show",
          "enum": [
            "show",
            "add",
            "exclude",
            "remove",
            "check",
            "paste",
            "import",
            "all"
          ],
          "type": "string"
        },
        "wide": {
          "description": "for add: accept a too-wide entry (*.com, wider than /16), audited",
          "type": "boolean"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "hosts replay may send to; exclusions (!entry) win; changes last until restart",
    "hidden": false,
    "mcp": false,
    "name": "scope",
    "risk": "policy",
    "ui": true,
    "usage": "scope [add <entry> [--wide] | exclude|remove <entry> | check <url|host> | paste | import | all on|off]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "entry": {
          "description": "a host, *.domain, IP or CIDR",
          "type": "string"
        },
        "op": {
          "description": "default: show",
          "enum": [
            "show",
            "add",
            "remove"
          ],
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "hosts no engagement may reach (your own company, shared providers, .gov): checked after scope, never approvable; an engagement can be exempted from an entry",
    "hidden": false,
    "mcp": false,
    "name": "never-test",
    "risk": "policy",
    "ui": true,
    "usage": "never-test [add|remove <entry>]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "field": {
          "description": "for set",
          "enum": [
            "name",
            "client",
            "starts",
            "ends",
            "expires",
            "notes"
          ],
          "type": "string"
        },
        "name": {
          "description": "for new: the engagement's name",
          "type": "string"
        },
        "op": {
          "description": "default: show",
          "enum": [
            "show",
            "list",
            "new",
            "use",
            "set"
          ],
          "type": "string"
        },
        "value": {
          "description": "for set; empty clears a date",
          "type": "string"
        },
        "which": {
          "description": "for use: an engagement id or name",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "the engagement you're testing under: details, and switching between engagements",
    "hidden": false,
    "mcp": false,
    "name": "engagement",
    "risk": "policy",
    "ui": true,
    "usage": "engagement [list | new <name> | use <id|name> | set <field> <value>]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "action": {
          "description": "for window and blackout",
          "enum": [
            "add",
            "remove",
            "clear"
          ],
          "type": "string"
        },
        "days": {
          "description": "for window add: like mon-fri",
          "type": "string"
        },
        "field": {
          "description": "for set",
          "enum": [
            "methods",
            "safe",
            "rate",
            "tz",
            "agents",
            "destructive"
          ],
          "type": "string"
        },
        "from": {
          "description": "for blackout add: YYYY-MM-DDTHH:MM",
          "type": "string"
        },
        "hours": {
          "description": "for window add: HH:MM-HH:MM",
          "type": "string"
        },
        "n": {
          "description": "for remove: which one, from 1",
          "minimum": 1,
          "type": "integer"
        },
        "note": {
          "description": "for blackout add",
          "type": "string"
        },
        "op": {
          "description": "default: show",
          "enum": [
            "show",
            "set",
            "window",
            "blackout"
          ],
          "type": "string"
        },
        "to": {
          "description": "for blackout add: YYYY-MM-DDTHH:MM",
          "type": "string"
        },
        "value": {
          "description": "for set",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "rules of engagement: methods, confirmation, rate, time zone, testing and no-test windows, agents",
    "hidden": false,
    "mcp": false,
    "name": "roe",
    "risk": "policy",
    "ui": true,
    "usage": "roe [set <field> <value> | window add|remove|clear | blackout add|remove|clear]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "clear": {
          "description": "clear the stop instead (halted tasks stay halted)",
          "type": "boolean"
        },
        "reason": {
          "description": "why, for the audit log",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "agent e-stop (Shift+Esc): halt every agent task for good; clear lets new tasks run",
    "hidden": false,
    "mcp": false,
    "name": "estop",
    "risk": "policy",
    "ui": true,
    "usage": "estop [reason] | estop clear"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "seconds": {
          "description": "how long (default 120)",
          "maximum": 600,
          "minimum": 1,
          "type": "integer"
        },
        "stop": {
          "description": "end it now",
          "type": "boolean"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "quad mode: 4x agent workers at 4x rate for a while (default 120s, at most 600), inside the RoE and budgets; then a numbers-only scorecard",
    "hidden": false,
    "mcp": false,
    "name": "quad",
    "risk": "send",
    "ui": true,
    "usage": "quad [seconds] | quad stop"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "base": {
          "description": "an in-scope http(s) URL the check's paths are relative to",
          "type": "string"
        },
        "check": {
          "description": "a check's name, from checks",
          "type": "string"
        },
        "yes": {
          "description": "for the console: run it (without, it says what it would cost)",
          "type": "boolean"
        }
      },
      "required": [
        "base",
        "check"
      ],
      "type": "object"
    },
    "desc": "request a check's paths (forced browsing: backup files, admin interfaces, metafiles) against an in-scope base URL, after a soft-404 baseline; a summary of hits, each an exchange. Every request is gated and charged; checks lists them",
    "hidden": false,
    "mcp": true,
    "name": "sweep",
    "risk": "send",
    "ui": true,
    "usage": "sweep <base-url> <check> [--yes]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "the checks sweep can run: name, WSTG rule, paths, matchers; spall's vetted ones and the engagement's",
    "hidden": false,
    "mcp": true,
    "name": "checks",
    "risk": "read",
    "ui": true,
    "usage": "checks"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "new": {
          "description": "mint a payload",
          "type": "boolean"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "out-of-band callbacks: your payloads and the hits on them (DNS, HTTP, SMTP), each tied to the exchange that carried it; oob new mints one (sends nothing)",
    "hidden": false,
    "mcp": false,
    "name": "oob",
    "risk": "record",
    "ui": true,
    "usage": "oob [new]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "mint an out-of-band callback host for this task (blind SSRF, XXE, command injection, DNS exfiltration); it sends nothing: put it in a replay or fetch, then check oob_hits. Late callbacks still count. It is also your disposable inbox: use it as an email address (user@<host>) in a signup or password-reset flow and read the mail with oob_hits (an SMTP hit) — but only when the app emails an address you chose and your OAST server has a mail domain; a lab app that emails a fixed server won't reach it",
    "hidden": false,
    "mcp": true,
    "name": "oob_payload",
    "risk": "record",
    "ui": false,
    "usage": "oob_payload"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "the out-of-band callbacks on this task's payloads: protocol, source, time, the raw request (untrusted), and the exchange that carried the payload (propose_finding with it)",
    "hidden": false,
    "mcp": true,
    "name": "oob_hits",
    "risk": "read",
    "ui": false,
    "usage": "oob_hits"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "host": {
          "description": "only this host",
          "type": "string"
        },
        "untested": {
          "description": "only endpoints not yet tested for this WSTG category",
          "enum": [
            "INFO",
            "CONF",
            "IDNT",
            "ATHN",
            "ATHZ",
            "SESS",
            "INPV",
            "ERRH",
            "CRYP",
            "BUSL",
            "CLNT",
            "APIT"
          ],
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "the endpoints seen and documented, with who touched each and its coverage by WSTG category (found, tested); untested ATHZ lists those never tested for authorization",
    "hidden": false,
    "mcp": true,
    "name": "surface",
    "risk": "read",
    "ui": true,
    "usage": "surface [untested <CATEGORY>]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "category": {
          "description": "the WSTG category tested",
          "enum": [
            "INFO",
            "CONF",
            "IDNT",
            "ATHN",
            "ATHZ",
            "SESS",
            "INPV",
            "ERRH",
            "CRYP",
            "BUSL",
            "CLNT",
            "APIT"
          ],
          "type": "string"
        },
        "exchange_id": {
          "description": "an exchange this task sent or read, on the endpoint",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "note": {
          "description": "one line: what you examined and how",
          "type": "string"
        }
      },
      "required": [
        "exchange_id",
        "category",
        "note"
      ],
      "type": "object"
    },
    "desc": "record that you tested an endpoint for a WSTG category (what you examined and how, in a line), with an exchange this task sent or read on it; it fills the Surface's coverage. Not a finding: propose_finding for those",
    "hidden": false,
    "mcp": true,
    "name": "record_coverage",
    "risk": "record",
    "ui": false,
    "usage": "record_coverage"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "argv": {
          "description": "the program and its arguments, e.g. [\"jq\", \".alg\", \"token.json\"]",
          "items": {
            "type": "string"
          },
          "maxItems": 64,
          "minItems": 1,
          "type": "array"
        },
        "timeout_seconds": {
          "description": "wall clock, 60 if left out",
          "maximum": 300,
          "minimum": 1,
          "type": "integer"
        }
      },
      "required": [
        "argv"
      ],
      "type": "object"
    },
    "desc": "run a tool in this task's sandbox (gVisor, its own scratch dir, nothing of the host): argv as a list, never a shell string; one command costs one unit of budget. Tools: curl, jq, python3, nmap, openssl, dig. Its HTTP goes through taphttp when the sandbox has a network (the result says): only to in-scope hosts, each request charged and recorded; otherwise it has none, so decode, parse and compute (a JWT, a hash, a response you read) and send with fetch, replay and sweep. Raw TCP has no route. Its output is untrusted",
    "hidden": false,
    "mcp": true,
    "name": "run_in_sandbox",
    "risk": "send",
    "ui": false,
    "usage": "run_in_sandbox"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "unknown": {
          "description": "only hosts neither in scope nor excluded",
          "type": "boolean"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "hosts seen in traffic and whether each is in scope, excluded or undecided",
    "hidden": false,
    "mcp": true,
    "name": "hosts",
    "risk": "read",
    "ui": true,
    "usage": "hosts [--unknown]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "anyway": {
          "description": "flag it although it looks like an existing candidate (#78)",
          "type": "boolean"
        },
        "cwe": {
          "description": "override the rule's CWE",
          "minimum": 1,
          "type": "integer"
        },
        "exchange_id": {
          "description": "the exchange the finding is about",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "level": {
          "description": "SARIF level",
          "enum": [
            "error",
            "warning",
            "note",
            "none"
          ],
          "type": "string"
        },
        "message": {
          "description": "what was found",
          "type": "string"
        },
        "param": {
          "description": "the parameter or location",
          "type": "string"
        },
        "rank": {
          "description": "SARIF rank",
          "maximum": 100,
          "minimum": 0,
          "type": "number"
        },
        "replay": {
          "description": "replays as evidence",
          "items": {
            "description": "a replay of the exchange",
            "pattern": "^[A-Za-z0-9_-]{1,128}$",
            "type": "string"
          },
          "type": "array"
        },
        "rule": {
          "description": "a rule id from rules, like WSTG-INPV-05",
          "type": "string"
        }
      },
      "required": [
        "exchange_id",
        "rule"
      ],
      "type": "object"
    },
    "desc": "flag an exchange as a candidate finding (rules lists rule ids); it waits in Findings for triage",
    "hidden": false,
    "mcp": false,
    "name": "flag",
    "risk": "record",
    "ui": true,
    "usage": "flag <exchange-id> --rule <id> [--param P] [--level L] [--rank N] [--message M] [--replay <id>]... [--cwe N] [--anyway]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "candidate_id": {
          "description": "the candidate",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "undo": {
          "description": "take it out of the next export",
          "type": "boolean"
        }
      },
      "required": [
        "candidate_id"
      ],
      "type": "object"
    },
    "desc": "mark a confirmed candidate for export (a human's call), or take it out",
    "hidden": false,
    "mcp": false,
    "name": "promote",
    "risk": "export",
    "ui": true,
    "usage": "promote <candidate-id> [--undo]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "unredacted": {
          "description": "skip redaction (needs --allow-unredacted-export)",
          "type": "boolean"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "save the promoted findings as a flux (SARIF) report, redacted; --unredacted needs spall's --allow-unredacted-export",
    "hidden": false,
    "mcp": false,
    "name": "export",
    "risk": "export",
    "ui": true,
    "usage": "export [--unredacted]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "text": {
          "description": "match id, title or CWE",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "the rule catalog (WSTG v4.2 tests, with CWE): every rule, or those matching text",
    "hidden": false,
    "mcp": true,
    "name": "rules",
    "risk": "read",
    "ui": true,
    "usage": "rules [text]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the task, like T-3; an agent's default and only choice is its own",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "a task: goal, status, constraints, spend, and what it waits on (an agent sees only its own)",
    "hidden": false,
    "mcp": true,
    "name": "task",
    "risk": "read",
    "ui": true,
    "usage": "task [id]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the task, like T-3",
          "type": "string"
        },
        "reason": {
          "description": "why, kept with the task",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "stop one agent task for good (estop stops them all)",
    "hidden": false,
    "mcp": false,
    "name": "halt",
    "risk": "record",
    "ui": true,
    "usage": "halt <T-n> [reason]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "evidence": {
          "description": "more exchanges (replays) backing it: ones this task sent or read",
          "items": {
            "description": "an exchange",
            "pattern": "^[A-Za-z0-9_-]{1,128}$",
            "type": "string"
          },
          "type": "array"
        },
        "exchange_id": {
          "description": "the exchange that shows it: one this task sent or read",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "level": {
          "description": "SARIF level",
          "enum": [
            "error",
            "warning",
            "note",
            "none"
          ],
          "type": "string"
        },
        "message": {
          "description": "what was found, for the tester",
          "type": "string"
        },
        "parameter": {
          "description": "the parameter or location, if any",
          "type": "string"
        },
        "rule": {
          "description": "a rule id from rules, like WSTG-INPV-05",
          "type": "string"
        }
      },
      "required": [
        "exchange_id",
        "rule",
        "message"
      ],
      "type": "object"
    },
    "desc": "propose a candidate finding with evidence from this task's own traffic; a human triages and promotes it",
    "hidden": false,
    "mcp": true,
    "name": "propose_finding",
    "risk": "record",
    "ui": false,
    "usage": "propose_finding"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "done": {
          "description": "true when you're finished: the task ends as done",
          "type": "boolean"
        },
        "text": {
          "description": "the report, plain text (at most 16,000 characters)",
          "type": "string"
        }
      },
      "required": [
        "text"
      ],
      "type": "object"
    },
    "desc": "file your report on this task for the tester: what you did, what you found, what you proposed, what's left. Filing again replaces it; done: true also ends the task (only while it runs)",
    "hidden": false,
    "mcp": true,
    "name": "report",
    "risk": "record",
    "ui": false,
    "usage": "report"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "name": {
          "description": "a skill's name, like test-authz",
          "type": "string"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "your role's skills: with no name, list them; with one, how to do that kind of work. Read one before you start that work",
    "hidden": false,
    "mcp": true,
    "name": "skill",
    "risk": "read",
    "ui": false,
    "usage": "skill [name]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the exchange",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Why is this interesting? Drafts a reporter task that reads the exchange and says what it shows. No requests",
    "hidden": false,
    "mcp": true,
    "name": "why",
    "risk": "record",
    "ui": true,
    "usage": "why <exchange-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Explain for devs: drafts a reporter task that writes the finding up for the developers. No requests",
    "hidden": false,
    "mcp": true,
    "name": "explain",
    "risk": "record",
    "ui": true,
    "usage": "explain <candidate-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the exchange",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Map auth: drafts a mapper task: which sessions reach this endpoint, with which methods. ~40 requests, safe methods",
    "hidden": false,
    "mcp": true,
    "name": "map_auth",
    "risk": "record",
    "ui": true,
    "usage": "map_auth <exchange-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "category": {
          "description": "the WSTG category",
          "enum": [
            "ATHZ",
            "INPV",
            "ATHN",
            "SESS",
            "BUSL",
            "CONF"
          ],
          "type": "string"
        },
        "id": {
          "description": "the exchange",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        }
      },
      "required": [
        "id",
        "category"
      ],
      "type": "object"
    },
    "desc": "Test for…: drafts a tester task for one WSTG category (ATHZ, INPV, ATHN, SESS, BUSL, CONF) on this endpoint. ~100 requests; unsafe ones asked for",
    "hidden": false,
    "mcp": true,
    "name": "test_for",
    "risk": "record",
    "ui": true,
    "usage": "test_for <exchange-id> <category>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Retest: drafts a tester task that replays the finding's evidence and says whether it still reproduces. Its methods are the evidence's",
    "hidden": false,
    "mcp": true,
    "name": "retest",
    "risk": "record",
    "ui": true,
    "usage": "retest <candidate-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Find variants: drafts a tester task for the same bug in sibling parameters and endpoints. ~60 requests; unsafe ones asked for",
    "hidden": false,
    "mcp": true,
    "name": "variants",
    "risk": "record",
    "ui": true,
    "usage": "variants <candidate-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Escalate impact: drafts a tester task: can it become a write, stored, or chain? ~30 requests, every one asked for",
    "hidden": false,
    "mcp": true,
    "name": "escalate",
    "risk": "record",
    "ui": true,
    "usage": "escalate <candidate-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "id": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        }
      },
      "required": [
        "id"
      ],
      "type": "object"
    },
    "desc": "Build a PoC: drafts a tester task to build a standalone, non-destructive proof in the sandbox (#13). ~30 requests, every one asked for",
    "hidden": false,
    "mcp": true,
    "name": "poc",
    "risk": "record",
    "ui": true,
    "usage": "poc <candidate-id>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "label": {
          "description": "a short filename, like idor-poc.txt",
          "type": "string"
        },
        "proof": {
          "description": "the proof: the exact request(s), the response that shows it, and what a reader should see",
          "type": "string"
        }
      },
      "required": [
        "proof"
      ],
      "type": "object"
    },
    "desc": "save the non-destructive proof you built in the sandbox as a task artifact for the tester; it's not a finding (propose_finding is)",
    "hidden": false,
    "mcp": true,
    "name": "save_poc",
    "risk": "record",
    "ui": false,
    "usage": "save_poc"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "candidate": {
          "description": "the candidate, like c-1a2b3c4d",
          "type": "string"
        },
        "exchange_id": {
          "description": "the replay this task sent that shows it",
          "pattern": "^[A-Za-z0-9_-]{1,128}$",
          "type": "string"
        },
        "note": {
          "description": "one line of why, for the tester",
          "type": "string"
        },
        "verdict": {
          "description": "what the retest showed",
          "enum": [
            "reproduces",
            "fixed",
            "unclear"
          ],
          "type": "string"
        }
      },
      "required": [
        "candidate",
        "verdict",
        "exchange_id",
        "note"
      ],
      "type": "object"
    },
    "desc": "file a retest's verdict on a candidate: reproduces, fixed or unclear, with the replay this task sent that shows it. Fixed sets flux status fixed; the tester can reopen it",
    "hidden": false,
    "mcp": true,
    "name": "retest_result",
    "risk": "record",
    "ui": false,
    "usage": "retest_result"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "taphttp's CA certificate: fingerprint, download, and browser setup steps",
    "hidden": false,
    "mcp": false,
    "name": "ca",
    "risk": "read",
    "ui": true,
    "usage": "ca"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "last": {
          "description": "how many, newest first",
          "minimum": 1,
          "type": "integer"
        }
      },
      "required": [],
      "type": "object"
    },
    "desc": "recent replay attempts, denials included",
    "hidden": false,
    "mcp": false,
    "name": "audit",
    "risk": "read",
    "ui": true,
    "usage": "audit [--last N]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "jump to the newest exchange and follow",
    "hidden": false,
    "mcp": false,
    "name": "newest",
    "risk": "read",
    "ui": true,
    "usage": "newest"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "hide what's in the table; new traffic still arrives",
    "hidden": false,
    "mcp": false,
    "name": "clear-table",
    "risk": "read",
    "ui": true,
    "usage": "clear-table"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "view": {
          "description": "the view",
          "enum": [
            "engagement",
            "traffic",
            "surface",
            "tasks",
            "findings"
          ],
          "type": "string"
        }
      },
      "required": [
        "view"
      ],
      "type": "object"
    },
    "desc": "engagement | traffic | surface | tasks | findings",
    "hidden": false,
    "mcp": false,
    "name": "goto",
    "risk": "read",
    "ui": true,
    "usage": "goto <view>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "name": {
          "description": "the theme",
          "enum": [
            "rust",
            "rust-light",
            "blued",
            "blued-light"
          ],
          "type": "string"
        }
      },
      "required": [
        "name"
      ],
      "type": "object"
    },
    "desc": "rust | rust-light | blued | blued-light",
    "hidden": false,
    "mcp": false,
    "name": "theme",
    "risk": "read",
    "ui": true,
    "usage": "theme <name>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "keyboard shortcuts",
    "hidden": false,
    "mcp": false,
    "name": "keys",
    "risk": "read",
    "ui": true,
    "usage": "keys"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
        "char": {
          "description": "one character",
          "maxLength": 1,
          "minLength": 1,
          "type": "string"
        }
      },
      "required": [
        "char"
      ],
      "type": "object"
    },
    "desc": "key that toggles this console (default `)",
    "hidden": false,
    "mcp": false,
    "name": "console-key",
    "risk": "read",
    "ui": true,
    "usage": "console-key <char>"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "clear the console",
    "hidden": false,
    "mcp": false,
    "name": "clear",
    "risk": "read",
    "ui": true,
    "usage": "clear"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {},
      "required": [],
      "type": "object"
    },
    "desc": "",
    "hidden": true,
    "mcp": false,
    "name": "god",
    "risk": "read",
    "ui": true,
    "usage": "god"
  }
] as const;
