---
name: retest
description: Retest a finding: replay its evidence, compare with the original, and say whether it still reproduces.
---
# Retest a finding

1. Your task names the finding and its evidence exchanges. `open` each one and
   read what made it a finding (the candidate's message says).
2. `replay` the evidence as captured. If the session has expired, say so: a
   401 is not a fix.
3. Compare the replay with the original: the same status, and the same proof
   in the body (the reflected payload, the other user's data, the error).
4. Verdict:
   - **Still reproduces**: the proof is there.
   - **Fixed**: the same request no longer shows it, and the endpoint still
     works (a 200 without the proof, or a clean 403), not merely gone (a 404
     for a moved path means "can't tell").
   - **Can't tell**: the endpoint changed, the session is gone, or the host
     now answers somewhere else.
5. File the verdict with `retest_result` (finding, verdict, the replay's
   exchange id, one line of why), then report.
