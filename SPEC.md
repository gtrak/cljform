# cljform — form-addressed Clojure editing: spec

Status: **implemented (v2: handles, tree/strip, handle-only edit, patch mode, bounded repair, strict, clj_draft, format)** · Created: 2026-09-29
Shape: standalone Rust CLI (`cljform`) + pi extension wrapper (`clojure-forms`)

## 1. Problem

pi's built-in `edit`/`write` tools operate on **raw text**. Clojure correctness
lives at the **s-expression** level. The gap between those two levels is where
edits go wrong, and this is not hypothetical — observed failure modes from
real agent-driven Clojure work:

| # | Failure | What happened | Caught by |
|---|---------|---------------|-----------|
| F1 | Swallowed form | A `defn` was left unclosed, so the next three `deftest`s parsed *inside* it. File was fully balanced. | **Nothing** — parinfer passed, linter passed, 3 tests silently unregistered; caught only by noticing 3 instead of 6 tests ran |
| F2 | Mis-targeted edit | A read-range overlap made the reviewer misread a duplicated line that didn't exist | manual re-check |
| F3 | Ad-hoc tooling churn | broken REPL one-liners, pathspec errors from wrong cwd — ~20 min of rediscovery | — |
| F4 | Bracket-counting under pressure | Any multi-hundred-line replacement requires hand-counting parens; the standard recovery (indent mode) is a manual scratch-file dance | parinfer, eventually |

F1 is the critical one: **all existing structural checks are well-formedness
checks**. A file can be perfectly well-formed and still be the wrong shape.
The only checks that catch F1 are (a) a top-level-form fingerprint diff before/
after the edit, and (b) "is this `deftest` supposed to be inside that `defn`?"
nesting sanity. Neither exists as a first-class tool today.

## 2. Goals / non-goals

**Goals**
- Edits expressed as **whole-form operations** with machine-checked
  well-formedness *and* shape invariants, applied atomically, with precise
  line/col diagnostics on failure.
- **Content-addressed handle resolution** so a stale view of the file (its
  target form edited or deleted, bytes changed by a formatter) can never
  target the wrong form: a handle pins position + content, and resolution
  either matches the one form it names or refuses (`stale-handle` /
  `ambiguous-handle`, exit 3) (§5, §10.1).
- **Never evaluate, never load**: the tool is purely syntactic. No Clojure
  runtime, no macro expansion, no namespace loading, ever.
- Deterministic, scriptable, fast: JSON I/O, stable exit codes, ms-scale.
- A pi wrapper that gives the agent the operations as tools and *also* guards
  the plain `edit`/`write` tools, so both paths are safe.

**Non-goals (v1)**
- Not a formatter: cljform splices bytes and preserves every byte outside the
  target form. A formatter (cljfmt et al.) remains the style gate (run it
  after, separately).
- Not a semantic analyzer: no type checking, no shadowing, no linting (that's
  kondo et al.). cljform catches *structural* drift.
- No nested (path) addressing in v1 — top-level forms only. (Shipped as the
  v2 handle design — §10, §14.)
- Not an editor for humans — it's an agent/CLI tool with a human-readable
  fallback.

## 3. Architecture

```
┌─────────────────────── pi (TS) ───────────────────────────────┐
│ extension: clojure-forms (global, ~/.pi/agent/extensions/)    │
│                                                               │
│ tools:   clj_forms · clj_tree · clj_get · clj_edit · clj_draft│
│ guard:   tool_result hook on built-in edit/write for          │
│          *.clj *.cljs *.cljc *.cljx *.edn  → cljform check    │
│ state:   per-file form-fingerprint cache (in-memory Map;      │
│          guard-hook shape deltas only)                        │
└───────────────────────────────────────────────────────────────┘
                │ spawn: cljform <op> --json …  (content via temp file)
┌───────────────▼───────────────────────────────────────────────┐
│ cljform (Rust, no network, no Clojure runtime)                │
│                                                               │
│ parse:  tree-sitter + tree-sitter-clojure grammar             │
│ view:   top-level form table (addr, kind, name, line range,   │
│         blake3 hash, contained-kind counts) + annotated tree  │
│         with content-addressed ⟦handles⟧                      │
│ ops:    forms · tree · strip · get · check · edit ·           │
│         materialize · format                                  │
│ safety: invariants I1–I6, atomic write, nesting detectors     │
└───────────────────────────────────────────────────────────────┘
```

The binary is self-contained: given a file path (or stdin) and an operation,
it produces a JSON result and an exit code. All intelligence about *what to
do next* lives in the wrapper/agent; all intelligence about *is this
structure-safe* lives in the binary.

## 4. CLI contract

### 4.1 Global

```
cljform [--json|--human] <op> [args]
```

- `--json` (default when stdout is not a TTY, always on for the wrapper): one
  JSON object on stdout. `--human`: plain text with line/col.
- File argument: `check`, `strip`, and `format` take a path or read stdin when
  the path is omitted; `forms`, `tree`, `get`, and `edit` require a path.
  `edit` content arrives via `--content`, `--content-file`, or stdin.
- **Exit codes** (the wrapper branches on these):
  - `0` — success (including a clean `--dry-run`)
  - `1` — parse/structure error: `parse-error`, `not-one-form`,
    `truncated-content`, `shape-violation`, `detector-fatal` (under
    `--strict`), `annotate-conflict` (`tree` view), `materialize-error`,
    `format-error`
  - `2` — usage error: `usage` (bad args, unknown op, target required but
    missing, `--handle` shorter than 6 hex chars, `--handle` with
    append/prepend)
  - `3` — targeting/refusal: `form-not-found`, `ambiguous`, `stale-handle`,
    `ambiguous-handle`, `patch-not-found`, `patch-ambiguous`,
    `repair-refused` (under `--strict`), `dedent-repair`
  - `4` — I/O error: `io` (unreadable file, write failure)
