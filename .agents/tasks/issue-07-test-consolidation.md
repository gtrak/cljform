# Issue 07 — test consolidation

Depends on issue 04 (which migrates the suite to handle targeting). Read the
post-migration test files before editing; this spec describes the target
shape, not the current one.

## Problem
Seven files each re-declare the same helpers and several re-test the same
behavior:
- five copies of "run the binary and parse the JSON envelope";
- six copies of "write a temp fixture";
- duplicated clusters: BOM/CRLF/unicode, delete/insert comment-gap reseaming,
  detectors D1–D4, name extraction, `--expect` guard (removed in 04),
  untouched-form byte-identity.

## Deliverable
Shared helpers + thematic files + table-driven merges. **No coverage loss**:
every assertion in the current suite must still be asserted somewhere.

## 1. `tests/common/mod.rs`
Rust integration tests share code only through a subdirectory module, so put
it at `tests/common/mod.rs` and add `mod common;` to each test crate. Provide:
- `run_json(args: &[&str], stdin: Option<&[u8]>) -> (i32, serde_json::Value, String)`
- `run_bytes(args: &[&str], stdin: Option<&[u8]>) -> (i32, Vec<u8>, String)`
- `fixture(name: &str, bytes: &[u8]) -> String` — write into one temp dir
- `fresh(name: &str) -> String` — the standard multi-form fixture
- `forms(file: &str) -> Vec<Value>`, `tree_nodes(file: &str) -> Vec<Value>`
- `handle_of(file: &str, name_or_path: &str) -> String`
- `assert_untouched(before: &[Value], after: &[Value], allowed_addrs: &[u32])`

Delete every local copy of these.

## 2. Thematic files
| File | Contents |
|---|---|
| `cli.rs` | flags, exit codes, human/JSON rendering, usage errors |
| `forms.rs` (from `golden.rs`) | table stability, name extraction, `get`, detectors |
| `edit.rs` | replace/patch/insert/delete, comment-gap reseaming, untouched byte-identity, multi-form content, ambiguity |
| `repair.rs` | bracket repair + `materialize` |
| `robustness.rs` (from `adversarial.rs` + `fuzz.rs`) | deep nesting, BOM/CRLF/unicode, brackets-in-strings, garbage/no-panic |
| `handle.rs` | `tree`/`strip` + handle edits |
| `regression.rs` | the F1 swallowed-deftest anchor (keep the name) |

## 3. Table-driven merges
Collapse each cluster into one table-driven test (a `Vec` of `(name, input,
expectation)` cases), preserving every existing assertion:
- `line_endings_and_bom`
- `comment_gap_reseaming` (delete + insert around comments and same-line
  neighbors)
- `detectors_fire` (D1–D4, including the reader-conditional/inert cases)
- `name_extraction` (metadata, docstrings, custom def-macros, `defmethod`)
- `mutations_leave_untouched_forms_identical` (the byte-identity battery)

## 4. Rules
- Merging may reduce the *number* of test functions, never the number of
  distinct assertions. If unsure, keep both.
- Keep the deep-nesting (20k) and no-panic tests; they are cheap insurance.
- `cargo test` must stay green after each file move (do it incrementally).

## 5. Also (tiny README fix, same run)
`README.md` (~line 50) says `--strict` turns "detector warnings and content
repairs into refusals (exit 3, `repair-refused`)". That conflates two codes:
`--strict` detector warnings are exit 1 `detector-fatal` (`src/main.rs`),
while content repairs are exit 3 `repair-refused`. Split the sentence so each
has its correct code. Docs only; no code change.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Before/after: file list, test-function count, line count; the merged tables;
anything you deliberately left duplicated and why.
