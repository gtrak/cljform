# Refactor-consolidate pass (global, post issue-16)

Skill: refactor-consolidate (gtrak/agent-skills), Option C broad scan, run
with delegation. Four read-only scout scans completed (structural, boilerplate,
test-redundancy, code-duplication); findings consolidated below. Preservation
requirements for every task: all tests pass unchanged (the suite is
golden-heavy and CLI-level, so **zero test-file changes** are expected for
C1/C3/C4 — if a cleanup needs a test change, it changed behavior and is
wrong); error messages and JSON envelopes byte-identical; public CLI surface
unchanged; `cargo build` · `cargo clippy --all-targets -- -D warnings` ·
`cargo test` green after every task.

## Task sequence (one worker task each, review + commit after each)

- **C1 (mechanical, Rust)** — collapse error/output boilerplate:
  `ErrorBody::new(code).with_hint(..).at(line, col)`-style constructors over
  the ~30 `Fail(n, ErrorBody { six-field literal })` sites; `Output::ok(op)`
  builder over the ~10 nine-field constructions; `mod exit` named exit codes
  (SPEC exit-table audit); `print_payload` for the 4 print_human
  newline-if-missing sites; fold the duplicated "stripped view markers" note
  into one `strip_and_note`; inline the `indent_mode_full` alias; drop the
  `reindent_dedent` alias (call `reindent_dedent_by` directly); hoist the
  `target.addr.to_string()` in the `form_handle` predicate.
  Acceptance: zero test changes; all gates green.
- **C2 (tests)** — consolidate the harness into `tests/common/mod.rs`:
  absorb the duplicated fixture constants (`HEDIT_FIXTURE` (byte-identical in
  edit.rs + handle.rs), `GOLDEN_FIXTURE` (forms.rs + format.rs),
  `FRESH_FIXTURE` (edit.rs MULTI_FIXTURE + format.rs CLI_FIXTURE vs
  `common::fresh`), BOM/CRLF/bracket-look-alike fixtures, the 20k-deep
  payload builder); make ONE `(line, depth)` full-tree lookup
  (`tree_full_nodes` + `handle_at_full`) replacing the three parallel
  implementations (edit.rs `handle_at_path`, handle.rs local trio, common's
  non-full `handle_at` stays for top-level-only callers); promote
  repair.rs's `edit_content` shape to a shared edit-invocation helper and
  rewrite the three table-driven sites (edit.rs ×2, canonical.rs).
  Acceptance: no assertion lost — the two ambiguous-name and two
  short-handle tests are intentionally distinct (keep both); test count may
  change only by removing truly identical functions (scans found none);
  every suite green.
- **C3 (structural-lite, Rust)** — pure code motion in `dispatch`: extract
  `run_forms`/`run_get`/`run_check`/`run_materialize`/`run_tree` at the same
  seam as the existing `run_strip`/`run_format`/`run_edit`; add
  `load_parsed` (read_file + parse_or_fail, 5 copies) and a `require_parse`
  variant for tree's discarded result; one positional-error mapper
  (`line, col, message` → Fail) shared by materialize/format/parse (follow
  `PrepareError::line_col`'s lead). Acceptance: zero test changes.
- **C4 (structural, Rust)** — decompose `run_edit` and move coherent blocks:
  extract `resolve_edit_target`, `build_patch_payload`,
  `build_prepared_payload` (incl. `enum BaseShift` at module level),
  `plan_splice`, `verify_edit` from run_edit's ~870 lines; move the
  seam/base-shift helpers (~230 lines) to `src/seam.rs` and the summary
  builders + `label`/`handle_label`/`human_summary` to `src/summary.rs`
  (`pub(crate)`, no signature changes); move `find_all` next to its callers.
  The inline `#[cfg(test)]` unit tests in main.rs move with their code.
  Acceptance: zero test changes; gates green.
- **C5 (TS wrapper)** — `runClj(args, timeout)` helper replacing the
  verbatim execCljform→parseEnvelope→error-fallback block in all six tools +
  the guard hook variant; `withTempFile` for the two temp-file sites; delete
  dead branches (`cacheKey` identity fn, unread `at` field, always-true
  `.length >= 0`, empty-if in clj_edit's content check); single `MODES`
  source for the literal union + guideline text.
  Acceptance: `node --experimental-strip-types --check` green; end-to-end
  harness on all six tools shows identical success/error outputs before and
  after (byte-compare the tool outputs on a fixture; note the typebox
  resolution trick: symlink node_modules/@sinclair/typebox → the installed
  `typebox` package, as done in earlier verification).

## Deferred (design decisions, report-only)
- D2 read unification (strip's read_to_string keeps BOM, read_file strips —
  unifying changes behavior on BOM files; needs an explicit decision).
- D5 materialize.rs's two near-identical byte scanners (edge-case-sensitive).
- D9 `split_lines` for the reindent trio (join/blank policies differ per
  caller; issue-14 seam semantics depend on exact behavior).
- D10 handle.rs/parser.rs form-table preamble (the tables key different
  types; needs a designed abstraction).
- TS file split into modules (extension loader expects one entry file).
- print_human per-op render dispatch (string-keyed match).

## Outcome (all five tasks completed, reviewed, committed)
- C1 (b8c7694): ErrorBody/Output constructors + mod exit over ~40 literal
  sites; print_payload; strip_view_markers_into; aliases inlined/dropped.
  Zero test changes; 70-scenario byte-identity battery PASS. main.rs -101.
- C2 (6c3ea8b): tests/common/mod.rs is the single harness home; CRLF/bracket
  variants kept distinct (form-count assertions differ). 100 tests before
  and after; every assertion preserved. tests -57 net lines.
- C3 (57f1605): run_forms/run_get/run_check/run_materialize/run_tree at the
  run_* seam; load_parsed/require_parse; positional_body. Zero test changes;
  53-scenario byte-identity battery PASS.
- C4 (9c6ba61): run_edit -> 164-line pipeline of five steps (bodies
  verbatim, pipeline order preserved); src/seam.rs (259) + src/summary.rs
  (250); main.rs 2354 -> 1951. Zero test changes; 54-scenario/216-artifact
  byte-identity battery PASS.
- C5 (ce329e3): runClj + withTempFile + MODES; dead branches removed;
  30-case before/after harness + tool metadata byte-identical. -126/+108.

Latent bug found and NOT fixed (behavior-preserving by design; separate
decision needed): the guard hook's shape-delta line is unreachable —
remember() writes the forms table before cache.get() reads it, so
shapeDelta always compares the table against itself and returns null.

Deferred (design decisions, unchanged from the plan above).