- **JSON envelope** (success):
  ```json
  { "ok": true, "op": "edit", "file": "/abs/path",
    "file_hash": "blake3:…", "forms": [ … ],
    "result": { … }, "warnings": [ … ], "notes": [ … ] }
  ```
- **JSON envelope** (error):
  ```json
  { "ok": false, "op": "edit",
    "error": { "code": "stale-handle" | "form-not-found" | "usage" | "io" | …,
               "line": 46, "col": 1, "message": "…",
               "hint": "…" } }
  ```
  Lookup and patch-mismatch errors carry a recovery payload —
  `suggestions` (did-you-mean candidates) on `--name` lookups, and the
  target form's exact bytes on `patch-not-found` — so the agent can re-aim
  without a second round-trip; every other error re-aims via `tree` or
  `forms`.

### 4.2 Form table (the `forms` array)

```json
{ "addr": 7, "kind": "defn", "name": "handle-thing",
  "line": [165, 196], "hash": "blake3:…",
  "contains": { "deftest": 0, "defn": 0, "let": 2, "if": 1 } }
```

The `forms` array is the **top-level listing**. The **nested view** is the
`tree --json` node table (§10.2): `path`, `kind`, `head`, `name`, `line`,
`depth`, `handle` per collection node.

- `addr` — 1-based top-level form index.
- `kind` — head symbol of the form (`defn`, `defn-`, `def`, `deftest`, `ns`,
  `in-ns`, `do`, `fn`, other, …); for non-symbol heads: `expr`.
- `name` — for named forms (`defn`/`def`/`deftest`/`defprotocol`…), the var
  name; else null.
