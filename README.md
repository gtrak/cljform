# cljform

This repo is mostly bot-written.

**Form-addressed Clojure editing.** `cljform` is a Rust CLI (plus a pi
extension) that lets an agent — or you — edit Clojure/EDN files at the
s-expression level: forms are addressed by content-hashed handles, every
write is verified against the parse tree, and there is no way to leave a
file unparsable.

It exists because text-oriented edit tools fail on s-expressions in a
specific way: a misplaced paren still *parses* (as the wrong structure),
and a "balanced" edit can silently swallow the forms after it. cljform
makes those failures either impossible or loud.

## Why build this? (human-written)

The way I prefer to work is with LLM assistance, queueing up a batch of tasks
with a smarter planner model, then walking away and thinking about something else, 
and reviewing the output.  I use local models (Qwen 3.8 27b as of this writing) 
for the bulk of my code generation. They have gotten good enough that line edits
mostly work, but clojure is consistently a painful experience. I might walk away
and come back to bespoke python scripts being made to count unbalanced parens and
just endless looping and context bloat from failed edits. Parinfer-rust got me
closer to a solution, but this is a more purpose-built tool for agents' expectations,
built by constant dogfooding between GLM 5.3 flash and my local model, with me nudging the design.
The original approach came from this article: https://lispmeister.github.io/deeprecursion/posts/2026-02-13-sexp-native-editing.html .
I started with tree-sitter and the LLM's own retrospective of a session with a
lot of failed edits.  I care less about conceptual cleanliness than I do about having a
useful tool with better working tradeoffs, so the implementation details are still
unstable.

## A thirty-second tour

```console
$ cljform tree todos.clj
(⟦a68821⟧ns demo.todos)

(⟦bd7b68⟧def todos (atom []))

(⟦043321⟧defn add-todo [text]
  (swap! todos conj {:text text :done false}))

(⟦a2fbf8⟧defn complete-all []
  (swap! todos (fn [t] (map #(assoc % :done true) t))))
```

Notice which forms got handles: every top-level form, plus nested forms
**large enough to span multiple lines** (here, none). That is the default
view's readability heuristic — a Clojure file is dense with small
collections (`(assoc …)`, `[x y]`, `{:k v}`), and marking each one would
bury the source in `⟦handle⟧` glyphs. The line the heuristic draws is also
the workflow line: a **multi-line nested form** is worth editing as a
whole unit, so it gets a handle; a **single-line inner expression** is a
patch target instead — you change it through its *parent's* handle, which
needs no deep handle at all. When a nested form *is* multi-line, it gets
its own handle — and its multi-line children get theirs, down the tree:

```console
$ cljform tree report.clj
(⟦da51a6⟧defn render-report [rows]
  (⟦71a7de⟧let [⟦a85514⟧total (reduce + 0 (map :amount rows))
        lines (⟦f3a214⟧for [r rows]
                (format "%-20s %8.2f\n" (:name r) (:amount r)))]
    (⟦83c8ea⟧str "TOTAL: " total "\n"
         (apply str lines))))
```

The `let`, the `for`, and the `str` span lines, so they carry handles;
their single-line children (`(reduce …)`, `(format …)`, the binding
vectors) do not. When you want every collection regardless:
`--full` marks all, `--name SYM` renders a matched form's entire subtree,
and `--json` always lists a handle for every node at every depth.

Every handle is a short content hash, and the **only** edit target. Edit
by handle:

```console
$ cljform edit todos.clj --handle a2fbf8 --mode patch \
    --old-text '(map #(assoc % :done true) t)' \
    --new-text '(map (fn [todo] (assoc todo :done true :completed-at (now))) todo)'
todos.clj: 4 forms
patched form ⟦b36464⟧ complete-all (lines 8–9) — 1 changed, 3 untouched
--- before / +++ after …
```

Mistype the target text and the refusal hands you the form's exact bytes to
copy from — recovery is one retry, not a re-read:

```console
$ cljform edit todos.clj --handle 043321 --mode patch --old-text '…wrong…'
error [patch-not-found]: --old-text not found inside list_lit (lines 5–6) …
exact form bytes (copy oldText from these): …
```

An unchanged form keeps its handle across edits elsewhere; a changed one
rotates its handle and the edit summary says so — a stale handle can
re-aim, never mis-aim.

## The model

- **Handles are content-addressed.** `tree` marks each form with
  `⟦handle⟧`; that handle is the only address `edit` accepts. Resolution
  either matches the one form it names or refuses (`stale-handle`,
  `ambiguous-handle`) — a stale view can never target the wrong form.
- **Writes are verified.** A write happens only if the post-splice file
  parses clean and every untouched form is byte-identical to before.
  Otherwise nothing is written and the error carries the exact line/col.
- **Inference is opt-in.** Unbalanced content is refused by default with
  the inferred candidate attached; `--repair` applies the inference from
  indentation (reported, never silent). Submitted content is reindented in
  parinfer paren mode and base-shifted to the target's column by default.
