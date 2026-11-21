# Requests for taphttp

Things spall needs (or would benefit from) that belong in taphttp, not
here.

## Open

### Log rotation semantics

spall's `--log-file` tailer handles truncation and file replacement, but
cannot detect a truncate-and-regrow past the previous size between polls.
If taphttp ever rotates its log, rotating by rename (new inode) is the
safe choice. This only matters while spall tails the file; in compose,
spall reads history from the lever API instead (spall#4).

## Resolved in v0.3.0

- **Concurrent writes corrupted the JSONL log:**
  [taphttp#6](https://github.com/cschooley/taphttp/issues/6). Each entry
  is now one write, and writers are serialized. Against v0.2.0, 300
  requests sent 50 at a time produced 9 fused lines (18 exchanges lost);
  the same test against `ghcr.io/cschooley/taphttp:0.3.0` produced 0
  fused lines and 0 blank lines out of 300.
- **Lever API usable by spall:**
  [taphttp#7](https://github.com/cschooley/taphttp/issues/7). It adds:
  - a token file kept off the CA volume
  - sqlite-backed `/exchanges` with a cursor and search
  - `/feed?since=<id>`

## Resolved in v0.2.0

- **Dockerfile / published image:** [taphttp#4](https://github.com/cschooley/taphttp/issues/4).
  spall's compose uses the GHCR image (amd64 only); `docker/taphttp/` is
  gone.
- **Schema v3:** [taphttp#5](https://github.com/cschooley/taphttp/issues/5).
  - Repeated headers are kept in wire order.
  - Non-UTF-8 header values are kept.
  - Bodies are byte-exact (`*_body_encoding: base64`).
  - Every line carries `schema_version`.
  - The `taphttp-types` crate replaces spall's hand-written mirror.

  Keep the `taphttp-types` tag in `Cargo.toml` and the image tag in
  `docker-compose.yml` in step.