- `line` — [first, last] 1-based inclusive.
- `hash` — blake3 of the **exact bytes** of the form (no normalization: a
  formatter changes bytes and must invalidate the hash — that's a feature, §6).
- `contains` — head-symbol counts of the form's **direct child list forms**
  (top body level), keyed by base (unqualified) head name. This is the cheap
  shape summary the agent eyeballs ("13 forms: 1 ns, 2 defn-, 10 deftest" —
  expected 11 deftests, so something is wrong).

### 4.3 Operations

| Op | Purpose | Mutates | Key args |
|----|---------|---------|----------|
| `forms <file>` | top-level form table (§4.2) | no | — |
| `tree <file>` | annotated view: source with `⟦handle⟧` after each marked collection's opening delimiter (§10.2) | no | `--depth N\|all`, `--full`, `--json` (flat node table) |
| `strip [file]` | delete every `⟦…⟧` marker → the exact original bytes; pure stdout filter, no envelope (§10.2) | no | file or stdin |
| `get <file>` | one form's exact bytes + metadata, including its handle | no | `--name sym` \| `--handle H` |
| `check [file]` | parse + form table + nesting warnings | no | file or stdin |
| `edit <file>` | whole-form / patch / insert / delete, handle-targeted (§5, §10.3) | yes | `--handle H` (append/prepend excepted), `--mode replace\|patch\|insert-after\|insert-before\|append\|prepend\|delete`, `--content` / `--content-file` / stdin, `--old-text`/`--new-text` (patch), `--strict`, `--repair`, `--dry-run` |
| `materialize` | draft→candidate via indent mode (§4.4) | no | `--content` / `--content-file` / stdin; emits candidate + unified diff |
| `format [file]` | parinfer paren-mode reindent, candidate-first (§10.5) | no | file or stdin |

**Content rule (edit):** content is normalized (markdown fences stripped,
blank edges trimmed) and may carry several top-level forms — the I3 window
generalizes to an N-form allowed change (the v1 "exactly one complete
balanced form" gate is gone; §14). Unbalanced content is repaired by
indent-mode inference when the fix is unambiguous, with a reported diff; the
default only completes missing trailing closers. Refusals: `not-one-form`
(exit 1 — needs a human eye), `truncated-content` (exit 1, an unterminated
opening fence), `repair-refused` (exit 3, any repair under `--strict`), and
`dedent-repair` (exit 3, a mid-file dedent closure — a guessed placement;
`--repair` opts in, candidate + diff attached). `patch` mode is exact-match:
`--old-text` must occur exactly once inside the target form's bytes,
`--new-text` may be empty; there is no repair. `patch-not-found` (exit 3)
returns the form's exact bytes; `patch-ambiguous` (exit 3) names the
occurrence count.

**Handle resolution (replaces the v1 `--expect` generation guard):** every
form-targeting mode resolves `--handle H` content-addressedly (§5, §10.1):
the handle is the shortest unique prefix (≥ 6 hex chars) of
`blake3(content)`, with position folded in only when identical content is
ambiguous. A handle pins position + content, and resolution either matches
the one form it names or refuses: `stale-handle` (exit 3 — no match; the form
changed or is gone; "re-run `tree`") or `ambiguous-handle` (exit 3 — multiple
matches; extend the prefix). A moved form still resolves; a changed or
deleted one does not; there is no third outcome. `append`/`prepend` are
file-level and take no target.

**Boundary check (the I2 extension for nested edits, §10.3):** the splice may
only touch its window — the node's byte range for replace/patch/delete, the
insert position for inserts. Prefix and suffix equality are verified after
the splice; a violation → `shape-violation` (exit 1), nothing written.

**Marker auto-strip on ingest (§10.4):** `⟦…⟧` markers in `--content`,
`--old-text`, and `--new-text` are stripped before use (lossless, with a
note), so annotated text can be copied straight back into an edit without
leaking markers into the file.

**`--dry-run`:** full validation (resolve, content preparation, splice,
I1–I3 + boundary check, detectors), report the outcome (`"wrote": false`,
the resulting form table and a per-form change summary), **write nothing**.
Exit 0 = "this edit would apply".

### 4.4 materialize (the indent-mode workflow, mechanized)

Input: a draft s-expression written with **correct indentation and no
(reliable) brackets** — the recovery technique the clojure-parinfer skill
prescribes. The tool runs a built-in **indent mode** (parinfer-style
paren-trail inference implemented in-tool, on top of the same
string/comment/char/regex-aware scanner used elsewhere) and returns:

```json
{ "ok": true, "op": "materialize",
  "candidate": "(defn f …)",
  "diff": "--- draft\n+++ inferred\n…",
  "note": "brackets inferred from indentation; verify nesting before use" }
```

Rules: **never** writes to a file, **never** applies smart mode silently —
this is the *only* mode in the tool where brackets are inferred, and the
output is explicitly labeled a candidate. The skill's "never use smart mode"
rule is preserved as: no inference path exists without an explicit,
attributable invocation whose output the agent must verify.

## 5. Addressing

A form is addressed by an opaque **handle** (§10): the shortest unique prefix
of `blake3(content)`, with the form's position folded in only when identical
content would otherwise be ambiguous. The handle is simultaneously the address
and the content pin — resolution either matches or refuses.

- **`--handle H`** — the single edit target for every form-targeting mode
  (`replace` / `patch` / `insert-after` / `insert-before` / `delete`), at any
  nesting depth; `append`/`prepend` are file-level and take no target.
- **`--name X`** — a *read* lookup, not an edit target: the unique top-level
  form whose head defines `X`. Zero matches → `form-not-found`; multiple →
  `ambiguous` (candidates listed with line ranges). The result carries the
  form's handle for use in an edit.

`--addr` (positional, unpinned) and `--expect` (a separate pin) are removed:
for a named form, name + hash *is* a handle, so the two collapse into one
token. Numeric insert anchors (`--after N`) go likewise — anchor by handle, and use
`append`/`prepend` for the file ends.

## 6. Safety invariants (the binary's contract)

- **I1 — no unparseable writes.** A file is only written if the post-splice
  content parses clean (paren mode, zero errors). Atomic write: temp file in
  same directory → write → fsync → rename, preserving the original file mode.
- **I2 — untouched bytes are untouched.** Every form other than the target
  must be byte-identical before/after. Verified by re-parsing and comparing
  per-form hashes; reported in the result (`"untouched": 41, "changed": 1`).
  Any violation → `code: "shape-violation"`, nothing written. (In practice
  impossible given the splice model; I2 is the *verification* that proves it.)
  For nested edits this extends to a **boundary check** (§10.3): every byte
  outside the replaced range is unchanged (prefix and suffix equality) —
  that is what covers edits inside a changed form.
- **I3 — exactly the claimed delta.** The change must fit the mode's allowed
  window: nested edits never move the top-level form count (only the
  containing form may change); top-level replace spans the N content forms,
  insert adds N, delete drops one (I3 generalized from the v1
  one-form-at-a-time rule — §14). Any other delta → `code:
  "shape-violation"`.
- **I4 — content-addressed handle resolution** (§5, §10.1). A handle pins
  position + content; a stale view (its form changed or deleted) ⇒ hard
  refusal, exit 3 (`stale-handle` / `ambiguous-handle`), nothing written.
- **I5 — no silent inference.** Bracket inference exists only in
  `materialize`, candidate-only, labeled (§4.4).
- **I6 — dry-run is complete.** `--dry-run` performs the entire I1–I5
  pipeline (incl. the boundary check) minus the write.

**Nesting-sanity detectors** (the F1 catch). The invariants catch *splice
corruption*; detectors catch *authoring intent* — the case where the agent
wrote balanced content that is still the wrong shape. Shipped detectors
(non-fatal warnings by default; `--strict` flips them to `detector-fatal`,
exit 1):

| ID | Rule | Fires on |
|----|------|----------|
| D1 | `deftest` at top body level of a `defn`/`defn-` | the swallowed-defest bug, exactly |
| D2 | `def`/`defn`/`defn-`/`defmacro` at top body level of another `defn` (local def — legal, rare in most codebases) | accidental top-level defs inside a body |
| D3 | `ns` not at addr 1 | reordered file |

(The v1 D4 — `deftest` inside a `let`/`fn`/`if` — was merged into D1, whose
host set is wide: all executable-scope heads matched by base name, so
`clojure.test/deftest` is caught too; D2 fires on any definition-like `def…`
head, not just the four listed. §14.)

Detector config is a data-driven list `(host-sym, forbidden-sym)` so v2 can
extend per-repo without code changes (e.g. repo rules via a `.cljform.toml` —
out of scope for v1, flag only).

Warnings surface in the JSON `warnings` array **and** in the wrapper's tool
result text prominently — the agent sees `"D1: deftest nested inside defn
make-widget (line 28–101) — intended?"` and decides.

## 7. Rust implementation notes

- **Engine:** `tree-sitter` + the `tree-sitter-clojure` grammar (oakmac).
  *Decision log:* the original proposal named "the `parinfer` crate — the
  exact engine inside parinfer-rust"; that crate does not exist as a usable
  parse API (parinfer-rust is an application, and its transformation pipeline
  is not importable as a syntax tree). tree-sitter is the better foundation
  anyway: byte-exact ranges and line/col for free, a real Clojure grammar
  (strings, regex/char literals, metadata, reader conditionals and `#_` are
  handled by the grammar rather than hand-rolled scanning), and graceful
  error recovery — `has_error()` plus ERROR/MISSING nodes are the paren-mode
  well-formedness check (I1). Top-level forms = the grammar's top-level
  children (comments and `#_` discards excluded from the table; their bytes
  are preserved as gap bytes). Nesting detectors run as an ancestor-stack
  walk over the tree, so a bracket-balanced-but-wrong-shape file (F1) parses
  cleanly and is caught by *shape*, not by parse failure.
- **materialize** implements parinfer indent-mode semantics (dedent below an
  open bracket's column closes its paren trail at the end of the previous
  line; EOF closes the rest) on top of a shared literal-aware scanner;
  output is a labeled candidate, never applied.
- **Hashing:** `blake3` over exact form bytes (and file bytes for
  `file_hash`); handles are shortest unique prefixes (≥ 6 hex chars) of
  `blake3(content)` (§10.1).
- **No evaluation, no I/O beyond the target file.** No Clojure interop, no
  plugin loading. Reader macros (`#(...)`, etc.) are tokens to the
  grammar; they are never executed. This is a hard, reviewable property of the
  dependency set.
- **Atomic write** per I1 (temp+rename+fsync, same-directory temp file).
- **Output:** `serde_json`; human mode is a thin formatter over the same
  structs. No color unless TTY.
- **Performance target:** `forms`/`check` on a 1,100-line / ~50 KB file (a
  large service bootstrap file) < 50 ms cold. O(n) parse; no allocation
  surprises.
- **Binary layout (as built):**
  ```
  cljform/
  ├── Cargo.toml
  ├── src/main.rs        # clap arg parsing, dispatch, JSON envelope
  ├── src/parser.rs      # tree-sitter wrapper, form table, detectors
  ├── src/handle.rs      # handles, tree annotation, marker strip
  ├── src/content.rs     # normalization + bounded repair
  ├── src/materialize.rs # indent mode, unified diff
  ├── src/format.rs      # parinfer paren-mode reindent
  ├── src/splice.rs      # contiguous byte-range splices
  ├── src/invariants.rs  # I1–I6, shape checks, atomic write
  └── tests/             # cli, golden, repair, adversarial, fuzz, format,
                         # handle, F1 regression
  ```
- **Testing:**
  - *Golden:* form-table goldens for representative real-world files (a
    mid-size logic file, a test file, a large bootstrap file); assert
    addr/kind/name/line/hash.
  - *Property:* for random balanced form trees and random target addrs,
    splice-identical ⇒ file byte-identical (round-trip I2); splice-replace ⇒
    only the target range differs.
  - *Fuzz:* unbalanced/garbage inputs → structured JSON error, exit 1/4,
    never a panic, never a partial write.
  - *Regression (mandatory, named):*
    `tests/regression.rs` (`swallowed_deftests_are_detected_with_exact_lines`)
    — feeds a fixture reproducing the F1 shape exactly (an unclosed `defn`
    swallowing the following `deftest`s): must report the reduced top-level
    count **and** fire D1 with the exact line range. This test exists because
    F1 defeated every other gate.

## 8. pi wrapper spec (`clojure-forms` extension)

Placement: shipped as `extension/clojure-forms.ts` in this repo, installed
into `~/.pi/agent/extensions/` (global — the author does Clojure across
multiple repos; behavior is cwd-agnostic; §14 wrapper entry).

**Binary discovery:** resolve `cljform` from `PATH`; optional
`CLJFORM_BIN` env override. Missing binary → tools return a single-line
`error` result with the install hint; the extension must not crash the
session.

**Tools** (typebox schemas; edit content is sent via a temp file —
`--content-file` — never argv, since pi's exec has no stdin channel; the one
exception is `clj_edit`'s autoFormat step, which pipes the raw content into
`cljform format`'s stdin via a direct spawn, because pi.exec closes the
child's stdin):

| Tool | Params | Maps to |
|------|--------|---------|
| `clj_forms` | `{path}` | `cljform forms --json`; refreshes the fingerprint cache |
| `clj_tree` | `{path, depth?, json?}` | `cljform tree` — the primary handle-discovery view (`--depth N\|all`); `json` returns the structured node list |
| `clj_get` | `{path, name? / handle?}` | `cljform get --json` — exact bytes + the form's `⟦handle⟧` |
| `clj_edit` | `{path, handle?, mode?, content? / oldText? + newText?, dryRun?, strict?, repair?, autoFormat?}` | `cljform edit --handle …` — mode auto-selects `patch` when `oldText` is present; append/prepend take no handle; `dryRun` ⇒ `--dry-run`, `strict` ⇒ `--strict`, `repair` ⇒ `--repair`; `autoFormat` (default true) reindents `content` by piping it into `cljform format`'s stdin before the edit (candidate-only parinfer paren-mode reindent, §10.5; on format refusal or a missing binary the original content is sent verbatim, the edit never fails for it; a changed content adds the note `reindented content (parinfer paren mode) before editing`); `oldText`/`newText` are exact patch text and are never reformatted |
| `clj_draft` | `{content}` | `cljform materialize --content-file …` — returns candidate + diff; never writes |

**Fingerprint cache:** in-memory `Map<realpath, forms>`. Refreshed on every
successful `clj_forms`/`clj_get` and post-edit shape check (the CLI result
carries the new table — no re-parse). It exists only to compute the guard
hook's lost/gained-forms delta; correctness never depends on it — a stale
view is refused by handle resolution (`stale-handle`), and the CLI
re-verifies against disk on every operation.

**Guard hook (the reason the wrapper earns its keep even when the agent uses
plain `edit`/`write`):** on `tool_result` for built-in `edit`/`write` whose
target matches `\.(clj|cljs|cljc|cljx|edn)$` and the file exists:
run `cljform check --json` (time-box 2 s; on timeout, skip with a note) and
append to the tool result:
- parse failure → prominent `BLOCKING:` line with line/col from the CLI;
- otherwise a one-line shape summary: `forms: 13→12, lost: deftest
  test-retry-then-success` or `shape ok (13 forms: 1 ns, 2 defn-, 10
  deftest)` + any detector warnings.
The hook never blocks or rewrites (that's `tool_call`'s job); it makes the
shape diff *inescapable* in the transcript, which is what was missing in F1.

**Error surfacing:** CLI JSON errors become tool-result text with an
actionable hint: `parse-error @ line 46 col 1: unclosed open-paren — the new
content is one form short a `)`. Options: fix the content, or use clj_draft
with an indentation-only draft.` Detectors fire as `WARNING:` lines.

**Subagents:** verify on M3 day-one that child runs in the same project load
global extensions (expected yes; `extensionBindings` exists for a reason). If
children don't inherit, the guard weakens to parent-only and the tools are
documented in subagent briefs — acceptable, not a blocker.

**System-prompt note:** register a one-paragraph append via
`before_agent_start` (only when cwd is inside a git repo containing `.clj`
files — cheap probe, cached per session): "For Clojure files prefer the
clj-* tools over raw text edits; after any change, shape reports in tool
results are binding: lost forms and D1–D3 warnings must be addressed or
explicitly justified."

## 9. End-to-end workflow (example)

```
agent: clj_tree {path: "src/myns/my_logic.clj"}
  → annotated source; the target reads
  (⟦7d21⟧defn handle-thing [x] …)

agent: clj_edit {path: …, handle: "7d21", content: <new defn>}
  → wrapper: cljform edit <file> --mode replace --handle 7d21 --json
             (content via --content-file)
  → { ok: true, changed: 1, untouched: 41, wrote: true,
      result: { summary: { handle: <new handle at the same path> }, … },
      warnings: [] }

agent: (runs the repo's linter + focused test suite as usual — cljform never
        replaces the semantic gates; it removes the structural class of
        failures)
```

Failure variant (the F1 replay): agent edits a test file with the `defn`
closer misplaced. Two outcomes, both safe:
- via `clj_edit`: the content splices (repaired from indentation when the
  fix is unambiguous) → **D1 fires** on the result (deftest inside defn) →
  agent sees it in-turn; or the content is unrepairable → `not-one-form` /
  `parse-error @ line …`, file untouched.
- via plain `edit` (if the agent skipped the tools): the **guard hook** appends
  `forms: 13→10, lost: 3× deftest` + D1 to the tool result. Same-turn catch.

## 10. v2 — nested form edits (the handle design)

The v2 headline is editing **any collection form**, not only top-level forms.
The unit of address is an opaque **handle**, not a structural path.

### 10.1 Handle

A form's handle is the **shortest unique prefix** (>= 6 hex chars) of
`blake3(content)`, with the form's position folded in **only when content
alone is ambiguous**:

- **unique content** -> `hash(content)`. Named forms land here: the name
  already guarantees uniqueness, so a named form's handle *is* its hash.
- **duplicate content** (`[]`, `(is (= 1 1))`) -> `hash(content || position)`,
  where position is the structural child-index path from the file root
  (top-level `2`, nested `2.3.1`), keeping the copies distinguishable.

Two deterministic disambiguation rules: different content sharing a short
prefix extends the prefix (git-style); identical content adds the position.

Because the handle is content-addressed, resolution finds the form **wherever
it is**: a moved form still resolves (safe re-aim), while an edited or deleted
form does not (refuse). There is no third outcome, so a handle can never
silently name the wrong form. This subsumes the v1 content-only form hash,
which is now the handle itself, so `--expect` disappears.

**Handles are snapshot tokens.** A handle changes when its form's content
changes, or (for duplicate forms) when it moves; it does **not** change when
unrelated forms are inserted or deleted. A list of handles therefore survives
a batch of edits elsewhere, and only edits to the form itself invalidate it.
Mutations return the new handles they produced.

### 10.2 Annotated view — `tree`

`tree <file>` emits the source with `⟦handle⟧` after each **opening**
collection delimiter:

```clojure
(⟦a3f9⟧ns n)

(⟦7d21⟧defn outer [x]
  (⟦9b12⟧let [a 1]
    (⟦1f3c⟧when x
      (inner x))))
```

- **Open only** — the handle names the whole form, so the tool owns extent and
  the caller never paren-matches.
- **Depth heuristic (default)** — every top-level form is marked; a *nested*
  form is marked (and descended into) only if it spans >= 2 lines. Single-line
  forms (`[x]`, `(inc x)`, `{:a 1}`) are easy to name by text, so they get no
  handle and are not descended into. This is sound: a single-line form cannot
  contain a multi-line descendant. The default view is therefore top-level
  forms plus their multi-line structural children, stopping at leaf-ish
  expressions.
- **`--depth N`** overrides the heuristic: mark forms down to nesting depth N
  (top-level = 1). **`--depth all`** (alias `--full`) marks every collection
  form. Depth is a **view** concern only: the resolver computes handles for all
  forms, so any handle resolves under the default cutoff.
- Emitted from the **same parse the resolver uses**, so every handle is
  guaranteed to resolve.
- **Lossless** — `strip` deletes every `⟦...⟧` and recovers the exact original
  bytes (BOM/CRLF preserved). Property test over the fuzz corpus:
  `strip(annotate(x)) == x`.
- **Collision policy** — if the source already contains the marker glyphs,
  refuse to annotate (`annotate-conflict`, exit 1) and fall back to `--json`.
- Markers go only at AST delimiter positions, never inside strings, regexes,
  comments, or char literals.
- `tree --json` emits the flat node table (`path`, `kind`, `head`, `name`,
  `line`, `depth`, `handle`; null `head`/`name` omitted, internal hash and
  byte offsets not serialized) derived from the same tree. Annotated source
  is the canonical read view; JSON is derived.

### 10.3 Edit contract

`edit <file> --handle H [--mode replace|insert-before|insert-after|delete] [--content ...]`

- Resolve the handle against the current file: content-addressed, so a moved
  form still resolves, but a changed or absent one does not.
- The splice is still a **contiguous byte-range replacement** — the engine is
  unchanged. Invariants extend rather than change:
  - **I1** parse before and after;
  - **I2** every untouched top-level form byte-identical, plus a **boundary
    check**: every byte outside the replaced range is unchanged (prefix and
    suffix equality), which is what covers nested edits inside a changed form;
  - **I3** form-count window.
- Unknown or stale handle -> `stale-handle`, exit 3, nothing written, with a
  "re-run `tree`" note.

### 10.4 Content ingest

Markers are stripped from `--content`, `--old-text`, and `--new-text` before
use (lossless and deterministic, with a note), so annotated text can be copied
straight back into an edit without leaking markers into the file.

`--handle` values are normalized in the same spirit: surrounding whitespace is
trimmed, and a value given as a single `⟦X⟧` span (no marker glyphs inside
`X`) has its marker glyphs removed so the bare `X` is resolved — this is why
copying a `⟦handle⟧` straight from the `tree` view and passing it as
`--handle` works (edit reports the strip with a note). The `⟦…⟧` spans in
submitted *text* are deleted outright (lossless view removal); a `--handle`
given as a single span instead keeps its content, since the handle *is* the
span's content. Values that are not a single well-formed span pass through
unchanged (trimmed) and fail the usual handle checks.

### 10.5 format (paren-mode reindent)

Built. `format <file>` (or stdin) reindents the whole file the way
parinfer's **paren mode** does — the only op that imposes a style; the edit
path only base-shifts. Adopted rules (reference: the local parinfer-rust
checkout's `src/parinfer.rs`, not added as a dependency):

- at a line's first code character (not inside a string, comment, regex,
  or char literal), the indent is clamped to
  `[innermost-open.col + 1, most-recently-closed-child.col]` (or the
  top-level max when nothing is open); leading whitespace is rewritten as
  spaces, tabs counted at display width 2;
- leading closing delimiters move up onto the previous content line (the
  paren trail), and whitespace between trailing closers is removed so
  closers become contiguous;
- comment lines, string interiors, and blank lines are left alone.

Candidate-first: the result mirrors `materialize`'s shape
(`candidate`, `diff`, `note`) and is never written. Verification before
emission: the candidate must re-parse clean and its token stream (the
non-whitespace byte sequence) must equal the input's — only whitespace and
the position of closing delimiters may change. A candidate that fails
either is `format-error` (exit 1) and is never emitted.

**Differential gate:** `format_matches_parinfer_rust` runs both cljform and
the installed parinfer-rust binary (`--input-format json --output-format
text`, paren mode) over a fixture corpus (existing fixtures plus
flat / over-indented / under-indented / nested / standalone-closer /
comment-line / string-with-newline / regex / `#(...)`/`#{...}` / CRLF /
tab cases) and asserts byte equality. The test skips (never fails) when
the binary is absent, so the suite stays hermetic.

### 10.6 Deferred / out of scope

- **Human-facing structural paths** (`7.2.1` syntax): handles are opaque and
  copy-only by design; a readable path syntax can layer on later.
- **Repo detector config** (`.cljform.toml`): per-repo D-rules and fatal sets.
- **Multi-file ops** (`edit --file-list`): prepare-all, then commit-all.
- **`clj-apply-diff`**: form-aware unified-diff application.
- **`resolve`**: merge-conflict regions via a caller-specified line region
  with a marker-conditional write.
- **Editor/REPL integration** (cider-nrepl, lsp-cljk): not planned; the tool
  is deliberately a CLI with a well-known contract.

## 11. Milestones & acceptance

Milestones M1–M4 record the v1 rollout as delivered; the v2 handle surface
(`tree`/`strip`, handle-only `edit`, `format`) landed after M4 (§14).

| M | Scope | Acceptance |
|---|-------|------------|
| **M1** (read-only) | `forms`, `get`, `check` + golden tests + perf | golden tables stable across repeated runs; grammar pinned via Cargo.lock; 1,100-line file `check` < 50 ms |
| **M2** (mutation) | `edit`, `insert`, `delete`, I1–I6, detectors, atomic writes, property/fuzz/regression suites | all §7 tests green; **regression test `swallowed_defest` fires D1 with the exact line range on its fixture** |
| **M3** (wrapper) | extension + 7 tools + guard hook + prompt note; subagent inheritance check; dogfood: redo a representative multi-file one-line schema change across several files, plus one function-body replacement, through the tools on a scratch branch | dogfood diff byte-matches the hand-done equivalent changes; guard demonstrably reports a lost form when a deliberately-broken `edit` is made |
| **M4** (draft, optional) | `materialize` + detector tuning + `CLJFORM_BIN` polish | the skill's indent-mode recovery is a single `clj_draft` call |

Cross-cutting acceptance (always): a `cargo clippy -D warnings` clean build;
no dependencies beyond `tree-sitter`, `tree-sitter-clojure`, `clap`,
`serde`/`serde_json`, `blake3`, `tempfile`, `unicode-width`,
`unicode-segmentation` (+ test crates); MIT license, matching
parinfer-rust. **Accepted:** `unicode-width` and `unicode-segmentation`
(issue 06) — `format` matches parinfer paren mode on display-width columns
(wide characters) and grapheme boundaries; the differential corpus includes
wide characters.

## 12. Open questions

1. **Handle length** — shipped at the shortest unique prefix (≥ 6 hex
   chars). Open: whether a longer default prefix, or a display-length cap in
   `tree`, is wanted for copy-paste robustness in agent transcripts.
2. **`.edn` handling** — same engine, but `deftest`-style detectors are clj
   only; for `.edn`, run checks + splice with an empty detector set.
3. **Should the guard hook also run a linter** (scoped, ~10 s)? *Default: no —
   keep the hook cheap; linting stays an explicit gate.*
4. **Distribution** — `cargo install --path` for now; `cargo publish` to
   crates.io (`cljform`) once stable. The pi wrapper should tolerate both a
   PATH binary and a repo-local `target/release/cljform` (via
   `CLJFORM_BIN`).

## 13. Relationship to the clojure-parinfer skill

The skill stays as the **recovery knowledge base** (stdin discipline, JSON
error mode, indent-mode technique, "kondo is the bracket-type gate").
cljform converts its mechanical parts into *enforcement*:

| Skill instruction today | cljform |
|------------------------|------------|
| "after EVERY edit run `parinfer-rust -m check < file`" | guard hook + `clj_edit`'s shape output (auto, structured, with shape diff) |
| "if unbalanced, fix by hand or via indent mode on a scratch file" | `clj_draft` (indent mode, candidate + diff, in-tool) |
| "never use smart mode" | preserved: inference exists only as explicit, labeled `materialize` |
| "kondo/cljfmt are the bracket-type/style gates" | unchanged; cljform deliberately defers to them (I2 keeps their input stable) |
| "verify registered test counts after adding deftests" | `forms` `contains` summary at edit time; the test runner's registered-test count remains the runtime gate |

## 14. Implementation addendum (as built)

Deviations from the letter of this spec, chosen deliberately during
implementation ("robust and foolproof" outranks the original friction-first
design):

- **Repair beats refusal.** The "content must be exactly one form" gate and
  the hard generation stop are gone. Content is normalized (markdown fences
  stripped, edges trimmed) and unbalanced brackets are repaired by
  indent-mode inference when unambiguous — with a reported diff. Only
  structurally ambiguous content is refused (exit 1, line/col, nothing
  written). Comments-only/empty content is still rejected (interpolation
  accidents).
- **Repair is bounded and truncated content is never repaired.** Unbalanced
  content is repaired by indent-mode inference with a reported diff, but the
  scope is deliberately narrow. The default only **completes** content the
  forced way: appending missing trailing closers. A repair that would close
  an inner form at a mid-file **dedent** is a placement guessed from
  indentation alone, so it is refused (exit 3, `dedent-repair`, carrying the
  candidate and diff); `--repair` opts in. `--strict` refuses **any** repair
  (exit 3, `repair-refused`). Content from an opening markdown fence with no
  closing fence is refused outright (exit 1, `truncated-content`) rather
  than repaired — a dangling fence means the paste was likely cut off.
  Complete content under a dangling fence is still accepted.
- **Indent mode fixed.** The completer compared an *absolute* byte column
  against a *per-line* indent, so an inner form closed a line early and its
  body escaped it (`(let [y 2])` followed by the body). Now `col` is
  line-relative; repair is asserted on nesting, not just paren balance.
- **`materialize` sees the real draft.** The op pre-repaired through
  `prepare` and then inferred again, so a pre-repaired draft always looked
  like a no-op. It now runs on the fence-stripped raw draft. Scope is
  deliberately conservative: it completes missing **closers** from
  indentation; it does **not** invent missing openers, so a fully
  bracket-less draft is returned as-is with a note. Exposed to agents as
  `clj_draft` (candidate + diff, never writes).
- **`--expect` is advisory by default (v1; removed with the v2 handle design).**
  A stale address was re-aimed by hash when the expected form was still
  uniquely findable; `--strict` restored the hard stop (exit 3). Staleness is
  now a `stale-handle` refusal (§10.3).
- **Detectors:** D1's host set is wide (all executable-scope heads, matched
  by base name so `clojure.test/deftest` is caught); former D4 merged into
  D1 (the message names the host). One warning per offending node,
  innermost host.
- **Engine:** tree-sitter + tree-sitter-clojure (§7 decision log). All tree
  walking runs on a 64 MB-stack worker thread; 50k-deep data parses in
  ~0.3 s. BOMs are stripped for analysis and re-prepended on write.
- **Splice hardening:** trailing-comment content gets a newline before a
  same-line neighbor (the neighbor cannot be commented out); delete seams
  keep attached comments with their surviving line.
- **Ops (v1).** `edit` absorbed insert/delete via `--mode` (replace |
  insert-after | insert-before | append | prepend | delete) and accepts
  multi-form content (I3 generalized to an N-form allowed-change window). The
  mode set carries into the v2 handle-targeted `edit`.
- **Granularity (v0.2).** A whole-form edit unit made small changes inside
  large forms force full re-transcription (a dropped `]` in a 60-line resend
  was the observed failure). Added `get` / `clj_get` to fetch exact form
  bytes, and `--mode patch` (`--old-text`/`--new-text`, `clj_edit`
  `oldText`/`newText`): exact-match replacement constrained to the target
  form's byte range, must occur exactly once, never crosses the boundary;
  full I1–I3 + guard pipeline still gates the write. No repair in patch
  mode — a bracket-breaking patch is refused (exit 1), not silently fixed.
  A `patch-not-found` error returns the form's **exact bytes** (the dominant
  failure is `oldText` re-typed from a `sed`/`cat` read), so recovery is one
  call with no `clj_get` round-trip. Strictness is the point: the error was
  granularity, not safety. Sub-form addressing (§10, the v2 handle design)
  shipped with the v2 entry below.
