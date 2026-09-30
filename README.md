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
| `get <file> [--addr N\|--name sym]` | One form's exact bytes + metadata |
| `check [file]` | Parse + table + nesting warnings (file or stdin) |
| `edit <file> --name s\|--addr N [--mode M] [--content C\|--content-file F]` | Whole-form edit; modes `replace` (default), `insert-after` (`--after`, 0=before first), `insert-before` (`--before`), `append`, `prepend`, `delete` |
| `materialize --content C` | Indent-mode bracket inference → labeled candidate + diff (never writes) |

Exit codes: `0` ok · `1` parse/structure failure (nothing written) · `2`
usage · `3` name not found / ambiguous / stale `--expect` under `--strict` ·
`4` I/O.

Guards: `--expect <hash-prefix>` (12+ hex chars, `blake3:` optional) is
advisory by default — a stale address is re-aimed by hash when the expected
form is still uniquely findable, and the re-aim is reported. `--strict`
turns mismatches and detector warnings into refusals. `--dry-run` validates
and writes nothing.

## Detectors

| ID | Fires on |
|----|----------|
| D1 | `deftest` nested inside executable scope (`defn`, `let`, `fn`, `try`, threading macros, … — hosts matched by base name, so `clojure.test/deftest` is caught too) |
| D2 | `def`/`defn`/… nested inside another def's body (accidental local def) |
| D3 | `ns` not the first form |

Quotes, syntax quotes, regex/char literals, `(comment …)` bodies, `#_`
discards are inert. Reader conditionals are transparent (branches splice into
scope). Detection is an ancestor-stack walk over the tree-sitter parse, run
on a 64 MB-stack worker thread — 50k-deep data is fine.

## The pi extension

`~/.pi/agent/extensions/clojure-forms.ts` registers:

- **clj_forms / clj_edit** — the table and the whole-form editor (content
  via temp file; names survive earlier edits better than addresses).
- **Guard hook** — after *any* built-in `edit`/`write` touching
  `*.clj|cljs|cljc|cljx|edn`, appends a shape report to the tool result:
  form-count delta, lost/gained named forms, D-warnings, or a `BLOCKING:`
  line when the file no longer parses. This is what catches F1 even when the
  agent skips the clj tools.
- **System-prompt note** — injected only when the project contains
  Clojure-ish files.

Subagents: builtin agents (e.g. `worker`) have strict tool allowlists and do
not inherit extension tools, but the guard hook still fires for them. For
form-addressed editing in subagents use the `clojure-worker` agent
(`~/.pi/agent/agents/clojure-worker.md`), whose allowlist includes the clj
tools.

## Building

```
cargo install --path cljform        # binary → ~/.cargo/bin/cljform
cargo test  --path cljform          # 48 tests: cli, golden, repair,
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
