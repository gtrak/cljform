# Press test T11 — monsters in clj-kondo (local model, cljform-only)

Repo: /tmp/press/clj-kondo (scratch clone — mutate freely, DO NOT commit; do
not touch corpus/ — two of its files are unparseable by design).
Same rules: cljform tree/get/edit/materialize/format/check only; never built-in
edit/write on .clj/.cljc. Chase returned handles; refetch only after refusals.
Record refusals verbatim; suspected tool bugs get a repro, not a fix.

Targets:
1. SCALE: `src/clj_kondo/impl/analyzer.clj` is 4744 lines. Measure (use `time`)
   `cljform tree` (default), `tree --depth 1`, and `tree --json` on it, then
   patch a form chosen from the LAST 100 lines of the file, then measure
   `cljform edit` wall time. Report the timings — slow is a finding, not a
   failure. Verify the file parses after.
2. #_ DISCARD: find a `#_` reader discard in analyzer.clj or
   impl/analyzer/namespace.clj. Confirm `tree`/`get` do NOT surface the
   discarded form as an editable target (record what tree shows around it).
   Patch the real form that follows a discard; confirm the discarded form's
   bytes survive byte-identical afterward (git diff shows only your change).
3. VENDORED ODDITY: `src/aaaa_this_has_to_be_first/pprint.clj` — pick a form
   with unusual internal layout and patch inside it precisely.
4. TWO FORMS ON ONE LINE: search the src tree for a line carrying two
   top-level forms (worker's choice how — e.g. `grep -rn` heuristics on short
   files). If genuinely none exist, create the case: append to a scratch copy
   of a small file two top-level forms on ONE line, then insert-after the
   FIRST of them by handle. The output must split the line cleanly — both
   original forms still on-parse, no reflow of the other form. Record exactly
   what the seam did.
5. DATA DEPTH: `src/clj_kondo/impl/types/clojure/core.clj` (1316 lines, mostly
   nested data). Patch one deeply nested leaf; then a second patch to the same
   form via the returned handle with NO refetch.
6. RAPID-FIRE HANDLES: 10 consecutive patches to the SAME form in analyzer.clj
   (e.g. its ns form or a mid-file defn), each using the handle returned by the
   previous edit — zero refetches for the whole chain. If any rotation surprises
   you, record it.
7. Cost check: `time cljform check` and `cljform format` on analyzer.clj;
   format must be empty-diff (canonical) for every file you touch.

Deliverable report: edit ledger, friction log, metrics (edit calls / refusals /
refetches / handle-chases + the timings from 1 and 7), verification (check
parses + format empty on every touched file; git diff summarized per hunk),
and specific answers for drills 2 and 4.
