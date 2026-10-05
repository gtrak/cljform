# Issue 32 — response scoping: result-first, no whole-file table dumps

Owner-directed. The friction family (F16 twice-reported, F17, T9's F2):
human-mode responses on real files bury the payload under whole-file state.
Today `edit --human` prints the ENTIRE post-edit forms table (161 lines on
db.cljc) before the diff; `get --json` returns the full forms array beside
the single result; `format --human` on an already-canonical file dumps the
full candidate (~350 lines) with an empty diff — misread as "changed
everything" (T9). Base: current main, 194 tests.

## A. `edit --human` — result-first, table gone
New order:
1. changed-region diff (when present — issue 27)
2. summary line (unchanged text: `patched form ⟦H⟧ label (lines a–b) —
   C changed, U untouched`)
3. the AFFECTED form's row(s) only — the new form for replace/patch, the
   inserted form(s) (+ anchor) for inserts, the deleted form's label +
   `(was lines a–b)` for delete — in the same table-row format, with handle
4. counts line: `N forms; C changed, U untouched — tree <file> for the full
   table`
5. notes, then warnings (D-lines) — result-adjacent, not after a table
NO full-table dump in human mode, at any file size. The next-handle
affordance lines (issue 15) stay exactly where they are.

## B. `get` — payload first
- `--human`: the form's exact bytes first (that IS the payload), then one
  compact metadata line (handle, kind, name, lines), then warnings. No
  forms-table dump.
- `--json`: keep `result`; REPLACE the whole-file `forms` array with a
  `formsCount` integer. Breaking change is acceptable (house rule) — but
  AUDIT consumers first: the wrapper's clj_get path, tests, and any code
  that reads `forms[]` from a GET envelope (note: the EDIT envelope's
  `forms` array STAYS — it is the summary-vs-table cross-check that caught
  issue 19's stale-head bug; document that rationale in SPEC §4.2).

## C. `format --human` — the unchanged case gets one line
- Candidate == input -> `already canonical (no changes)` (+ any notes).
  No candidate dump, no diff headers.
- Changed -> candidate-first + diff + note, exactly as today (candidate is
  the payload there).

## D. Related-surface audit (the "look for related issues" mandate)
While in the response-shaping code, audit and fix or report:
- `materialize --human`: candidate + diff is the payload — leave, unless
  the unchanged case dumps redundantly (apply the C rule if so).
- `edit --json`: forms array stays (rationale above); confirm no OTHER
  whole-file arrays ride in envelopes (warnings are per-file by nature — fine).
- Ordering of notes vs warnings in every human op (consistent: result,
  then notes, then warnings).
- The wrapper's rendering of the reshaped outputs (clj_edit/clj_get/clj_
  forms pass-through paths) — update any code that assumes the table.
- Anything else the audit turns up: fix if mechanical + behavior-preserving
  for healthy files' JSON, else REPORT as a follow-up candidate.

## Acceptance
- Tests: result-first ordering; affected-row presence (replace/insert/
  delete variants); counts line; format unchanged short-line; get payload-
  first + formsCount; no whole-file table in any human output (fixture with
  5+ forms). EXISTING tests that assert the removed table formatting may be
  updated ONLY for that; summary/diff/count assertions preserved.
- Gates: build, clippy (issue-30 gate), cargo test (194 + new) green;
  node --check; hx harness for wrapper paths.
- Battery (serial, bounded): JSON envelopes byte-identical EXCEPT get's
  forms->formsCount (enumerate); human edit/get/format outputs = intended
  diffs (the reshapes); everything else identical.
- SPEC §4.2/4.3/10.3 + README examples updated (README's tour output was
  captured pre-change — re-capture the edit/get examples so they show the
  new result-first shape).

## Report
Per-section implementation, the consumer audit (who read forms[] from get),
test list, gates, battery intended-diff list, anything D. turned up.