- **Wrapper (v1; the v2 entry below extends it).** tools are
  `clj_forms`/`clj_get`/`clj_edit`/`clj_draft`;
  content passes via `--content-file` (pi exec has no stdin). Guard hook and
  prompt note as specced. Builtin subagents don't inherit extension tools
  (strict allowlists) but the guard hook fires for them; a
  `clojure-worker` user agent ships with the clj tools allowed.
- **`format` (paren-mode reindent, §10.5).** An explicit, candidate-first
  op that adopts parinfer paren-mode rules (reference: local parinfer-rust
  checkout's `src/parinfer.rs` — `correct_indent`, `set_max_indent`,
  `on_indent`, `check_indent`, `on_comment_line`, `finish_new_paren_trail`,
  `append_paren_trail`, `clean_paren_trail`, `add_indent`, `init_line`,
  `is_in_stringish` — not a dependency). It is the only op that imposes a
  style; the edit path only base-shifts. The from-scratch pass replaces the
  reference's `indent_delta` incremental machinery and mirrors its failure
  rules too (unmatched closer, hanging backslash, unbalanced comment
  quotes, unclosed string/opener → `format-error`, exit 1). Emission is
  gated on re-parse + token-stream equality (only whitespace and
  closing-delimiter position may move). Gated by the differential test
  `format_matches_parinfer_rust` against the installed parinfer-rust
  binary, which skips when the binary is absent.
