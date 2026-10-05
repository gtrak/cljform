# Press T16 — dogfood the broken-file recovery flow (merge conflicts, real git)

Repo: /tmp/press/t16 (fresh `git clone -q --depth 1
https://github.com/clj-kondo/clj-kondo /tmp/press/t16`; scratch, DO NOT
commit; corpus/ off limits). cljform on PATH (has issue 31: conflict-marker
gate + tree --recover).

RESOURCE DISCIPLINE: serial; heavy invocations through
scripts/bounded-run.sh 2048; no deep probes; everything on real files.

Mission: exercise the REAL merge-conflict workflow with real git conflicts —
not hand-written markers — and judge whether the recovery flow keeps an agent
inside a structured process.

1. REAL CONFLICTS: create two branches from the same commit, edit the SAME
   regions of 2-3 real source files differently on each branch (e.g. change
   different functions in src/clj_kondo/impl/analyzer.clj and one more
   file), merge → conflicts. `git status` first, then cljform.
2. THE GATE: `cljform check` on the conflicted repo files → expect
   conflict-markers refusal naming REGIONS (not just lines). Record the
   exact messages. Try `cljform edit` on one side's form → expect refusal
   (this was the safety hole; verify it is closed).
3. THE VIEW: `cljform tree --recover` on the worst file → verify
   segmentation: common-region forms labeled as merged content, each
   conflict region with side attribution (HEAD/base/incoming), true line
   spans. Judge: could you plan the resolution from this view alone?
4. PROGRESSIVE RESOLUTION: resolve the conflicts one at a time with the
   built-in edit tool (THE DESIGNATED RECOVERY TOOL — cljform's write path
   is gated by design). After each resolution, `cljform check` → the
   remaining-region count must decrease. Resolve the LAST conflict
   deliberately WRONG (leave unbalanced parens) → expect the layered
   diagnostics to flip to parse-error (not conflict-markers) → fix it →
   check ok.
5. RESUME: once the file parses, `cljform tree` (real handles!) → make one
   normal cljform edit → format empty-diff. The loop is closed.

Rules: cljform check/tree/edit/format per the flow above; built-in edit is
AUTHORIZED ONLY for conflict-resolution text edits (that is the designed
workflow) — do not use it for convenience elsewhere; refusals and
internal-error envelopes are data; touch nothing outside /tmp/press/t16.

Deliverable: ledger (git ops, cljform ops, refusals verbatim), friction log
(was the gate message actionable? did --recover segmentation match the real
conflict structure? was the progressive count useful? anything that pushed
you to raw reading?), metrics (check/tree/edit/recover calls through the
resolution), verification (files parse, git diff summarized), and a
one-paragraph verdict per phase (gate / view / progressive / resume).
