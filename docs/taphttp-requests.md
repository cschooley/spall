# Requests for taphttp

Things spall needs (or would benefit from) that belong in taphttp, not
here.

## Open

### Concurrent writes corrupt the JSONL log (found against v0.2.0)

`JsonLinesBackend::append_line` opens the file with `O_APPEND` and calls
`writeln!(f, "{line}")`. On an unbuffered `File`, `writeln!` can issue two
`write` calls (the line, then `\n`), and `Store::record` is called from
every connection task with no lock. Two exchanges finishing together
produce `{A}{B}\n\n`: one line consumers cannot parse, plus a blank line.
Both exchanges are lost to anything tailing the file.

Reproduced against `ghcr.io/cschooley/taphttp:0.2.0`: 300 requests, 50 at
a time, through the proxy produced 9 fused lines (18 exchanges lost, 6%).
A normal browsing session had already produced 26.

Ask: write each entry with a single `write_all` of `line + "\n"`, and
serialize writers (e.g. keep the file open behind a `Mutex`). Add a test
that appends from many threads at once and asserts every line parses.

spall does not try to split fused lines; it logs them as malformed and
skips them.

### Log rotation semantics

spall handles truncation and file replacement, but cannot detect a
truncate-and-regrow past the previous size between polls. If taphttp
ever rotates its log, rotating by rename (new inode) is the safe choice.
M1 may move spall to the lever API, which makes this moot.

## Resolved in v0.2.0

- **Dockerfile / published image:** [taphttp#4](https://github.com/cschooley/taphttp/issues/4).
  spall's compose uses `ghcr.io/cschooley/taphttp:0.2.0` (amd64 only);
  `docker/taphttp/` is gone.
- **Schema v3:** [taphttp#5](https://github.com/cschooley/taphttp/issues/5).
  Repeated headers kept in wire order, non-UTF-8 header values kept,
  bodies byte-exact (`*_body_encoding: base64`), `schema_version` on every
  line, and the `taphttp-types` crate, which spall now depends on in place
  of its hand-written mirror. Keep the `taphttp-types` tag in `Cargo.toml`
  and the image tag in `docker-compose.yml` in step.
