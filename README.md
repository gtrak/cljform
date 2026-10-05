# cljform

**Form-addressed Clojure editing.** A Rust CLI (`cljform`) plus a pi extension
(`clojure-forms`) that let an agent edit Clojure/EDN files at the
s-expression level — with bracket repair, machine-verified shape, and no way
to leave a file unparsable.

## Philosophy

1. **Do what the caller means.** Target forms by `⟦handle⟧` (copied from
   `tree`; names are read lookups); content may be fenced in markdown or
   padded with blank lines — it is normalized. Unbalanced brackets are
   refused by default with the inferred candidate attached; `--repair`
   infers them from indentation (reported, never silent).
2. **Never leave the file broken.** A write happens only if the post-splice
   file parses clean and every untouched form is byte-identical to before.
   Otherwise: nothing is written and the error carries the exact line/col.
3. **Verification over blocking.** Structural drift (a `deftest` swallowed by
   a drifted `defn` closer — the F1 bug) parses fine and is *reported* as a
   D1 warning with exact line ranges; `--strict` flips warnings to hard
   failures for CI/paranoid flows.

## CLI

```
cljform [--json|--human] <op> …
```

| Op | Purpose |
|----|---------|
| `forms <file>` | Top-level form table: addr, kind, name, lines, blake3, `contains` shape summary, D-warnings |
| `tree <file> [--depth N\|all \| --full] [--json] [--name SYM] [--start-line N \| --end-line N] [--recover]` | Annotated view: the source with `⟦handle⟧` after each marked collection's opening delimiter (default: top-level + multi-line forms). Handles are the only edit targets; every emitted handle resolves. `--json` emits the flat node table. Refuses sources that already contain the marker glyphs (`annotate-conflict`, exit 1). `--name SYM` selects only the forms that define `SYM` (exact def-like name, any nesting depth): a count header, then each matched source block at full depth with its handles inline, sorted by line; zero matches is an ok empty result. `--start-line/--end-line` page to a line window (complete forms only, real file line numbers, the effective span echoed). `--recover` renders the broken-file recovery view: verbatim source + per-side conflict spans + parse-error spans + the intact top-level forms, NO handles (SPEC §10.2) — on a healthy file it is the normal view |
| `strip [file]` | Delete every `⟦…⟧` marker span → recovers the exact original bytes (BOM/CRLF preserved). Pure filter on stdout, no envelope; file or stdin |
| `get <file> [--name sym\|--handle H]` | One form's exact bytes + metadata, including its `⟦handle⟧` (pass it to `edit`). `--name` is a top-level read lookup — names are extracted for var-defining `def…` heads (`defn`, `defapifn`, …; not `defmethod`, which extends an existing multimethod); `--handle` reads any collection node |
| `check [file]` | Parse + table + nesting warnings (file or stdin) |
| `edit <file> --handle H [--mode M] [--content C\|--content-file F] [--repair] [--no-format-content]` | Whole-form edit; modes `replace` (default), `insert-after` / `insert-before` (anchor: `--handle`), `append`, `prepend` (file ends, no target), `delete`. Submitted content is reindented in parinfer paren mode (default on; `--no-format-content` sends it verbatim, and patch/delete never reformat) and then base-shifted to the target's column. `--repair` enables bracket inference (missing trailing closers + a guessed mid-file dedent closure); without it, unbalanced content is refused (`unbalanced-content`, exit 3, candidate + diff attached). Inference runs on the content as submitted — its decision/outcome is identical with or without the reindent and at any target column |
| `edit <file> --handle H --mode patch --old-text T [--new-text U]` | Surgical text patch inside one form: `T` must occur exactly once in the form's byte range and never cross its boundary; `U` (default empty) replaces it. Bytes outside the match are untouched; no bracket repair, but I1–I3 still gate the write. A mismatch error returns the form's **exact bytes**, so recovery needs no `clj_get` round-trip |
| `materialize --content C` | Indent-mode bracket completion → labeled candidate + diff (never writes). Completes missing **closers** implied by indentation; it does not invent missing openers, so a fully bracket-less draft comes back unchanged with a note |
| `format [file]` | Reformat indentation the way parinfer paren mode does — the only op that imposes a style on a whole file (the edit path reindents its submitted content with the same pass, by default, then base-shifts it). Candidate-first: candidate + unified diff vs the input + note, never writes. The candidate must re-parse clean and keep the input's token stream (only whitespace and closing-delimiter position may move); otherwise `format-error`, exit 1, nothing emitted |

