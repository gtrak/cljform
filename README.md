# cljform

**Form-addressed Clojure editing.** A Rust CLI (`cljform`) plus a pi extension
(`clojure-forms`) that let an agent edit Clojure/EDN files at the
s-expression level — with bracket repair, machine-verified shape, and no way
to leave a file unparsable.

## Philosophy

1. **Do what the caller means.** Target forms by name or address; content may
   be unbalanced, fenced in markdown, or padded with blank lines — it is
   normalized, and brackets are repaired from indentation when the fix is
   unambiguous (reported, never silent).
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
| `get <file> [--addr N\|--name sym]` | One form's exact bytes + metadata. Names are extracted for any `def`-prefixed head, so project macros (`defapifn`, `defstate`, …) are name-targetable |
| `check [file]` | Parse + table + nesting warnings (file or stdin) |
| `edit <file> --name s\|--addr N [--mode M] [--content C\|--content-file F] [--repair]` | Whole-form edit; modes `replace` (default), `insert-after` (`--after`, 0=before first), `insert-before` (`--before`), `append`, `prepend`, `delete`. `--repair` allows a guessed mid-file dedent closure |
| `edit <file> --name s\|--addr N --mode patch --old-text T [--new-text U]` | Surgical text patch inside one form: `T` must occur exactly once in the form's byte range and never cross its boundary; `U` (default empty) replaces it. Bytes outside the match are untouched; no bracket repair, but I1–I3 still gate the write |
| `materialize --content C` | Indent-mode bracket completion → labeled candidate + diff (never writes). Completes missing **closers** implied by indentation; it does not invent missing openers, so a fully bracket-less draft comes back unchanged with a note |

Exit codes: `0` ok · `1` parse/structure failure (nothing written) · `2`
usage · `3` targeting failure or refused repair (not found / ambiguous /
stale `--expect` under `--strict` / `repair-refused` / `dedent-repair`) ·
`4` I/O.

Guards: `--expect <hash-prefix>` (12+ hex chars, `blake3:` optional) is
advisory by default — a stale address is re-aimed by hash when the expected
form is still uniquely findable, and the re-aim is reported. `--strict`
turns mismatches, detector warnings, **and content repairs** into refusals
(exit 3, `repair-refused`, with the diff it declined to apply). `--dry-run`
validates and writes nothing.

Refusal beats guessed repair. By default cljform only *completes* unbalanced
content the forced way — appending missing trailing closers. A repair that
would close an inner form at a mid-file **dedent** is a placement guessed
from indentation, so it is refused with the candidate diff (exit 3,
`dedent-repair`); pass `--repair` to apply it. Two more cases are refused
outright: content from an **unterminated markdown fence** (a likely
truncated paste — exit 1, `truncated-content`), and any repair under
`--strict`.

## Detectors

| ID | Fires on |
|----|----------|
| D1 | `deftest` nested inside executable scope (`defn`, `let`, `fn`, `try`, threading macros, … — hosts matched by base name, so `clojure.test/deftest` is caught too) |
| D2 | any `def…` head (`def`, `defn`, project macros like `defapifn`/`defstate`) nested inside another def's body (accidental local def) |
| D3 | `ns` not the first form |

Quotes, syntax quotes, regex/char literals, `(comment …)` bodies, `#_`
discards are inert. Reader conditionals are transparent (branches splice into
scope). Detection is an ancestor-stack walk over the tree-sitter parse, run
on a 64 MB-stack worker thread — 50k-deep data is fine.

## The pi extension

Lives in this repo at `extension/clojure-forms.ts` (with the
`extension/agents/clojure-worker.md` subagent definition). It registers:

- **clj_forms / clj_get / clj_edit / clj_draft** — the table, single-form byte
  fetch, the editor, and a draft recovery aid. `clj_get` returns one form's
  exact bytes (refreshing the cache); `clj_edit` takes whole-form `content`
  or a surgical `oldText`/`newText` patch (mode auto-selects `patch`), so a
  four-line change in a 60-line form no longer means re-transcribing 60
  lines. `clj_draft` runs the indent-mode completer on an
  indentation-only draft and returns candidate + diff (never writes).
  Content is passed via temp file; names survive earlier edits better than
  addresses.
- **Guard hook** — after *any* built-in `edit`/`write` touching
  `*.clj|cljs|cljc|cljx|edn`, appends a shape report to the tool result:
  form-count delta, lost/gained named forms, D-warnings, or a `BLOCKING:`
  line when the file no longer parses. This is what catches F1 even when the
  agent skips the clj tools.
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
refusals and self-corrections, stale-address recovery via the `was …`
result field, D1 bait compliance/justification, and `BLOCKING:` recovery —
see SPEC.md §14.

## Building

```
cargo install --path .              # binary → ~/.cargo/bin/cljform
cargo test                          # 58 tests: cli, golden, repair,
                                    # adversarial, fuzz, F1 regression
```

Dependencies: `tree-sitter` + `tree-sitter-clojure` (pinned via Cargo.lock —
golden tables are grammar-sensitive), `clap`, `serde`/`serde_json`,
`blake3`, `tempfile`. No network, no Clojure runtime, nothing is ever
evaluated.

## Tests worth knowing about

- `tests/regression_swallowed_deftest.rs` — the F1 bug that motivated the
  tool: a balanced file whose `defn` closer drifted to swallow three
  `deftest`s. Must report the reduced form count **and** fire D1 with exact
  line ranges.
- `tests/adversarial.rs` — deep data, BOM, qualified heads, same-line
  neighbors, CRLF, unicode, bracket look-alikes, quote/comment/discard
  burial, reader conditionals, stale-view traps.
- `tests/fuzz.rs` — 300 deterministic garbage inputs: structured errors,
  never a panic, never a partial write.
- `tests/repair.rs` — repair is asserted on **nesting**, not just paren
  balance. Indent mode once compared an absolute byte column against a
  per-line indent, closing an inner form a line early (`(let [y 2])` with
  the body escaping it); balance-only assertions passed the wrong output.
  Now the repaired form text is compared exactly; an unterminated
  markdown fence around unbalanced content is refused, not repaired; and a
  mid-file **dedent** closure (the last guessed placement) is refused unless
  `--repair` opts in.
