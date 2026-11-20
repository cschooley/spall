# spall

An assessment workbench for web application security testing: a web UI for interactive testing and an MCP server for agentic testing, built on top of [taphttp](https://github.com/cschooley/taphttp). Findings are exported as [flux](https://github.com/cschooley/flux), a SARIF 2.1.0 profile.

**Status:** pre-alpha. Milestone M0 ("hello": live traffic console) in progress. See [docs/architecture.md](docs/architecture.md).

**For authorized testing and local development only.** spall displays raw captured traffic, including credentials. It binds to localhost by default; keep it that way unless you have added authentication.

## Quickstart (M0, once built)

```sh
docker compose up
curl -x http://127.0.0.1:8080 http://example.com
# open http://127.0.0.1:3000
```

## Provenance

Built with Claude Code, human-directed and reviewed.
