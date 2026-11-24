---
name: test-authz
description: Test authorization (WSTG-ATHZ): IDOR, missing function-level checks, privilege escalation, by reading, never writing.
---
# Test authorization (WSTG-ATHZ)

1. Find requests that name an object: an id in the path, query or body
   (`/orders/1043`, `?user=17`, `{"account": "..."}`).
2. **IDOR (WSTG-ATHZ-04).** Replay with the id changed to a neighbour or to
   one seen elsewhere in traffic. Same session. Compare with the original:
   another user's data in a 200 is the finding; a 403 or 404 is the control
   working.
3. **No session.** Replay with `drop_headers: ["Cookie", "Authorization"]`.
   The same answer as with a session means the endpoint isn't protected.
4. **Function level (WSTG-ATHZ-02).** Admin-looking paths seen in traffic or
   the app's JavaScript, replayed with an ordinary user's session.
5. Read, never write: test with GET where you can. Changing or deleting
   someone else's object is never the proof, even when the method is allowed.
6. Propose with the replay as the exchange and the original in `evidence`, and
   say exactly which id you changed.
