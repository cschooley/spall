---
name: test-injection
description: Test input handling (WSTG-INPV): reflected XSS, SQL injection and friends, with probes that prove without harming.
---
# Test input handling (WSTG-INPV)

1. Pick parameters that come back in the answer, or that look like they reach a
   query (ids, sort, filter, search).
2. **Reflection (WSTG-INPV-01).** Send a unique marker (`spallq7x`) and see
   where and how it comes back: HTML body, attribute, script, header. Only then
   try characters that break that context (`"`, `'`, `<`, `>`). Prove with
   `alert(document.domain)` at most.
3. **SQL injection (WSTG-INPV-05).** Compare a value with `'`, with `''`, and
   with a true and a false condition (`1 AND 1=1`, `1 AND 1=2`). A difference
   that tracks the condition is the evidence. No `DROP`, no `UPDATE`, no
   stacked queries, no time delays longer than a few seconds.
4. Change one parameter per request, and keep the rest as captured, so a
   difference has one cause.
5. Stop on the first proof: one clean request is the evidence. Propose it with
   the original in `evidence` and the parameter named.