Exit codes: `0` ok · `1` parse/structure failure or `annotate-conflict`
(nothing written) — including `conflict-markers` (git conflict markers in
the file: the gate that refuses every op on a conflicted file, whose
markers otherwise parse as legal symbols) and `not-supported-broken`
(`tree --name` on a broken file) — or `internal-error` (a residual panic
caught at the dispatch boundary — a tool bug; nothing was written) ·
`2` usage · `3` targeting failure or refused content /
repair (`form-not-found` / `ambiguous` / `stale-handle` / `ambiguous-handle`
/ `unbalanced-content` / `repair-refused`) · `4` I/O.

Guards: the `⟦handle⟧` is a content pin — resolution either matches the one
form it names or refuses (`stale-handle` / `ambiguous-handle`, exit 3,
"re-run `tree`"), so a stale view can never target the wrong form. Handles
are content-addressed: an unchanged form keeps its handle across edits
elsewhere. `--strict` turns detector warnings into a refusal (exit 1,
`detector-fatal`) and, together with `--repair`, **content repairs** into a
refusal (exit 3, `repair-refused`, with the diff it declined to apply —
`--strict` wins).
`--dry-run` validates and writes nothing.

Refusal beats inferred repair. Unbalanced content is refused by default
(exit 3, `unbalanced-content`, nothing written) with the inferred candidate
and its diff attached; pass `--repair` to apply it. The inference decision
runs on the content as submitted — it does not depend on the base-shift
dedent, the parinfer reindent, or the target column. Two more cases are
refused outright: content from an **unterminated markdown fence** (a likely
truncated paste — exit 1, `truncated-content`), and any repair under
`--strict` together with `--repair` (exit 3, `repair-refused`).

## Detectors

| ID | Fires on |
|----|----------|
| D1 | `deftest` nested inside executable scope (`defn`, `let`, `fn`, `try`, threading macros, … — hosts matched by base name, so `clojure.test/deftest` is caught too) |
| D2 | any definition-like `def…` head (`def`, `defn`, `defmethod`, project macros like `defapifn`/`defstate`) nested inside another def's body (accidental local def) |
| D3 | `ns` not the first form |

Quotes, syntax quotes, regex/char literals, `(comment …)` bodies, `#_`
discards are inert. Reader conditionals are transparent (branches splice into
scope). Detection is an ancestor-stack walk over the tree-sitter parse, run
on a 64 MB-stack worker thread — 50k-deep data is fine.

## The pi extension

Lives in this repo at `extension/clojure-forms.ts` (with the
`extension/agents/clojure-worker.md` subagent definition). It registers:

- **clj_forms / clj_tree / clj_get / clj_edit / clj_draft** — the top-level
  table, the annotated `⟦handle⟧` view, single-form byte fetch, the editor,
  and a draft recovery aid. `clj_tree` is the primary way an agent discovers
  handles (the only edit targets); `clj_get` returns one form's exact bytes
  plus its handle (`--name` or `--handle`); `clj_edit` targets by
  `⟦handle⟧` and takes whole-form `content` or a surgical
  `oldText`/`newText` patch (mode auto-selects `patch`), reindenting
  submitted content to the target's column — a four-line change in a 60-line
  form no longer means re-transcribing 60 lines. `cljform edit` itself also
  reindents the `content` being submitted in parinfer paren mode (default on,
  `--no-format-content` to disable; `autoFormat: false` maps to that flag —
  the wrapper no longer makes a separate `cljform format` call, and content
  the reindent refuses is sent verbatim with a note; patch
  `oldText`/`newText` are never touched). `clj_draft` runs the
  indent-mode completer on an indentation-only draft and returns candidate +
  diff (never writes). Content is passed via temp file; handles survive
  edits elsewhere, so a copied handle keeps working until its own form
  changes (`stale-handle` → re-run `clj_tree`).
