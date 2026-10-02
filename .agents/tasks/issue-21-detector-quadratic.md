# Issue 21 — P-1: D1/D2 nesting detectors are O(depth^2) at extreme depth
T12 finding (confirmed: 50k-deep file -> check 35.8s, tree 39.2s; 20k is
fine, 20k siblings 0.18s). SPEC promises ms-scale. The ancestor scan walks
O(depth^2) when no host head matches. Fix direction: single pass carrying
ancestor state (the walker already descends linearly; accumulate depth/head
info on the stack instead of re-walking ancestors per node), or memoize
per-node depth. Acceptance: 50k-deep check+tree+edit < 2s each; all 100
tests green; byte-identity battery for tree output unchanged (D1/D2 warning
text identical).
