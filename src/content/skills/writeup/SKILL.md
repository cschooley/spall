---
name: writeup
description: Write a finding up for the developers who will fix it, from its evidence.
---
# Write up for developers

A developer reads this once, between other tickets. Give them:

1. **What**: one sentence, no jargon they'd have to look up.
2. **Where**: the method and path, the parameter.
3. **Reproduce**: the request from the evidence, trimmed to what matters, and
   what to look for in the answer.
4. **Impact**: what an attacker gets, concretely, for this app. No scare words.
5. **Fix**: the fix in their stack's terms (parameterized query, output
   encoding for that context, an ownership check in the handler), and how to
   check it's fixed.
6. **Reference**: the WSTG test and CWE from the rule.

Quote only what the evidence shows. Keep masked credentials masked.
