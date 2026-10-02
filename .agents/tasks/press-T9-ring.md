# Press test T9 — real refactors in ring (local model, cljform-only edits)

Repo: /tmp/press/ring (scratch clone — mutate freely, DO NOT commit).
Binary: cljform on PATH. Tools: clj_tree/clj_get/clj_edit/clj_forms/clj_draft only —
NEVER use the built-in edit/write tools on .clj files. You may read files freely.

Mission: perform the six refactorings below in ring-core/src/ring/util/response.clj
and request.clj, as a working developer would. Chase `next handle: ⟦…⟧` — do not
refetch a form between consecutive edits to it (refetch only when clj_edit refuses).

1. response.clj `not-found`: give it a docstring via patch (oldText/newText).
2. response.clj `redirect`: TWO consecutive patches to the same form (e.g. tighten
   the docstring, then adjust a body form) — second must use the returned handle
   with NO refetch in between.
3. response.clj: insert a new `(defn gone ...)` styled like its neighbors AFTER
   `not-found` (insert-after).
4. response.clj: rename the private helper `directory-transversal?` (yes, it is
   misspelled) to `directory-traversal?` and update its call site in the same
   file (two separate edits; locate the call site first).
5. request.clj `content-length`: replace the whole form with a version whose body
   you slightly extend (replace mode, submit at column 0 — the tool reindents).
6. request.clj: insert-before `set-context` a one-line `(def ^:dynamic *context*)`-
   style placeholder def of your choosing.

Rules:
- If a tool refuses, that is DATA: record the exact refusal and how you recovered.
  Do not fight a refusal with raw-text tools; recover with clj_edit means only.
- Do not fix anything in the cljform repo. If you suspect a tool bug, record a
  repro and move on.

Deliverable report:
- Edit ledger: for each edit — op, target, handle used, refused? (why), recovered how.
- Friction log: anything confusing, slow, or that made you consider raw-text tools.
- Metrics: total clj_edit calls, refusals, refetches, next-handle chases.
- Verification: `cljform check` on both files (parse ok), `cljform format` on both
  (must be empty = format-canonical), and `git -C /tmp/press/ring diff` pasted
  (or summarized precisely per hunk).
