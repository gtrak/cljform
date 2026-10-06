# Issue 37 — edit diagnostics: batch labeling, indent deltas, head-change note, sub-form steering
(Owner feedback from a real session, ordered by cost. All four are
diagnostic/affordance work — refusal semantics unchanged everywhere.)

## A. Batch abort must not read as accomplishments
When a batch fails at op N, ops 1..N-1 print "patched" with diffs — the
owner trusted them and discovered three steps later that NOTHING persisted
(atomic design). Fixes:
- On abort, every per-op block is relabeled: `would apply (not written —
  batch aborted at op N)`. The abort line leads the response.
- On SUCCESSFUL multi-op batches, append a final **end-state echo**: one
  entry per DISTINCT touched form (a form edited by 3 ops appears once) —
  its post-batch handle, label, line span, and the first line of its final
  bytes — so all intended changes are verifiable in one place.

## B. patch-not-found: report indentation deltas
Three failures were re-typed continuation lines off by one space. When the
refusal fires, compute a diagnosis: normalize leading whitespace per line
of oldText; if the whitespace-normalized oldText matches EXACTLY ONE region
within the target form's bytes, report per-line deltas for that region
(`line 3 of oldText: expected 21 leading spaces, got 20`); if it matches
zero or multiple regions, keep the current message (no guessing). The
refusal stands; this is diagnosis, not inference. Token-sequence matching
(ignoring whitespace anywhere) is explicitly out of scope — riskier.

## C. Replace: note on head-symbol change
The owner passed a handle on the outer `(is ...)` wrapper believing it was
the call; the replace silently consumed the assertion wrapper (parses, diff
looks plausible, lost test). When a replace's NEW content's head symbol
differs from the target form's head symbol, append a note:
`note: form head changed: is -> request-cache/get-or-compute (check you
targeted the intended form)`. Advisory note, not a warning (head changes
are legitimate — defn->def happens); do not gate --strict on it.

## D. patch-not-found: steer to sub-form handles
The owner's most reliable mode turned out to be replacing by sub-form
handle (no oldText at all). On patch-not-found: if the oldText (trimmed)
matches EXACTLY ONE nested sub-form's bytes in the target form (the node
table has nested handles), append:
`hint: this region is sub-form ⟦xxx⟧ (defn …) — replace it by handle`.
Composes with B (both hints when both fire). Non-inferential: the sub-form
exists with those exact bytes.

## Acceptance
- Tests per item: abort relabeling + no "patched" language on failed
  batches; end-state echo on multi-op success (incl. same-form sequences
  collapsing to one entry with the FINAL handle); indent-delta diagnosis
  (one-space-off continuation line -> delta reported; zero/multi normalized
  matches -> no diagnosis, old message); head-change note (is->other fires;
  defn->def fires; same-head silent); sub-form hint (exact sub-form bytes
  -> hint with handle; partial bytes -> no hint).
- Battery (serial, bounded): healthy no-flag scenarios byte-identical
  EXCEPT the intended output additions (enumerated); refusal envelopes gain
  hint text only where B/D fire.
- Gates: build, issue-30 lint gate, cargo test, node --check (wrapper
  untouched unless A's rendering lives there — it does: batch abort
  relabeling is wrapper + CLI; keep both consistent).
- SPEC §10.3 (batch abort rendering, head-change note, patch-not-found
  hints), README unchanged unless the tour shows patch-not-found (it does —
  update the example if the hint text changes).

## Report
Per-item implementation, test list, gates, battery intended-diff list.
