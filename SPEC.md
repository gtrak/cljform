# cljform — form-addressed Clojure editing: spec

Status: **implemented (v1; patch mode, bounded repair, strict, clj_draft)** · Created: 2026-09-29
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
- A **generation guard** so a stale view of the file (indexes shifted by an
  earlier edit, bytes changed by a formatter) can never target the wrong form.
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
- No nested (path) addressing in v1 — top-level forms only. Reserved for v2
  (§10).
- Not an editor for humans — it's an agent/CLI tool with a human-readable
  fallback.

## 3. Architecture

```
┌─────────────────────────── pi (TS) ───────────────────────────┐
│ extension: clojure-forms (global, ~/.pi/agent/extensions/)    │
│                                                               │
│  tools:   clj-forms · clj-get · clj-check · clj-edit          │
│           clj-insert · clj-delete · clj-draft                 │
│  guard:   tool_result hook on built-in edit/write for         │
│           *.clj *.cljs *.cljc *.cljx *.edn  → cljform check   │
│  state:   per-file form-fingerprint cache (in-memory Map)     │
└───────────────┬───────────────────────────────────────────────┘
                │ spawn: cljform <op> --json …  (content via stdin)
┌───────────────▼───────────────────────────────────────────────┐
│ cljform (Rust, no network, no Clojure runtime)                │
│                                                               │
│  parse:  tree-sitter + tree-sitter-clojure grammar            │
│  map:    top-level form table: addr, kind, name, line range,  │
│          blake3 hash, contained-kind counts                    │
│  ops:    forms · get · check · edit · insert · delete ·       │
│          materialize                                          │
│  safety: invariants I1–I6, atomic write, nesting detectors    │
└───────────────────────────────────────────────────────────────┘
```

The binary is self-contained: given a file path (or stdin) and an operation,
it produces a JSON result and an exit code. All intelligence about *what to
do next* lives in the wrapper/agent; all intelligence about *is this
structure-safe* lives in the binary.

## 4. CLI contract

### 4.1 Global

```
cljform [--json] [--quiet] <op> [args]
```

- `--json` (default when stdout is not a TTY, always on for the wrapper): one
  JSON object on stdout. Human mode: plain text with line/col.
- File argument: path, or `-` for stdin (stdin mode is non-mutating: ops that
  write error with `code: "no-write-to-stdin"`).
- **Exit codes** (the wrapper branches on these):
  - `0` — success (including `--check` clean)
  - `1` — parse/structure error (file or new content unbalanced, detector
    failures configured fatal, etc.)
  - `2` — usage error (bad args, unknown op)
  - `3` — generation guard mismatch (stale `--expect`)
  - `4` — I/O error (unreadable file, write failure)
- **JSON envelope** (success):
  ```json
  { "ok": true, "op": "forms", "file": "/abs/path",
    "fileHash": "blake3:…", "generation": "blake3:…",
    "forms": [ … ], "warnings": [ … ] }
  ```
- **JSON envelope** (error):
  ```json
  { "ok": false, "op": "edit",
    "error": { "code": "parse-error" | "stale-generation" | "not-one-form"
                 | "form-not-found" | "detector-fatal" | "io" | "usage",
               "line": 46, "col": 1, "message": "unclosed open-paren",
               "hint": "…" },
    "forms": [ … ]   // current form table, when the file parsed
  }
  ```
  Error output always carries the *current* form table when the file itself
  parsed, so the agent can re-aim without a second round-trip.

### 4.2 Form table (the `forms` array)

```json
{ "addr": 7, "kind": "defn", "name": "handle-thing",
  "line": [165, 196], "hash": "blake3:…",
  "contains": { "deftest": 0, "defn": 0, "let": 2, "if": 1 } }
```

- `addr` — 1-based top-level form index.
- `kind` — head symbol of the form (`defn`, `defn-`, `def`, `deftest`, `ns`,
  `in-ns`, `do`, `fn`, other, …); for non-symbol heads: `expr`.
- `name` — for named forms (`defn`/`def`/`deftest`/`defprotocol`…), the var
  name; else null.
