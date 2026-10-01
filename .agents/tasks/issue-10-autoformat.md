# Issue 10 — wrapper reindents edit content (auto-format experiment)

Design authority: `SPEC.md` §10.5 (`format`), §8 (wrapper). The point: the pi
extension reindents the **content being submitted to `clj_edit`** by default
(not the whole file in a separate invocation), then we observe whether that
surprises the agent.

## Deliverable
`clj_edit` in `extension/clojure-forms.ts` reindents its `content` with
`cljform format` before handing it to the CLI, by default. No CLI change and
no whole-file reformat.

## Behavior
- Add an `autoFormat` boolean param to `clj_edit`, default **true**.
- When `content` is present and non-empty and `autoFormat` is on:
  1. run `cljform format` on the content via **stdin** (candidate-only; the
     CLI reads stdin when no file arg is given) — `format` parses the isolated
     form(s) and applies parinfer paren-mode reindentation;
  2. if it succeeds, use the returned `candidate` as the content sent to
     `clj_edit`;
  3. if it fails (parse error, e.g. unbalanced content the edit path would
     repair), keep the original content unchanged and note that it was not
     reindented.
- The Rust edit path then base-shifts the (now parinfer-shaped) content to the
  target column, so the splice lands at the right place with parinfer's
  relative indentation.
- Report it in the tool result: when the content changed, a line such as
  `reindented content (parinfer paren mode) before editing`.
- `autoFormat: false` skips step 1 entirely (content sent verbatim).
- Does not apply to patch mode (`oldText`/`newText` are exact text and must
  not be touched) or to `append`/`prepend` (still content, so it does apply to
  those unless `autoFormat` is false). Do not reindent `oldText`/`newText`.
- If `cljform format` cannot be run (binary missing), fall back to the
  original content and do not fail the edit.

## Docs
- README: note that the pi wrapper reindents `clj_edit` content by default
  (the CLI `edit` base-shifts only; `format` is the explicit whole-file op).
- SPEC §8: record `autoFormat` (default true) and the stdin `format` step.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test` ·
`node --experimental-strip-types --check extension/clojure-forms.ts`

## Report
Commands + outcome; the exact wrapper note text; what happens when the content
is unbalanced (the fallback path); confirmation that `oldText`/`newText` are
never reformatted.
