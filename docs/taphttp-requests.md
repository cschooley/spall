# Requests for taphttp

Things spall needs (or would benefit from) that belong in taphttp, not
here.

## Open

### Replay API gaps (M2)

[taphttp#10](https://github.com/cschooley/taphttp/issues/10), still open
in v0.5.0. Not blocking: spall#19 and spall#21 work around it.

- Header overrides are a name->value map, so repeated headers can't be
  edited in order.
- The body override is a string, so binary bodies can only be replayed
  as captured.
- `GET /exchanges` can't filter by `replay_of`, which the Replays tab
  (spall#21) wants.

### Deployment info for the launcher tile

[taphttp#18](https://github.com/cschooley/taphttp/issues/18): `GET /info`
on the lever API. It would return:

- the version
- proxy and API listeners
- TLS and auth mode
- CA public details

It must expose no secrets: no CA key, API token, proxy credentials or
paths. For spall#16; not scheduled.

### Log rotation semantics

spall's `--log-file` tailer handles truncation and file replacement, but
cannot detect a truncate-and-regrow past the previous size between polls.
If taphttp ever rotates its log, rotating by rename (new inode) is the
safe choice. This only matters while spall tails the file; in compose,
spall reads history from the lever API instead (spall#4).

## Resolved in v0.5.0

- **chrono unmaintained (RUSTSEC-2026-0543):**
  [taphttp#11](https://github.com/cschooley/taphttp/issues/11).
  `taphttp-types` 0.4.0 uses its own `Timestamp` (wrapping jiff), and
  `ts` serializes byte for byte as before. spall needed no code change.

## Resolved in v0.4.0

- **Attribution and a status-class filter:**
  [taphttp#9](https://github.com/cschooley/taphttp/issues/9).
  - Schema v4 adds `source`: the proxy-auth username, or `null`.
  - `/exchanges?status=` takes classes (`4xx`) and lists (`401,5xx`).
- **Remote deployment:**
  [taphttp#8](https://github.com/cschooley/taphttp/issues/8). A
  non-loopback listener needs:
  - the proxy: auth and TLS, or `--allow-open-proxy`;
  - the API: `--api-tls`, or `--api-allow-plaintext`.

  spall's compose passes both overrides, since it publishes only on
  127.0.0.1.

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