- `line` — [first, last] 1-based inclusive.
- `hash` — blake3 of the **exact bytes** of the form (no normalization: a
  formatter changes bytes and must invalidate the hash — that's a feature, §6).
- `contains` — counts of macro forms occurring at the form's **top body
  level**: `deftest`, `defn`, `defn-`, `def`, `defmacro`, plus any configured
  detector symbols. This is the cheap shape summary the agent eyeballs
  ("13 forms: 1 ns, 2 defn-, 10 deftest" — expected 11 deftests, so something
  is wrong).

### 4.3 Operations

| Op | Purpose | Mutates | Key args |
|----|---------|---------|----------|
| `forms` | list the form table | no | file |
| `get` | print one form (exact bytes + hash + line range) | no | `--addr N` \| `--name sym` |
| `check` | parse + form table + nesting warnings | no | file |
| `edit` | replace the form at addr | yes | `--addr N` \| `--name sym`, `--expect <hash>`, content via stdin, `--check` dry-run |
| `insert` | insert one new form after addr | yes | `--after N` (0 = before first), `--expect-file <fileHash>`, content via stdin, `--check` |
| `delete` | remove the form at addr | yes | `--addr N`, `--expect <hash>`, `--check` |
| `materialize` | draft→candidate via indent mode | no | content via stdin; emits candidate + unified diff |

**Content rule (edit/insert):** stdin content must be **exactly one complete
balanced top-level form** — no leading/trailing extra forms, no
comments-only, no whitespace-only. Violations: `code: "not-one-form"` with a
breakdown ("parsed 2 forms: defn, deftest" / "parsed 1 comment").

**`--expect` (generation guard):** required on `edit`/`delete`. The blake3 of
the target form as it was when the agent last listed it. On mismatch:
`exit 3`, `code: "stale-generation"`, current table attached. `insert` uses
`--expect-file` (whole-file hash) since it has no target form. Rationale:
indexes shift after any insert/delete; byte hashes shift after any formatter
run. The guard turns "I think form 7 is `handle-thing`" into a
machine-checked fact. The agent recovers by calling `forms` again — one extra
round-trip, never a wrong-form edit.

**`--check` (dry-run):** full validation (parse content, guard, invariants,
detectors), report the resulting form table + a per-form change summary,
**write nothing**. Exit 0 = "this edit would apply".

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

- **By index** (`--addr N`) — primary; cheap and unambiguous with the guard.
- **By name** (`--name handle-thing`) — resolved to the *unique*
  top-level form whose head defines that name. Zero or multiple matches →
  `code: "form-not-found"` / `"ambiguous"` listing candidates with line
  ranges.
- **Nested paths** (`7.2.1`) — v2, §10. The JSON form table already carries
  line ranges; v2 adds `--deep` (per-node ids) to `forms`/`get`.

## 6. Safety invariants (the binary's contract)

- **I1 — no unparseable writes.** A file is only written if the post-splice
  content parses clean (paren mode, zero errors). Atomic write: temp file in
  same directory → write → fsync → rename, preserving the original file mode.
- **I2 — untouched bytes are untouched.** Every form other than the target
  must be byte-identical before/after. Verified by re-parsing and comparing
  per-form hashes; reported in the result (`"untouched": 41, "changed": 1`).
  Any violation → `code: "shape-violation"`, nothing written. (In practice
  impossible given the splice model; I2 is the *verification* that proves it.)
- **I3 — exactly the claimed delta.** `edit`: form count unchanged. `insert`:
  +1. `delete`: −1. Any other delta → `code: "shape-violation"`.
- **I4 — generation guard** (§4.3). Stale view ⇒ hard stop, exit 3.
- **I5 — no silent inference.** Bracket inference exists only in
  `materialize`, candidate-only, labeled (§4.4).
- **I6 — dry-run is complete.** `--check` performs the entire I1–I5 pipeline
  minus the write.

**Nesting-sanity detectors** (the F1 catch). The invariants catch *splice
corruption*; detectors catch *authoring intent* — the case where the agent
wrote balanced content that is still the wrong shape. v1 detectors (non-fatal
warnings by default; `--fatal-detectors` flips them to errors):

| ID | Rule | Fires on |
|----|------|----------|
| D1 | `deftest` at top body level of a `defn`/`defn-` | the swallowed-defest bug, exactly |
| D2 | `def`/`defn`/`defn-`/`defmacro` at top body level of another `defn` (local def — legal, rare in most codebases) | accidental top-level defs inside a body |
| D3 | `ns` not at addr 1 | reordered file |
| D4 | form with head symbol `deftest` appearing *inside* a `let`/`fn`/`if` form | same class as D1, other hosts |

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
  `--expect-file` / `generation`).
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
- **Binary layout (suggested):**
  ```
  cljform/
  ├── Cargo.toml
  ├── src/main.rs        # arg parsing (clap), dispatch, JSON envelope
  ├── src/parser.rs      # tree-sitter wrapper, form table build
  ├── src/ops/{forms,get,check,edit,insert,delete,materialize}.rs
  ├── src/invariants.rs  # I1–I6 + detectors
  └── tests/{golden, property, fuzz, regression}/
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
  - *Regression (mandatory, named):* `tests/regression/swallowed_defest.rs`
    — feeds a fixture reproducing the F1 shape exactly (an unclosed `defn`
    swallowing the following `deftest`s): must report the reduced top-level
    count **and** fire D1 with the exact line range. This test exists because
    F1 defeated every other gate.

## 8. pi wrapper spec (`clojure-forms` extension)

Placement: `~/.pi/agent/extensions/clojure-forms/index.ts` (global — the
author does Clojure across multiple repos; behavior is cwd-agnostic).

**Binary discovery:** resolve `cljform` from `PATH`; optional
`CLJFORM_BIN` env override. Missing binary → tools return a single-line
`error` result with the install hint; the extension must not crash the
session.

**Tools** (typebox schemas; content always sent via stdin, never as an arg):

| Tool | Params | Maps to |
|------|--------|---------|
| `clj-forms` | `{path}` | `cljform forms --json`; refreshes the fingerprint cache |
| `clj-get` | `{path, addr? / name?}` | `cljform get --json` |
| `clj-check` | `{path}` | `cljform check --json` (cheap post-edit sanity) |
| `clj-edit` | `{path, addr? / name?, content, dryRun?}` | `cljform edit` with `--expect` from cache (auto-`forms` first if cache missing); `dryRun` ⇒ `--check` |
| `clj-insert` | `{path, after, content, dryRun?}` | `cljform insert` with `--expect-file` |
| `clj-delete` | `{path, addr, dryRun?}` | `cljform delete` |
| `clj-draft` | `{content}` | `cljform materialize` — returns candidate + diff; never writes |

**Fingerprint cache:** in-memory `Map<realpath, {fileHash, forms, at}>`.
Written on every successful `forms`/mutation (the CLI result carries the new
table — no re-parse). No session-state persistence in v1: correctness never
depends on the cache (a missing/stale entry degrades to one extra `clj-forms`
call; a *lying* cache is impossible because the CLI re-verifies against disk
on every mutation).

**Guard hook (the reason the wrapper earns its keep even when the agent uses
plain `edit`/`write`):** on `tool_result` for built-in `edit`/`write` whose
target matches `\.(clj|cljs|cljc|cljx|edn)$` and the file exists:
run `cljform check --json` (time-box 2 s; on timeout, skip with a note) and
append to the tool result:
- parse failure → prominent `BLOCKING:` line with line/col from the CLI;
- otherwise a one-line shape summary: `forms: 13→12, lost: deftest
  test-retry-then-success (line 81)` or `shape unchanged (13 forms, 2 changed
  at lines 28–101)` + any detector warnings.
The hook never blocks or rewrites (that's `tool_call`'s job); it makes the
shape diff *inescapable* in the transcript, which is what was missing in F1.

**Error surfacing:** CLI JSON errors become tool-result text with an
actionable hint: `parse-error @ line 46 col 1: unclosed open-paren — the new
content is one form short a `)`. Options: fix the content, or use clj-draft
with an indentation-only draft.` Detectors fire as `WARNING:` lines.

**Subagents:** verify on M3 day-one that child runs in the same project load
global extensions (expected yes; `extensionBindings` exists for a reason). If
children don't inherit, the guard weakens to parent-only and the tools are
documented in subagent briefs — acceptable, not a blocker.

**System-prompt note:** register a one-paragraph append via
`before_agent_start` (only when cwd is inside a git repo containing `.clj`
files — cheap probe, cached per session): "For Clojure files prefer the
clj-* tools over raw text edits; after any change, shape reports in tool
results are binding: lost forms and D1–D4 warnings must be addressed or
explicitly justified."

## 9. End-to-end workflow (example)

```
agent: clj-forms {path: "src/myns/my_logic.clj"}
  → addr 7: defn handle-thing, lines 165–196, hash 3fa9…, gen c41d…

