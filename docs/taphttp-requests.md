# Requests for taphttp

Things spall needs (or would benefit from) that belong in taphttp, not
here.

## Open

### Distinct hosts in history

[taphttp#21](https://github.com/cschooley/taphttp/issues/21): `GET /hosts`, the distinct hosts in history with counts and first/last seen. spall#39 builds scope from traffic, but spall only sees its backfill (the newest `--history` exchanges) and the live feed, so a host seen once early in a long engagement never reaches its list. spall keeps the scope state; taphttp only counts.

### CA install hints use container paths

[taphttp#19](https://github.com/cschooley/taphttp/issues/19): `taphttp ca info` prints install commands for `/ca/ca.crt`, which doesn't exist on the host. It should point at `ca print`, show the fingerprint, and recommend a dedicated browser profile over system-wide trust. The README's "A browser for testing" covers it meanwhile.

### Log rotation semantics

spall's `--log-file` tailer handles truncation and file replacement, but
cannot detect a truncate-and-regrow past the previous size between polls.
If taphttp ever rotates its log, rotating by rename (new inode) is the
safe choice. This only matters while spall tails the file; in compose,
spall reads history from the lever API instead (spall#4).

## Resolved in v0.6.0

- **Replay API for M2:**
  [taphttp#10](https://github.com/cschooley/taphttp/issues/10).
  - `POST /replay` takes an ordered `req_headers` list, `body_encoding`
    for binary bodies, and `meta` attribution.
  - Failed attempts are recorded with `error` (schema v5) and returned
    as a 502.
  - `GET /exchanges?replay_of=<id>` lists an exchange's replays.
- **Deployment info:**
  [taphttp#18](https://github.com/cschooley/taphttp/issues/18).
  `GET /info` reports the version, listeners, TLS and auth mode, and CA
  public details, with no secrets. It's for spall#16.

## Resolved in v0.6.1

- **Serve the public CA certificate:**
  [taphttp#20](https://github.com/cschooley/taphttp/issues/20).
  `GET /ca.pem` and `/ca.der` on the lever API (the certificate `/info`
  reports as `ca.sha256`), and `http://taphttp/` through the proxy for
  devices. spall#32 offers the download from its UI.

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
