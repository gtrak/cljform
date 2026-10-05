# Press T17 — dogfood the response-shape generation (issues 32/33/34/35)

Repo: /tmp/press/t17 (fresh `git clone -q --depth 1
https://github.com/clj-kondo/clj-kondo /tmp/press/t17`; scratch, DO NOT
commit; corpus/ off limits). cljform on PATH — has the full response-shape
generation: result-first edit/get (issue 32), depth-honoring JSON +
large-file form-index default (issue 33), single-diff rendering (34),
labeled inserted-handles blocks (35).

RESOURCE DISCIPLINE: serial; heavy calls through scripts/bounded-run.sh
2048; no deep probes; real files only.

IMPORTANT context: clj-kondo's source is NOT parinfer-canonical — `cljform
format` shows a pre-existing diff there (proven pristine in T15/T16). Do
NOT treat format empty-diff as an acceptance bar; the invariants are:
format runs, token stream verified, nothing written.

Mission: a normal working session that exercises every reshaped surface
and judges whether the response-quality generation reads better:

1. INDEX DISCOVERY (33): `clj_tree` (no params) on
   src/clj_kondo/impl/analyzer.clj (4750 lines) → expect the compressed
   form index (header + ~159 live-handle rows, NO source body). Judge: is
   this a usable table of contents? Pick a target from the index and edit
   by its handle WITHOUT ever dumping the full annotated view.
2. RESULT-FIRST EDITS (32): do a replace, an insert, and a delete on real
   forms. For each: does the diff + summary + affected row + counts line
   give you everything in the first lines of the response (vs T15's
   buried-result complaint)? Explicitly confirm NO whole-file table.
3. SINGLE-DIFF (34): confirm each edit shows the changed-region diff
   EXACTLY ONCE (the pre-fix wrapper showed it twice).
4. LABELED INSERTS (35): one multi-form insert of 3 visually similar
   defns; verify the labeled block (handle + name + lines per entry);
   then edit by the SECOND listed handle and confirm it targets the
   MIDDLE form (the ambiguity-kill, in a real session).
5. GET PAYLOAD-FIRST (32): use clj_get for a form's bytes; confirm bytes
   lead and no forms array rides along.
6. One handle chase (>=2 consecutive edits on returned handles).

Rules: clj_tree/clj_get/clj_edit/clj_forms/clj_draft only; never built-in
edit/write on .clj; refusals + internal-error envelopes are data (record
verbatim); suspected tool bugs get a repro; touch nothing outside
/tmp/press/t17.

Deliverable: ledger (op/response-shape observed), friction log (per issue:
adopted or bypassed? anything STILL weird in the response shapes?),
metrics (edit calls, refusals, refetches, chases, paging/index calls),
verification (parse-ok after each edit; git diff summarized; NO
format-empty-diff claims — see the caveat above), and a one-paragraph
verdict: is this generation of responses better for a working agent?
