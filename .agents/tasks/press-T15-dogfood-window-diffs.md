# Press T15 — dogfood the post-T14 agent-visible changes (light resources)

Repo: /tmp/press/t15 (fresh `git clone -q --depth 1
https://github.com/clj-kondo/clj-kondo /tmp/press/t15`; scratch, DO NOT
commit; corpus/ off limits). cljform on PATH. Binary state: issue 30 hardening
is in — a panic would surface as an `internal-error` envelope, which itself
would be a finding (none expected).

RESOURCE DISCIPLINE (binding): serial; cljform through
scripts/bounded-run.sh 2048; NO generated deep files, NO scale probes, NO
old-vs-new batteries. All work on real source files.

Mission: work as a developer making real edits in clj-kondo's big files,
exercising exactly the changes T14 never saw:

1. PAGED DISCOVERY (issue 28): in src/clj_kondo/impl/analyzer.clj (4744
   lines), use `tree --start-line/--end-line` to explore — page 3+ times
   (e.g. 1–80, a middle band, a tail band). Verify each block carries TRUE
   file line ranges (correlate: pick a form whose lines the window
   expanded — the header's requested-vs-effective echo — and confirm the
   label's real range), copy a handle, and edit it.
2. --name CONTEXT (F14): pick a nested named form; confirm the
   `inside form ⟦…⟧` context line gives you the enclosing form's handle
   directly (edit the enclosing form via that handle WITHOUT a second
   tree call — was impossible in T14).
3. WHOLE-FORM DIFFS (F12): do one replace, one insert, one delete and
   verify EACH from the tool's own returned diff — the T11/T14 complaint
   was having to reach for git diff. Explicitly do NOT use git diff for
   verification. Judge: is the diff sufficient to confirm correctness
   (right bytes changed, right position)?
4. EOF DELETE (F13): delete the LAST form of a file and check the returned
   diff + resulting tail: no trailing-blank residue expected.
5. One handle chase (>=2 consecutive edits, returned handles only).

Rules: cljform tree/get/edit/format only; never built-in edit/write on
.clj; refusals and internal-error envelopes are data (record verbatim);
suspected tool bugs get a repro; touch nothing outside /tmp/press/t15.

Deliverable: edit ledger (op/handle/diff-sufficiency), friction log (do
the four changes close T11's/T14's complaints? what still itches?),
metrics (edit calls, refusals, refetches, chases, paging calls), and a
one-paragraph verdict per change (paging / --name context / whole-form
diffs / EOF trim): adopted into workflow, or still bypassed?
