# Issue 06 — `format` op (parinfer paren mode, adopted)

Design authority: `SPEC.md` §5/§10; the reference implementation is the local
checkout at `../parinfer-rust` (`src/parinfer.rs`). Adopt its rules; do not add
it as a dependency.

## Deliverable
`cljform format` — an explicit, candidate-first op that reformats indentation
the way parinfer paren mode does. It is the *only* place cljform imposes a
style; the edit path (issue 03) only base-shifts.

## Rules to adopt (from `../parinfer-rust/src/parinfer.rs`)
Read these functions before writing: `correct_indent`, `set_max_indent`,
`on_indent`, `check_indent`, `on_comment_line`, `finish_new_paren_trail`,
`append_paren_trail`, `clean_paren_trail`, `add_indent`, `init_line`,
`is_in_stringish`, and `Paren`/`State`.

For each line, at its first code character (not inside a string, block
comment, regex, or char literal):
- Let `O` = the innermost still-open delimiter.
- `min = O.col + 1` (`0` if no open delimiter).
- `max = O.max_child_col` — the column of `O`'s most recently closed child
  delimiter (`set_max_indent`), or the top-level max when `O` is absent.
- New indent = `clamp(current_indent, min, max)`; leading whitespace is
  rewritten as spaces (tabs normalize by display width).
- **Comment lines** are not clamped (a comment char is not an indentation
  point); they are left alone.
- **Leading closing delimiters** move up onto the previous content line (the
  paren trail), and whitespace between trailing closers is removed so closers
  become contiguous.
- Lines inside strings/comments are untouched.

The reference's `indent_delta` machinery exists for incremental edits; a
from-scratch pass over the whole text can compute the same result directly.
Where the two could differ, the reference wins — see the differential test.

## CLI
- `cljform format [file]` — read the file (or stdin when omitted).
- **Candidate-first**: never writes. Output the formatted candidate, the
  unified diff vs the input, and a note; mirror `materialize`'s result shape
  (`candidate`, `diff`, `note`). A `--write` flag may be added, but default is
  candidate-only.
- Verification before reporting the candidate: it must re-parse clean, and its
  **token stream must equal the input's** (only whitespace and the position of
  closing delimiters may change). A candidate that fails either is an error
  (`format-error`, exit 1), never emitted.

## Differential test (the real gate)
`parinfer-rust` is installed at `~/.cargo/bin/parinfer-rust` (and the source
is at `../parinfer-rust`). Add an integration test that, for a corpus of
fixtures, runs both and asserts byte equality:
- invoke `parinfer-rust --input-format json --output-format text` with
  `{"mode":"paren","text":<input>,"options":{}}` on stdin;
- compare to `cljform format`.
Skip the test (do not fail) when the binary is absent, so the suite stays
hermetic. Corpus: the existing fixtures plus flat, over-indented,
under-indented, nested, standalone-closer, comment-line, string-with-newline,
regex, `#(...)`/`#{...}`, CRLF, and tab cases.

## Tests
- `format_matches_parinfer_rust` (differential, skipped if absent).
- `format_raises_to_open_plus_one`, `format_clamps_to_child_column`,
  `format_preserves_over_indent_below_max`, `format_moves_standalone_closers`,
  `format_leaves_comment_lines`, `format_skips_string_interiors`,
  `format_is_candidate_only`.
- `format_token_stream_unchanged` on every fixture.

## Docs (this issue owns the `format` documentation)
- `README.md`: add `format` to the CLI table (candidate-first; parinfer
  paren-mode rules) and note it is the only op that imposes a style.
- `SPEC.md` §10/§14: record `format` as built, with the adopted rule summary
  and the differential-test gate.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Commands + outcome; which reference functions you mirrored; any input where
your from-scratch pass could not match the reference and how you resolved it;
the differential corpus size.