- **Definition heads are recognized by prefix, not a fixed list**, and the
  two uses are distinguished. **Definition-like** (for D2, "accidental local
  def") is any `def…` head: `defn`, `defmacro`, `defmethod`, and project
  macros like `defapifn`/`defstate`/`defroutes`; a small denylist (`default`,
  `defer`, `defensive`) rejects English words that merely start with `def`.
  **Var-defining** (for name extraction) is definition-like minus
  `defmethod`: it extends an existing multimethod, so the symbol after it
  names a var the `defmulti` defines — naming it would make
  `--name <multimethod>` ambiguous between the `defmulti` and every method.
  Metadata (`^:malli/always`, `^:private`, type hints) was already skipped
  correctly; only the variable symbol is taken. Did-you-mean ranks exact and
  substring matches above edit distance.
- **Tests:** the full suite (cli, golden, repair, adversarial, fuzz, F1
  regression) is green; `cargo clippy -D warnings` clean; release `check` on a
  1,081-line file < 10 ms. Dogfooded end-to-end by a local model via the
  wrapper (4 tasks, tests green, shape reports binding) including a live
  bracket-mistake → precise-refusal → self-correction cycle.
- **v2 as built (handles).** §5/§10 are now the shipped surface. A form's
  handle is the **shortest unique prefix (>= 6 hex chars) of
  `blake3(content)`**, with the structural position folded in only when
  identical content would otherwise be ambiguous; handles are
  content-addressed snapshot tokens — a moved form still resolves, a changed
  or absent one refuses (`stale-handle` / `ambiguous-handle`, exit 3,
  "re-run `tree`"). Two new read ops: **`tree`** (annotated source with
  `⟦handle⟧` after each marked collection's opening delimiter; default
  heuristic = top-level + multi-line forms, `--depth N|all` / `--full`,
  `--json` flat node table; `annotate-conflict`, exit 1, if the source
  already contains the marker glyphs) and **`strip`** (lossless marker
  removal — `strip(annotate(x)) == x` — a pure stdout filter with no
  envelope). **`edit` is handle-only**: `--handle H` targets
  replace/patch/delete/insert-after/insert-before; `append`/`prepend` are
  file-level; `--addr`, `--expect`, and the numeric `--after`/`--before`
  anchors were removed (`--name` remains a read lookup on `get`, carrying
  the form's handle). Submitted content is marker-stripped (§10.4) and
  reindented to the target's column (base-shift: line 0 takes the splice
  column; nested inserts that introduce a line break carry the target
  column end to end). The wrapper exposes `clj_tree`, `clj_get` takes
  `--name`/`--handle`, and `clj_edit` targets by `handle` only.
