---
name: proof-of-concept
description: Build a standalone, non-destructive proof of a candidate in the sandbox, and save it as a task artifact (save_poc).
---
# Build a proof of concept (non-destructive)

A PoC shows the bug is real and reproducible, and nothing more. It never
harms the target, exfiltrates data, or persists. Build it in the sandbox,
then hand it back with `save_poc`.

1. **Read the candidate first.** Open its evidence (the exchanges it was
   flagged on) and be sure you understand the one thing it claims. You are
   proving that claim, not hunting for new bugs.
2. **Prove, don't harm.** Pick the smallest action that demonstrates it:
   - XSS: show `document.domain` (or a harmless `print`), never a cookie
     stealer, keylogger, or anything that calls out.
   - IDOR / broken access control: read one record that should be denied
     you, and show the ownership field proving it isn't yours. Never write,
     update, or delete.
   - SQL injection: read a harmless value (a version string, a boolean
     difference), never `DROP`/`UPDATE`/`DELETE`, no stacked queries, no
     long time delays.
   - SSRF / open redirect: point at a sink you control (your OOB payload),
     never an internal metadata endpoint you then act on.
3. **Build it in the sandbox.** Use `run_in_sandbox` to assemble the proof —
   a short script, a `curl` sequence, a minimal HTML page. Keep it readable:
   the tester will review it. Each unsafe send through the gate still asks a
   human first; nothing leaves scope.
4. **Make it reproducible.** The proof should carry the exact request(s),
   the response excerpt that proves it, and one line on what a reader should
   see. Assume the tester runs it once, months from now.
5. **Save it.** Call `save_poc` with that proof as its text. It becomes a
   task artifact the tester opens; it is not a finding (propose_finding
   stays for that). Then `report` what you proved and, plainly, what you
   deliberately did not touch.