- **Guard hook** — after *any* built-in `edit`/`write` touching
  `*.clj|cljs|cljc|cljx|edn`, appends a shape report to the tool result:
  form-count delta, lost/gained named forms, D-warnings, or a `BLOCKING:`
  line when the file no longer parses — plus, on a broken file, one line
  per diagnostic (conflict regions with per-side line spans, parse-error
  spans), so each edit restates what is still broken. This is what catches
  F1 even when the agent skips the clj tools, and a conflicted file even
  though its markers parse as legal symbols (`conflict-markers`).
- **System-prompt note** — injected only when the project contains
  Clojure-ish files.

### Install

```sh
cargo install --path .
ln -s "$PWD/extension/clojure-forms.ts" ~/.pi/agent/extensions/clojure-forms.ts
ln -s "$PWD/extension/agents/clojure-worker.md"   ~/.pi/agent/agents/clojure-worker.md
```

The symlink keeps the installed extension live-tracking this repo — edits
here apply on the next pi session (or `/reload`).

Subagents: builtin agents (e.g. `worker`) have strict tool allowlists and do
not inherit extension tools, but the guard hook still fires for them. For
form-addressed editing in subagents use the `clojure-worker` agent, whose
allowlist includes the clj tools. Verified against the builtin `worker` and
`clojure-worker` with adversarial drills on a local model: bracket-mismatch
refusals and self-corrections, stale-handle refusal → re-`tree` recovery, D1
bait compliance/justification, and `BLOCKING:` recovery — see SPEC.md §14.

## Building

```
cargo install --path .              # binary → ~/.cargo/bin/cljform
cargo test                          # cli, forms, repair,
                                    # robustness, F1 regression
```

Standard gates (run before reporting any change): `cargo build` ·
`cargo clippy --all-targets -- -D warnings -W clippy::unwrap_used
-W clippy::expect_used -W clippy::panic -W clippy::unreachable -D
clippy::todo` · `cargo test`. Release builds run with `overflow-checks = true`; the lint gate denies panicking constructs
(`unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`); every remaining site
is an in-source `#[allow]` carrying a one-line written justification
(auditable assumptions, issue 30), and a residual panic is converted at
the dispatch boundary into the `internal-error` envelope (SPEC §4.1).

Dependencies: `tree-sitter` + `tree-sitter-clojure` (pinned via Cargo.lock —
golden tables are grammar-sensitive), `clap`, `serde`/`serde_json`,
`blake3`, `tempfile`, `unicode-width`, `unicode-segmentation` (the last two
back `format`'s display-width / grapheme handling; accepted in SPEC §11).
No network, no Clojure runtime, nothing is ever evaluated.

## Tests worth knowing about

- `tests/regression.rs` — the F1 bug that motivated the tool: a balanced
  file whose `defn` closer drifted to swallow three `deftest`s (`
  swallowed_deftests_are_detected_with_exact_lines`). Must report the
  reduced form count **and** fire D1 with exact line ranges.
- `tests/robustness.rs` — adversarial + fuzz material, consolidated: deep
  data, BOM, qualified heads, same-line neighbors, CRLF, unicode, bracket
  look-alikes, quote/comment/discard burial, reader conditionals, stale-view
  traps, plus deterministic garbage inputs: structured errors, never a
  panic, never a partial write.
- `tests/repair.rs` — repair is asserted on **nesting**, not just paren
  balance. Indent mode once compared an absolute byte column against a
  per-line indent, closing an inner form a line early (`(let [y 2])` with
  the body escaping it); balance-only assertions passed the wrong output.
  Now the repaired form text is compared exactly; an unterminated
  markdown fence around unbalanced content is refused, not repaired;
  inference is opt-in (`--repair` — the default refuses with the
  `unbalanced-content` candidate); `--repair --strict` refuses
  (`repair-refused`); and the inference outcome is asserted byte-identical
  with or without `--format-content` and at two different target columns.
