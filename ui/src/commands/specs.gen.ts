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
    "mcp": true,
    "name": "help",
    "risk": "read",
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
    "usage": "filter [query]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
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
    "mcp": true,
    "name": "curl",
    "risk": "read",
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
    "usage": "replay <id> [--yes]"
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
    "usage": "scope [add|exclude|remove <entry> | check <url|host> | paste | import | all on|off]"
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
    "usage": "estop [reason] | estop clear"
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
    "usage": "hosts [--unknown]"
  },
  {
    "args": {
      "additionalProperties": false,
      "properties": {
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
    "usage": "flag <exchange-id> --rule <id> [--param P] [--level L] [--rank N] [--message M] [--replay <id>]... [--cwe N]"
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
    "usage": "rules [text]"
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
    "usage": "god"
  }
] as const;
