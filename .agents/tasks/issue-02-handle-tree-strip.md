# Issue 02 — handle core + `tree` annotated view + `strip`

Design authority: `SPEC.md` §5, §10.1, §10.2, §10.4.

## Deliverable
A new `cljform tree` read op and a `cljform strip` filter, plus the handle
computation they share. No edit-path changes (issue 03).

## Read first
- `SPEC.md` §5 (Addressing) and §10.1/§10.2 (handle + annotated view).
- `src/parser.rs` — how nodes are walked, `Form`, the big-stack worker
  (`with_big_stack`), `head_symbol`.
- `src/hashutil.rs` — blake3 helpers.
- `src/main.rs` — `Op`, `dispatch`, `Output`, `static_op_name`, `read_file`,
  `read_content`, `parse_or_fail`, `print_envelope`/`print_human`.
- `tests/adversarial.rs` and `tests/fuzz.rs` — fixture style for the round-trip.

## 1. `src/handle.rs`

```rust
pub struct Node {
    pub path: String,        // "2", "2.1", "2.1.3" — informational
    pub kind: String,        // list_lit | vector_lit | map_lit | set_lit | anon_fn_lit | ...
    pub head: Option<String>,// leading sym for list forms, else None
    pub name: Option<String>,// def var name for top-level def forms, else None
    pub line: [usize; 2],
    pub handle: String,      // shortest unique prefix, >= 6 hex chars
    pub raw: String,         // full 64-hex key the handle is a prefix of
    pub start_byte: usize,
    pub end_byte: usize,
    pub depth: usize,        // collection nesting: top-level form = 1
}
```

- `pub fn collect(bytes: &[u8]) -> Vec<Node>` — every **collection** node in
  document order. Collection kinds are the list/vector/map/set/anon-fn literals
  the grammar produces (discover the exact kind names from tree-sitter; include
  `#(...)`, `#{...}`, `#:ns{...}`). Run on the big-stack worker like
  `parser::parse`.
- **Position path**: top-level form index (1-based, over the same children
  `parser::build_table` treats as forms) then 0-based indices among each
  parent's `named_children`, dot-joined.
- **Handle algorithm** (exact):
  1. `content = &bytes[start..end]`; `ch = blake3(content)` hex.
  2. Count nodes sharing each `ch`. If a node's `ch` is unique among all
     collected nodes, `raw = ch`.
  3. Otherwise `raw = blake3(position_path.as_bytes() ++ content)` hex.
  4. `handle` = the shortest prefix of `raw` (length >= 6) unique among all
     nodes' `raw`. Extend on collision (git-style). Keep `raw` on the node so
     resolution can also accept a longer (full-hash) input.
- `pub fn annotate(bytes: &[u8], depth: Depth) -> Result<String, AnnotateError>`
  where `Depth = Heuristic | Levels(usize) | All`. Inserts `⟦handle⟧`
  immediately after the **opening delimiter** of each marked node (find the
  first byte in `([{` at/after `start_byte`; for `#(`/`#{` that is `start+1`).
  Marking rules:
  - `Heuristic` (default): every top-level collection is marked; a nested
    collection is marked **iff it spans >= 2 lines**; descend only into marked
    collections (sound: a single-line form cannot contain a multi-line
    descendant).
  - `Levels(n)`: mark every collection with `depth <= n` (top-level = 1).
  - `All`: mark every collection.
  - `AnnotateError::MarkerConflict` if the source already contains `⟦` or `⟧`.
- `pub fn strip(text: &str) -> String` — delete every `⟦...⟧` span
  (U+27E6 .. U+27E7). Lossless: `strip(annotate(x)) == x` for any parseable x.

Marker glyphs are `\u{27E6}` and `\u{27E7}`. Define them as constants.

## 2. CLI

- `cljform tree <file> [--depth <N|all>] [--full] [--json]`
  - default: annotated source on stdout (human mode), depth = heuristic.
  - `--depth N` -> `Levels(N)`; `--depth all` or `--full` -> `All`.
  - `--json`: emit the normal envelope with `op:"tree"`, `file`,
    `file_hash`, and `result: { "nodes": [ ...collect() rows... ] }`.
    Do **not** add a new top-level `Output` field.
  - Marker conflict -> `Fail(1, code "annotate-conflict")`, hint: use `--json`.
- `cljform strip [file]` — read the file, or stdin when omitted; write the
  stripped bytes to stdout **raw** (bypass the JSON/human envelope), exit 0.
  A pure filter. On read error use the normal io error (exit 4).
  Implement by handling `Op::Strip` in `main()` before `print_envelope`, or
  return a sentinel; keep it simple and documented in a comment.

Add both to `Op`, `static_op_name`, and the human/JSON dispatch.

## 3. Tests (`tests/` — new file `handle.rs` is fine)

- `strip_roundtrip_on_fixtures`: for a set of Clojure samples (reuse the
  adversarial/fuzz fixture bytes where practical), run the binary
  `tree --full` then `strip`, and assert the result is byte-identical to the
  input. Include: deeply nested data, BOM-prefixed file, CRLF file, brackets
  inside strings/regex/comments, `#(...)`/`#{...}`.
- `tree_heuristic_marks_top_level_and_multiline_only`: a fixture with a
  single-line nested vector/call and a multi-line nested form; assert the
  single-line ones are unmarked and the multi-line one is marked.
- `tree_depth_1_marks_top_level_only` and `tree_full_marks_all`.
- `strip_removes_markers_only`: `strip` on text containing `⟦abc⟧` removes it;
  plain text is unchanged.
- `annotate_conflict_is_refused`: a file containing a literal `⟦` makes
  `tree` exit 1 with code `annotate-conflict`.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Commands + outcome; new files; the handle algorithm as implemented (one
paragraph); test names; any grammar-kind discovery worth noting.
