# Issue 35 — label the inserted-handles list: which handle is which form
(QUEUED behind issue 34 — same renderer, one writer. Issue 34 fixes the
double-diff; this fixes the ambiguity of the inserted-handles list.)

## Problem (owner-identified)
After a multi-form insert, the wrapper emits
`inserted handles: ⟦a⟧, ⟦b⟧, ⟦c⟧ (anchor ⟦d⟧) — use these for the next edit`.
The list is in document order, but each handle is an opaque hash with no
binding to WHICH inserted form it names. The identity lives only in the
adjacent affected-rows block (issue 32), forcing a mental join between two
blocks — and for visually similar inserted forms (three `defn`s) the join
is error-prone. Single-form inserts are unambiguous (the summary names the
handle); the gap is specifically multi-form.

## Fix
Make the line self-sufficient — per-entry labels from data the insert
summary already has:

```
inserted after ⟦8b7d6c⟧:
  ⟦35a60d⟧ defn gone (lines 7–13)
  ⟦25d9dc⟧ def delta (lines 14–15)
  ⟦1197b5⟧ defn apply (lines 16–19)
— use these for the next edit
```

- CLI (src/summary.rs, insert action): the JSON summary gains per-entry
  labels alongside `handles` — e.g. `inserted: [{handle, head, name,
  line: [a,b]}...]` (additive key; keep the existing `handles` array for
  compatibility or replace it outright — house rule prefers one way, so
  REPLACE, updating the one consumer: the wrapper's runEdit).
- Wrapper (runEdit): render the labeled rows (see sketch) instead of the
  bare comma list. Anchor line stays. The `— use these for the next edit`
  instruction stays.
- Affected-rows block (issue 32) is unchanged and now redundant for
  inserts — ACCEPTABLE (rows are the general mechanism; the labeled list
  is the insert-specific instruction). Do not remove the rows.

## Edge cases
- Nested inserts: the top-level forms table does not contain them; the
  labels must come from the summary builder's own view of the inserted
  nodes (it has them — the affected rows prove it), NOT from out.forms.
- Single-form insert: the labeled form collapses to one row; the summary
  line's `next handle` still covers it — the insert block may be omitted
  for single-form inserts if that reads cleaner (decide by looking at
  real output; report the choice).
- Delete/replace/patch: untouched.

## Acceptance
- hx harness: multi-form insert (3 visually similar defns) -> each labeled
  row present, handles match the affected rows, anchor named; edit by the
  SECOND listed handle targets the middle form (the ambiguity is dead).
  Single-form insert per the decision above. Nested insert labels correct.
- JSON: additive/changed keys enumerated; battery: healthy no-flag CLI
  scenarios byte-identical EXCEPT the insert summary JSON + human reshapes
  (intended diffs).
- Gates: build, clippy (issue-30 gate), cargo test green, node --check.
- SPEC §10.3 (the insert response contract) + the wrapper doc if the
  example there shows the old bare list.
