# Press test T10 — adversarial forms in datascript (local model, cljform-only)

Repo: /tmp/press/datascript (scratch clone — mutate freely, DO NOT commit).
Same rules as T9: clj_tree/clj_get/clj_edit/clj_draft only; NEVER built-in
edit/write on .cljc/.clj. Chase `next handle: ⟦…⟧`; refetch only after a refusal.
If a refusal surprises you, record it verbatim. Record suspected tool bugs with
a repro; do not fix anything outside the scratch repo.

Targets (all real code, gnarlier than ring):
1. src/datascript/conn.cljc ns form: the require line contains a reader
   conditional `#?@(:cljs [:refer [DB FilteredDB]])`. Patch that line to also
   refer (cljs-only) `Storage`… careful: pick a REAL var from datascript.db
   (look one up first); the point is editing INSIDE a reader conditional.
2. src/datascript/parser.cljc ~line 28: a syntax-quoted form with `~@` splice
   and `new#` gensyms. Patch a precise sub-form inside it (e.g. change the
   quoted symbol list or add a field to the `postwalk` call). Exact bytes
   matter — backquotes and splices included.
3. src/datascript/db.cljc (2092 lines): find the deepest nested literal you can
   (a big def/defn with nested maps or case-tree data) and patch one nested
   leaf. Then, WITHOUT refetching, make a second patch to the same form via the
   returned handle.
4. Ambiguity drill: construct an oldText that appears in TWO different forms of
   the same file (grep first to confirm) and patch WITHOUT a handle → expect
   `patch-ambiguous` refusal naming candidates; then redo it WITH a handle.
   Record the refusal text exactly.
5. Repair drill: pick a small defn, and in ONE step replace it with a version
   you submit DELIBERATELY missing its final closing paren, WITHOUT --repair →
   expect refusal with the candidate shown. Then resend the same content WITH
   --repair → the tool infers the closer from indentation. Verify the result
   parses and the paren count matches your intent. Record whether the inference
   placed the closer where YOU would have.
6. Bracket-lookalikes: insert-after some top-level form a new defn whose body
   contains BOTH a regex literal with an unbalanced-looking paren (e.g.
   #"^\($") and a string containing ") (" — brackets inside literals.
7. Seam drill: find a defn that has a preceding ; comment line. Delete the
   defn (delete mode). The comment must survive, no dangling separator
   weirdness; check the surrounding lines with a normal read.
8. Top-level ns form edit: patch the ns form of db.cljc to add one more
   :require entry (alphabetical position), then confirm the file still parses.

Deliverable report (same shape as T9): edit ledger, friction log, metrics
(edit calls / refusals / refetches / handle-chases), verification
(cljform check on every touched file = parse ok; cljform format = empty diff;
git diff summarized per hunk), plus: for drill 5, the exact repair outcome.
