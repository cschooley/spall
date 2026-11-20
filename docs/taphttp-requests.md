# Requests for taphttp

Things spall needs (or would benefit from) that belong in taphttp, not
here. Noted while building M0 against taphttp `51eb9bc`.

Filed upstream:
- [taphttp#4](https://github.com/cschooley/taphttp/issues/4): Dockerfile (item 1)
- [taphttp#5](https://github.com/cschooley/taphttp/issues/5): schema v3: repeated headers, non-UTF-8 header values, byte-exact bodies, `schema_version` in the log, shared types crate (items 2–5)

## 1. Dockerfile / published image (needed for M0; worked around)

taphttp has no Dockerfile, so compose cannot use a git-URL build context
(`build: https://github.com/cschooley/taphttp.git` needs a Dockerfile in
that repo). spall builds it from source in `docker/taphttp/Dockerfile` at
a pinned commit instead.

Ask: a Dockerfile in the taphttp repo (non-root user, `TAPHTTP_DATA`
pointing at a volume), and ideally a published image. spall's
compose file would then shrink to `image:` or a git build context, and
the pin would move to a tag.

## 2. `TrafficEntry` as a shared crate

spall mirrors `storage::TrafficEntry` (schema v2) in `src/entry.rs`. A
small `taphttp-types` crate (or a `lib` target) would remove the
duplication and make schema bumps a compile error instead of a silent
drift.

## 3. Schema version in the JSONL log

`SCHEMA_VERSION` exists in code but is not written into the log. A
consumer tailing the file cannot tell which shape it is reading. Options:
a `"v": 2` field on each line, or a header line.

## 4. Repeated headers are collapsed (matters for DAST)

`collect_headers` builds a `HashMap<String, String>`, so repeated headers
keep only one value. Multiple `Set-Cookie` response headers, which matter
for session and cookie-flag checks, are lost. Ask: preserve all values
(`Vec<(String, String)>` or `HashMap<String, Vec<String>>`) in the next
schema version.

## 5. Binary bodies are stored lossily

`maybe_utf8` uses `String::from_utf8_lossy`, so non-UTF-8 bodies are
corrupted rather than preserved. Ask: store such bodies base64-encoded
with an encoding marker (e.g. `res_body_encoding: "base64"`), so replay
and evidence stay byte-exact.

## 6. Log rotation semantics

spall handles truncation and file replacement, but cannot detect a
truncate-and-regrow past the previous size between polls. If taphttp
ever rotates its log, rotating by rename (new inode) is the safe choice.
M1 moves spall to the lever API / sqlite, which makes this moot.