agent: clj-edit {path: …, addr: 7, content: <new defn>}
  → wrapper: cljform edit --addr 7 --expect 3fa9… --json  (content on stdin)
  → { ok: true, changed: 1, untouched: 41, generation: 91b2…,
      warnings: [] }

agent: (runs the repo's linter + focused test suite as usual — cljform never
        replaces the semantic gates; it removes the structural class of
        failures)
```

Failure variant (the F1 replay): agent edits a test file with the `defn`
closer misplaced. Two outcomes, both safe:
- via `clj-edit`: content parses as one form → splice OK → **D1 fires** on the
  result (deftest inside defn) → agent sees it in-turn; or content is
  unbalanced → `parse-error @ line …` in the content, file untouched.
- via plain `edit` (if the agent skipped the tools): the **guard hook** appends
  `forms: 13→10, lost: 3× deftest` + D1 to the tool result. Same-turn catch.

## 10. v2 / out of scope

- **Nested path addressing** (`7.2.1` — "the `:or` binding in some `defn`'s
  param map"): `forms --deep` emitting node ids per tree level; splice at
  inner boundaries; invariants unchanged (I2 becomes "untouched *nodes*").
- **Repo detector config** (`.cljform.toml`): per-repo D-rules and fatal sets.
- **Multi-file ops** (apply the same form replacement across N files — e.g.
  adding one schema key to several service config schemas): `edit --file-list`.
- **`clj-apply-diff`**: consume a unified diff and apply form-aware, rejecting
  hunks that cross form boundaries (bridge for patch-based workflows).
- **Editor/REPL integration** (cider-nrepl, lsp-cljk): not planned; the tool
  is deliberately a CLI with a well-known contract.

## 11. Milestones & acceptance

| M | Scope | Acceptance |
|---|-------|------------|
| **M1** (read-only) | `forms`, `get`, `check` + golden tests + perf | golden tables stable across repeated runs; grammar pinned via Cargo.lock; 1,100-line file `check` < 50 ms |
| **M2** (mutation) | `edit`, `insert`, `delete`, I1–I6, detectors, atomic writes, property/fuzz/regression suites | all §7 tests green; **regression test `swallowed_defest` fires D1 with the exact line range on its fixture** |
| **M3** (wrapper) | extension + 7 tools + guard hook + prompt note; subagent inheritance check; dogfood: redo a representative multi-file one-line schema change across several files, plus one function-body replacement, through the tools on a scratch branch | dogfood diff byte-matches the hand-done equivalent changes; guard demonstrably reports a lost form when a deliberately-broken `edit` is made |
| **M4** (draft, optional) | `materialize` + detector tuning + `CLJFORM_BIN` polish | the skill's indent-mode recovery is a single `clj-draft` call |

Cross-cutting acceptance (always): a `cargo clippy -D warnings` clean build;
no dependencies beyond `tree-sitter`, `tree-sitter-clojure`, `clap`,
`serde`/`serde_json`, `blake3`, `tempfile` (+ test crates); MIT license,
matching parinfer-rust.

## 12. Open questions

1. **Hash algorithm ergonomics** — blake3 hex prefixes (12 chars) in
   `--expect`? Short prefixes make agent transcripts readable; collision
   space is irrelevant at this scale. *Default: yes, 12-char prefix, full
   hash available in JSON.*
2. **`--expect` friction vs. safety** — one could make the guard *advisory*
   (warn instead of exit 3) for agent flows. *Default: hard stop; the extra
   `clj-forms` round-trip is milliseconds and the failure mode it prevents is
   F2 (wrong-form edits), which is worse than a redundant call.*
3. **`.edn` handling** — same engine, but `deftest`-style detectors are clj
   only; for `.edn`, run checks + splice with an empty detector set.
4. **Should the guard hook also run a linter** (scoped, ~10 s)? *Default: no —
   keep the hook cheap; linting stays an explicit gate.*
5. **Distribution** — `cargo install --path` for now; `cargo publish` to
   crates.io (`cljform`) once M2 is stable. The pi wrapper should tolerate
   both a PATH binary and a repo-local `target/release/cljform` (via
   `CLJFORM_BIN`).

## 13. Relationship to the clojure-parinfer skill

The skill stays as the **recovery knowledge base** (stdin discipline, JSON
error mode, indent-mode technique, "kondo is the bracket-type gate").
cljform converts its mechanical parts into *enforcement*:

| Skill instruction today | cljform v1 |
|------------------------|------------|
| "after EVERY edit run `parinfer-rust -m check < file`" | guard hook + `clj-check` (auto, structured, with shape diff) |
| "if unbalanced, fix by hand or via indent mode on a scratch file" | `clj-draft` (indent mode, candidate + diff, in-tool) |
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
- **`--expect` is advisory by default.** A stale address is re-aimed by hash
  when the expected form is still uniquely findable; `--strict` restores the
  hard stop (exit 3).
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
- **Ops:** `edit` absorbed insert/delete via `--mode` (replace | insert-after
  | insert-before | append | prepend | delete) and accepts multi-form
  content (I3 generalized to an N-form allowed-change window).
- **Granularity (v0.2).** A whole-form edit unit made small changes inside
  large forms force full re-transcription (a dropped `]` in a 60-line resend
  was the observed failure). Added `get` / `clj_get` to fetch exact form
  bytes, and `--mode patch` (`--old-text`/`--new-text`, `clj_edit`
  `oldText`/`newText`): exact-match replacement constrained to the target
  form's byte range, must occur exactly once, never crosses the boundary;
  full I1–I3 + guard pipeline still gates the write. No repair in patch
  mode — a bracket-breaking patch is refused (exit 1), not silently fixed.
  Strictness is the point: the error was granularity, not safety. Sub-form
  path addressing (§10) remains reserved for v2.
- **Wrapper:** tools are `clj_forms`/`clj_get`/`clj_edit`/`clj_draft`;
  content passes via `--content-file` (pi exec has no stdin). Guard hook and
  prompt note as specced. Builtin subagents don't inherit extension tools
  (strict allowlists) but the guard hook fires for them; a
  `clojure-worker` user agent ships with the clj tools allowed.
- **Definition heads are recognized by prefix, not a fixed list.** Name
  extraction (and D2) treats any `def…`-prefixed head as a definition:
  `defn`/`defmacro`/`defrecord` and project macros like `defapifn`,
  `defstate`, `defroutes`. Only the following symbol names the var, so
  metadata (`^:malli/always`, `^:private`, type hints) is naturally skipped.
  A small denylist (`default`, `defer`, `defensive`, `defmethod` — the last
  attaches to an existing multimethod rather than defining one) avoids
  inventing names. Did-you-mean ranks exact and substring matches above edit
  distance.
- **Tests:** 58 green (cli, golden, repair, adversarial, fuzz, F1
  regression); `cargo clippy -D warnings` clean; release `check` on a
  1,081-line file < 10 ms. Dogfooded end-to-end by a local model via the
  wrapper (4 tasks, tests green, shape reports binding) including a live
  bracket-mistake → precise-refusal → self-correction cycle.
