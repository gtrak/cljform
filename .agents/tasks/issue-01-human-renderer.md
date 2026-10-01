# Issue 01 — `--human` renderer

## Problem

`cljform --human <op>` is op-agnostic: `print_human` prints the form table
whenever `out.forms` is present, then `result.text` if present. Two ops put
their payload somewhere else, so human output is wrong:

1. `get` — the result carries `result.form` (the requested form's exact bytes)
   and `out.forms` (the whole table). Human mode prints the table and never the
   form.
2. `materialize` — the result carries `candidate` / `diff` / `note`, none of
   which `print_human` reads, so human mode prints only the note.
3. `edit` — `print!("{text}")` emits `result.text` with no trailing newline, so
   the following `note:` line runs into it.

## Read first
- `src/main.rs` — `print_human` (~line 241), `human_summary` (~1329),
  `forms_output` (~1385), the `Get` and `Materialize` arms of `dispatch`.
- `tests/cli.rs` — `human_mode_goes_to_stdout_stderr_without_json`.
- `SPEC.md` §4.3/§4.4 for the `get` and `materialize` result shapes.

## What to build

Make `print_human` op-aware. After the error branch:

- `out.op == "get"` — print a one-line header
  (`<file> · <kind> [name] · lines a–b · blake3:<hash>`) then `result.form`
  bytes. Do **not** print the form table.
- `out.op == "materialize"` — print `result.candidate`, then `result.diff`
  when it is non-empty.
- every other op — unchanged: table when `out.forms` is present, then
  `result.text` if present.
- Ensure a newline separates `result.text` from the warnings/notes that follow
  (print a trailing newline when the text does not already end in one).
- Warnings and notes still print for every op.

Do not change JSON output, result shapes, or any non-human path.

## Tests (add to `tests/cli.rs`)
- `human_get_shows_form_bytes`: run `get <file> --name <defn> --human`; assert
  stdout contains the form's exact code (e.g. the body expression) and does not
  merely repeat the table.
- `human_materialize_shows_candidate`: run `materialize --content <unbalanced
  draft> --human`; assert stdout contains the bracketed candidate and a diff
  marker (`@@` or `+`/`-`).
- Keep `human_mode_goes_to_stdout_stderr_without_json` green.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Commands run + outcome; files changed; the two new test names; anything
ambiguous you resolved. Keep it short.
