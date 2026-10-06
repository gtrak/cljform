# Issue 36 — batch edit: N ops, one call, no stale-handle churn
(Motivated by a real agent session: an ~8-form change (ns require, several
docstrings, two bodies) was made with the UNGUARDED built-in edit because
clj_edit is one-op-per-call. Owner framing: "stale handles create too much
back-and-forth for an edit like this." The batch removes both the
round-trip count AND the handle-chase.)

## Surface
`cljform edit <file> --batch <ops.json>` (also `--batch-file`? one flag;
ops via file or inline `--ops`). ops.json:

```json
[{"handle": "a1b2c3", "mode": "patch", "oldText": "...", "newText": "..."},
 {"handle": "a1b2c3", "mode": "patch", "oldText": "...", "newText": "..."},
 {"handle": "d4e5f6", "mode": "replace", "content": "(defn … )"},
 {"handle": "7890ab", "mode": "delete"}]
```

Every op is a normal edit op (all modes, all content flags) targeting a
pre-batch handle. `--dry-run` composes.

## Core semantics (the novel part)
1. **Handles resolve against the ORIGINAL file's node table** — the same
   handle keeps targeting the same structural form across the whole batch,
   even after an earlier op modified that form (position/identity tracking
   via the existing resolve_at/at_chain machinery — issues 19/22; this is
   what kills the stale-handle chase: docstring-then-body on one form is
   two ops, one handle, zero refetches).
2. **Sequential application, full verification per op** — the exact
   pipeline (prepare → splice → parse → untouched-forms byte-identity →
   detectors) runs per op on the evolving file. No weakening.
3. **Atomic**: any op fails → NOTHING is written; the error names the
   failing op index, its code, and the standard recovery affordances
   (patch-not-found exact bytes, unbalanced-content candidate, …), plus
   which ops succeeded logically before the abort (informational — the
   file is untouched).
4. **Target-loss is a clean failure**: if an op DELETES/replaces a form
   that a later op targets, the later op fails with a targeted message
   ("op 3 targets a form removed by op 1") — never a silent mis-aim.
5. **Response** (issue-32 shape): result-first; per-op blocks (diff +
   summary + affected row); one aggregate counts line
   (`N ops applied; file: M forms; C changed, U untouched`); notes;
   warnings. `--dry-run` renders the same without writing.

## Non-goals (v1)
- No cross-op templating/interpolation; no ops that create forms and then
  target them by new handle (create-then-edit sequences still use the
  returned `next handle` in separate calls). Document.
- No ordering guarantees beyond document application order; ops on the
  same form apply in listed order.

## Edge cases to test
- Same-form op sequences (docstring patch + body patch, one handle).
- Disjoint forms (the 8-docstring scenario): handles valid throughout.
- Op k deletes form F; op k+1 targets F -> targeted failure, nothing
  written.
- Batch where op 1's new content makes op 2's oldText ambiguous WITHIN
  the form -> patch-ambiguous, atomic abort.
- --dry-run; --repair inside one op; malformed ops.json -> usage.
- A batch on a 159-form file: response size stays scoped (issue 32 rules).

## Acceptance
- Rust tests: the edge matrix above; JSON envelope shape; atomicity
  (failed op => file bytes unchanged, verified).
- Harness: wrapper clj_edit gains batch param (ops array); a real
  8-docstring-style batch on a clj-kondo file lands in ONE call.
- Gates: build, issue-30 lint gate, cargo test, node --check.
- Battery (serial, bounded): single-op batches must behave byte-identically
  to the equivalent single edits (batch of 1 == edit); healthy no-flag
  scenarios byte-identical; batch cells are new surface.
- SPEC §4.3/§10.3 (batch contract, resolution semantics, atomicity);
  README one paragraph + clj_edit row; wrapper doc + guidance line
  ("multi-form change -> one batch").

## Report
Per-section implementation (especially the identity-tracking semantics),
test list, gates, battery intended-diff list.
