# Press T19 — dogfood the verdict line on a warning-heavy file (issue 38)

Repo: /tmp/press/t19 (fresh `git clone -q --depth 1
https://github.com/clj-kondo/clj-kondo /tmp/press/t19`; scratch, DO NOT
commit; corpus/ off limits). cljform on PATH — has the warning-delta
verdict (issue 38), batch edit (36), response scoping (32), form-index
default (33).

RESOURCE DISCIPLINE: serial; heavy calls through scripts/bounded-run.sh
2048; no deep probes; real files only.

Mission: the behavioral question this dogfood answers — **does the verdict
line change re-verification behavior on a warning-heavy file?** The
baseline (T15/T16-era behavior): agents on analyzer.clj saw 21 unlabeled
D2 warnings and had to reason "corpus facts, not mine" — sometimes
re-verifying.

1. BASELINE COUNT: before editing, note how many pre-existing warnings
   analyzer.clj carries (cljform check) — that is the P in every verdict.
2. THE CLEAN-EDIT LOOP: make 4-5 real edits (docstrings, small body
   tweaks) via clj_edit on DIFFERENT forms, each response expected to lead
   with `verified — … warnings: 0 new (P pre-existing)`. For EACH edit,
   record honestly: after reading the verdict, did you feel any pull to
   re-run tree/get/check to confirm the file's health? Did you re-verify?
   (The T15/T17 baseline: redundant tree/get calls after clean edits.)
3. THE ATTRIBUTION CASE: make ONE edit that deliberately introduces a new
   nesting warning → the response must lead with the new warning labeled
   as yours. Then FIX it (the response should show the warning as
   resolved — `warnings: 0 new (P pre-existing; R resolved)` if
   supported). Record both responses verbatim.
4. THE TRAP: edit a form whose body ALREADY contains a D2 warning
   (pre-existing on the form you're touching) — the verdict must still
   say 0 new (the warning is pre-existing even though the form is in your
   diff). Record verbatim; this is the classification's sharpest case.
5. Close with one batch edit (issue 36) touching 3+ forms on the same
   file — verify the batch's verdict aggregates the delta correctly.

Rules: clj_tree/clj_get/clj_edit/clj_forms only; never built-in edit/write
on .clj; refusals are data; touch nothing outside /tmp/press/t19.

Deliverable: ledger (edit → verdict line verbatim → re-verify? y/n),
friction log (does the verdict read as trustworthy? any case where it
was ambiguous?), metrics (edit calls, re-verification calls after clean
edits — THE number — refusals, chases), verification (parse-ok per edit),
and the one-paragraph answer: did the verdict line change how you
allocated attention?
