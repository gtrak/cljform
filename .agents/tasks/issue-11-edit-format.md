# Issue 11 — reindent inside `cljform edit` (drop the wrapper's separate call)

Design authority: `SPEC.md` §10.3/§10.5, §8. Supersedes the wrapper-side
mechanism of issue 10: the reindent must happen inside the edit CLI, not via a
separate `cljform format` invocation.

## Deliverable
1. `cljform edit` reindents the submitted content with the parinfer paren-mode
   algorithm itself, **by default**, with `--no-format-content` to disable.
2. The pi extension stops calling `cljform format`; it just passes
   `--no-format-content` when `autoFormat: false`, and drops the now-dead
   `formatContentStdin`/`child_process` path.

## 1. Rust: reindent in the edit pipeline
In `run_edit`, for **content** modes (replace / insert-before / insert-after /
append / prepend) only — never patch (`oldText`/`newText` are exact) or delete:
- After `content::prepare` (normalize + bracket repair), if formatting is on
  and the prepared content is non-empty:
  - run `format::format_paren(&prepared_text)`;
  - accept the result only if it still parses and
    `format::token_stream(&candidate) == format::token_stream(&prepared)` (the
    same gate `format` uses — whitespace and closer positions only);
  - if it fails or would change the token stream, keep the prepared content
    and add a note (do not fail the edit).
- Then apply the existing base-shift (`reindent_to_column` / `reindent_block`)
  to the formatted content, unchanged.
- Result: `note: "reindented content (parinfer paren mode)"` when the bytes
  changed; the existing base-shift note still applies.

CLI: `Op::Edit` gains a bool, default **true**, disabled by
`--no-format-content` (use clap's negation pattern; `--format-content` may
also be accepted). Keep `--strict`/`--repair`/`--dry-run` as they are.

## 2. Wrapper: drop the separate format call
- Delete `formatContentStdin` and the `node:child_process` import (and any
  now-dead helper); `clj_edit` no longer runs `cljform format`.
- `autoFormat` param stays (default true): when `false`, pass
  `--no-format-content`; when `true`, pass nothing (the CLI default).
- Remove the "reindented content (parinfer paren mode) before editing"
  wrapper note — the CLI now reports it.

## 3. Docs
- README: the reindent is part of `cljform edit` (default on,
  `--no-format-content`); the wrapper no longer makes a separate call.
- SPEC §10.3/§10.5: record the in-edit reindent; §8: `autoFormat` maps to
  `--no-format-content`.

## Tests
- `edit_format_content_reindents`: submit flat balanced content to a nested
  form; assert the result is parinfer-indented + base-shifted, and the note is
  present. Same with `--no-format-content`: assert the relative shape is
  preserved verbatim (base-shift only).
- `edit_format_content_skips_unbalanced`: content the repair fixes; assert no
  `format-error` and the edit still succeeds.
- Keep the whole suite green: if a default-on reindent changes an existing
  test's expected bytes, report exactly which test and why (do not silently
  weaken an assertion); reindentation-only changes may update the expected
  bytes.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test` ·
`node --experimental-strip-types --check extension/clojure-forms.ts`

## Report
Commands + outcome; the clap shape of the flag; which existing tests changed
(if any) and why; confirmation that patch/delete content is never reformatted.