- **Drift is reported, not hidden.** A balanced-but-wrong structure (the
  original motivating bug: a drifted closer swallowing three `deftest`s)
  parses fine, so detectors D1/D2 flag it with exact line ranges;
  `--strict` turns warnings into refusals for CI.

## CLI

| Op | Purpose |
|----|---------|
| `tree <file>` | The annotated view — source with `⟦handle⟧` per form. The primary discovery surface. `--name SYM` selects a nested form by name; `--start-line/--end-line` page large files (complete forms only); `--json` emits the node table; `--recover` is the broken-file view (below) |
| `forms <file>` | Flat table: addr, kind, name, lines, blake3, shape summary, warnings |
| `get <file> --name SYM \| --handle H` | One form's exact bytes + metadata (pass its handle to `edit`) |
| `edit <file> --handle H` | Whole-form edit: `--mode replace\|patch\|insert-after\|insert-before\|append\|prepend\|delete`, with `--content`/`--content-file` or patch `--old-text/--new-text`. Returns a unified diff of the changed region. See below for repair/strict |
| `check <file>` | Parse + form table + detector warnings (file or stdin) |
| `materialize --content C` | Indent-mode bracket completion → candidate + diff, never writes |
| `format <file>` | Reindent like parinfer paren mode → candidate + diff, never writes |
| `strip` | Remove `⟦…⟧` markers → exact original bytes (pure filter, file or stdin) |

`--json` works on every envelope op; `--human` (or a TTY) for the readable
view. `--dry-run` validates and writes nothing. Full contract: SPEC §4.

Exit codes: `0` ok · `1` parse failure / broken-file gate / `internal-error`
· `2` usage · `3` targeting or refused content · `4` I/O. Nothing is ever
written on a nonzero exit.

## When files break: conflicts and recovery

Merge conflicts are where agents are most tempted to fall back to raw text
edits — and where a subtlety bites: conflict markers are *legal Clojure
symbols*, so a conflicted file still parses, and a naive tool will happily
edit one side and call it fixed.

cljform treats a conflicted (or otherwise unparsable) file as a first-class
**broken state**:

```console
$ cljform check conf.clj
error [conflict-markers]: 1 conflict region(s) (lines 3-7)
hint: resolve the conflict(s) with a text edit; cljform resumes when the file parses
```

Every op refuses a broken file — an edit cannot land on one side of a
conflict and call it resolved. `tree --recover` shows the way out: verbatim
source, each conflict region with per-side line spans (HEAD / base /
incoming), parse-error spans, and the intact forms around them — no handles
yet, because none are usable while the file is broken:

```console
$ cljform tree conf.clj --recover
file does not parse — 1 conflict region(s), 0 parse error(s);
handles appear when the file is repaired
source (lines 1–7): …
conflict region lines 3–7 (head 3–4 · incoming 5–7)
```

Resolve with a text edit, `check` goes green, and the normal handle flow
resumes. The pi extension's guard hook reports the same diagnostics after
every built-in edit, with the remaining region count decreasing as you work
through a multi-conflict file.

## Detectors

| ID | Fires on |
|----|----------|
| D1 | `deftest` nested inside executable scope (`defn`, `let`, `fn`, threading macros, …) |
| D2 | any `def…` head nested inside another def's body (accidental local def) |
| D3 | `ns` not the first form |

Quotes, syntax quotes, regex/char literals, `(comment …)` bodies and `#_`
discards are inert; reader conditionals are transparent. Detection runs on a
64 MB-stack worker thread — 50k-deep data is fine.

## The pi extension

`extension/clojure-forms.ts` registers five tools — `clj_tree`,
`clj_get`, `clj_edit`, `clj_forms`, `clj_draft` — that wrap the CLI with
agent-shaped ergonomics: discovery, exact-byte fetch, handle-targeted
editing (a four-line change in a 60-line form no longer means
re-transcribing 60 lines), and the guard hook on built-in edits. See
SPEC §8 for the full tool contract.

Install (the symlinks make the extension live-track this repo):

```sh
cargo install --path .
ln -s "$PWD/extension/clojure-forms.ts" ~/.pi/agent/extensions/clojure-forms.ts
ln -s "$PWD/extension/agents/clojure-worker.md"  ~/.pi/agent/agents/clojure-worker.md
```

## Development

```sh
cargo build && cargo test        # 194 tests across 11 suites
```

Standard gates: `cargo build` · `cargo clippy --all-targets -- -D warnings
-W clippy::unwrap_used -W clippy::expect_used -W clippy::panic
-W clippy::unreachable -D clippy::todo` · `cargo test`. Panicking
constructs are lint-denied with auditable allowances; release builds run
with `overflow-checks = true`; a residual panic is caught at the dispatch
boundary and becomes an `internal-error` envelope — never a crash, never a
partial write. No network, no Clojure runtime, nothing is ever evaluated.

`SPEC.md` is the complete contract: envelope shapes, the addressing model,
safety invariants (I1–I6), the recovery view, the design-decision log, and
the test map.
