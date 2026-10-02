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
