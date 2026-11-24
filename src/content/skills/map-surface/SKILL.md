---
name: map-surface
description: Map an app's endpoints, parameters, authentication and stack from traffic and safe requests (WSTG-INFO).
---
# Map the surface (WSTG-INFO)

1. Start from what's captured: `filter host:<host>`, then `filter method:POST`,
   `filter status:4xx` and `filter status:5xx`. Captured traffic costs no budget.
2. For each endpoint note: method, path (with ids replaced by `{id}`), its
   parameters (query, body, headers that matter), whether it needs a session,
   and what it returns.
3. Fingerprint the stack (WSTG-INFO-02, -08): `Server`, `X-Powered-By`, cookie
   names, error pages, framework paths.
4. Fill gaps with safe sends only: `fetch` a documented path (`/robots.txt`,
   `/.well-known/security.txt`, an OpenAPI document) or `replay` a GET without
   its cookie to see whether it needs authentication.
5. Passive findings worth proposing: missing security headers (WSTG-CONF-07,
   -12), version disclosure, verbose errors with stack traces (WSTG-ERRH-01).
6. Report the map: endpoints, how auth works, the stack, related hosts, and
   what a Tester should look at first and why.
