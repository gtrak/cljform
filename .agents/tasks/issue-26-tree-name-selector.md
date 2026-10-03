# Issue 26 — `tree --name` selector: discovery for nested named forms

User-approved design (the parked nested-handle UX question). Discovery-only:
handles remain the ONLY edit address. One selector, one output convention,
zero new edit semantics. The two-case rule this enables (goes in worker
guidance): named nested form -> `tree --name X`, copy the handle, edit;
anonymous nested content -> patch within the enclosing named form's handle.

## Rust: `cljform tree <file> --name X` (src/ops.rs run_tree)
- Exact `name` equality against the node table's `name` field (def-likes;
  no substring matching). Works with or without `--full`; when `--name` is
  set, matched forms' subtrees are marked at full depth (reuse the --full
  marking machinery) — the point is seeing the inner handles.
- Human output: a count header line, then each match's annotated source
  block with its handle inline, sorted by line, with its line range.
- JSON output: the matching nodes (same node shape as tree --json) —
  filtered table.
- Zero matches: ok:true with an empty/zero-count result, NOT an error.
- Default `tree` output (no --name) must be byte-identical to before.

## Wrapper: clj_tree gains `name` (extension/clojure-forms.ts)
- Optional `name` string param; when present, append `--name X` to the CLI
  args. Tool description + promptGuidelines updated with the two-case rule
  above. node --experimental-strip-types --check green; verify the deployed
  symlink (~/.pi/agent/extensions/clojure-forms.ts -> repo file) picks it up
  via the hx-style harness (clj_tree with name on a nested-defn fixture
  returns the handle; without name, byte-identical to before).

## Tests / gates
- Rust CLI tests: single match, multiple matches (count + order), no match
  (empty ok), nested defn inside let (the headline case), --full
  interaction, default-tree-unchanged assertion, JSON + human.
- cargo build, clippy --all-targets -- -D warnings, cargo test green
  (124 + new).
- Byte-identity battery (serial, bounded-run): old vs new — every scenario
  WITHOUT --name must be byte-identical (the selector is additive); the
  --name scenarios are new-surface only.
- SPEC.md: document the selector in the tree op section (§ for tree).
