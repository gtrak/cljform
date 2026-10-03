# Press/fuzz resource discipline (standing, applies to every T≥13 spec)

Background: the provider host OOMed mid-T12-finisher (502s) and one local
battery pass aborted (rc=134) while two batteries raced concurrently.
Root causes: unbounded generated inputs, concurrent batteries, no memory
ceiling. Rules from now on:

1. SERIAL ONLY. One battery at a time; no `&`/setsid battery processes.
2. Every heavy invocation (fuzz loops, deep-file generation, scale probes,
   old-vs-new batteries) goes through `scripts/bounded-run.sh <max_mb> ...`.
   Suggested caps: fuzz/battery loops 2048M; 50k-deep probes 4096M;
   100k-deep probes 8192M; ordinary edits/checks need no wrapper.
3. Generated-input caps by default: depth <= 20k, file size <= 5 MB,
   siblings <= 20k. Deeper/bigger only with an explicit per-case cap via
   bounded-run and a stated reason.
4. Peak RSS from `/usr/bin/time -v` is reported per battery in the
   deliverable (the wrapper prints it; copy the line).
5. A breach (command killed by the cap) is a DATA POINT (report it), not a
   crash to retry around.
6. Old-vs-new comparisons: build both binaries FIRST, then run the matrix
   once, serially, each side bounded.

## Amendment (after the second box OOM, issue-22 battery)
7. NEVER run the OLD binary of an old-vs-new battery unbounded. Its known
   regressions are the thing being measured; a cap breach IS the datum.
   Old-binary heavy probes: cap at 4096M; if killed by the cap, report
   "exceeds 4G" instead of a wall time. Use known-good measured numbers
   from earlier runs (e.g. tree 100k old = 18.2s / 19.7 GB, already on
   record) instead of re-measuring.
8. Box budget: ~14G headroom total (sglang TP4 holds ~45G of 60G). Total
   concurrent allocation across ALL processes in a battery must stay
   under ~8G. One battery at a time means exactly that: no other heavy
   process may run alongside (that includes the agent's own builds —
   cargo release builds of this repo peak ~200M, fine).
9. Depth/scale probes on the NEW binary are bounded at 8192M; if the new
   binary breaches, that is a finding (issue 22 is exactly this).
