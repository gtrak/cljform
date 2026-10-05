# Issue 34 — wrapper clj_edit double-renders the changed-region diff
(QUEUED behind issue 32 — one writer; both touch the wrapper. Found by the
owner on a post-issue-27 build: two diffs in one clj_edit response, an
"output" block with a diff and a `diff (changed region):` block with the
same diff.)

## Root cause
extension/clojure-forms.ts runEdit (~line 728): `lines = [r.text ?? "done"]`
— r.text is the CLI's human output, which since issue 27 ALREADY embeds the
changed-region diff (run_edit appends it to result.text; patch always did).
Then lines 734–738 append `diff (changed region):` + r.diff — the same diff
a second time. The issue-27 worker renamed the stale `patch diff:` label but
missed that r.text now carries the diff.

## Fix
Render the diff exactly once. Preferred: drop the wrapper's separate
r.diff block when r.text already contains it:
`if (r.diff && !(r.text ?? "").includes(r.diff)) { ... }` — defensive
against both old binaries (r.text without diff -> block renders) and new
(r.text with diff -> block suppressed). The `r.repairDiff` block is a
DIFFERENT diff (the pre-repair candidate) — keep it unconditionally.
Audit the sibling renderers for the same pattern (clj_get/clj_forms/clj_draft
labels vs their r.text payloads) — fix any identical duplication.

## Also identify the "output" header
The owner saw an `output` header around the first diff — grep the wrapper
and the clojure-worker agent doc for its source (it may come from the agent
doc's tool-result formatting rather than the wrapper). If it is wrapper-
emitted, label the single remaining diff consistently; if it is the agent
doc's example formatting, update the doc to show one diff.

## Acceptance
- hx harness: clj_edit on a real fixture returns EXACTLY ONE diff occurrence
  (count `--- before`/hunk bodies in the tool text); old-binary + new-wrapper
  and new-binary + new-wrapper both single-diff (the containment check must
  not break old-CLI rendering); patch, replace, insert, delete modes.
- node --check; no CLI changes; battery not needed beyond the harness cells
  (wrapper-only change) but run the standard clj_edit harness suite.
- Report: the output-header source, the fix diff, harness cell results.
