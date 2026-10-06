# Press T18 — dogfood the batch workflow (issue 36, the 8-docstring scenario for real)

Repo: /tmp/press/t18 (fresh `git clone -q --depth 1
https://github.com/clj-kondo/clj-kondo /tmp/press/t18`; scratch, DO NOT
commit; corpus/ off limits). cljform on PATH — has batch edit (issue 36).

RESOURCE DISCIPLINE: serial; heavy calls through scripts/bounded-run.sh
2048; no deep probes; real files only.

Mission: replay the REAL failure that motivated the batch feature. A
previous agent, asked to add/extend docstrings across ~8 separate forms in
one file, abandoned clj_edit for the unguarded built-in edit tool because
clj_edit was one-op-per-call. You will do the equivalent change the
NEW way and judge whether the batch removes that temptation.

1. DISCOVERY: `clj_tree` (or the form index on this large file) on
   src/clj_kondo/impl/cache.clj (or another real clj-kondo file you pick —
   say why). Identify >=6 forms needing docstring improvements (missing or
   thin docstrings) + at least one pair of changes to the SAME form
   (e.g. docstring + small body tweak) — the same-form sequence is the
   stale-handle case the batch exists to kill.
2. ONE BATCH: build one ops.json covering ALL of it (>=8 ops, mixing patch
   and replace, including the same-form sequence sharing ONE pre-batch
   handle) and submit as a single `cljform edit --batch` call. Verify:
   parse-ok, the labeled per-op results, the counts line.
3. ATOMICITY EXPERIENCE: deliberately include ONE op that will fail
   (a patch oldText with a deliberate typo) in a first attempt — observe
   the abort, the op-naming, that NOTHING was written, and how you
   recover (fix the one op, resubmit — do the OTHER ops' handles survive
   the resubmit? they should: content-addressing).
4. TARGET-LOSS (optional but interesting): include a delete + a
   follow-up op on the deleted form → observe target-removed.
5. JUDGE THE TEMPTATION: after the batch flow, answer honestly —
   would you still reach for the built-in edit for this change? What's
   STILL easier with raw text editing (e.g. pure-whitespace seam fixes,
   comment insertion — things cljform intentionally doesn't do)?

Rules: clj_tree/clj_get/clj_edit/clj_forms only for ALL form edits (the
batch is the point); built-in edit NOT used at all this session (unlike
the motivating session); refusals + internal-error envelopes are data;
suspected tool bugs get a repro; touch nothing outside /tmp/press/t18.

Deliverable: ledger (ops count, one-call?, refusals verbatim, recovery
steps), friction log (per phase: discovery / batch build / atomicity /
temptation judgment), metrics (clj_edit calls, tree calls, refetches,
chases), verification (parse-ok, git diff summarized), and the verdict:
does the batch close the tool-choice gap?
