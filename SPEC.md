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
│         materialize · format · balance                        │
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
- File argument: `check`, `strip`, `format`, and `balance` take a path or
  read stdin when the path is omitted; `balance` additionally accepts an
  explicit `--stdin` flag (mutually exclusive with the path, §4.5). `forms`,
  `tree`, `get`, and `edit` require a path.
  `edit` content arrives via `--content`, `--content-file`, or stdin, and
  patch text via `--old-text`/`--old-text-file` and `--new-text`/
  `--new-text-file` (issue 41: content-from-path is a first-class twin of
  the inline route, not a draft-specific escape hatch). File routes are
  mutually exclusive with their inline twins (both given → exit 2 `usage`);
  the file must exist and be valid UTF-8 (exit 4 `io` envelope — a BOM or
  other invalid bytes surface as the honest UTF-8 error, never a silent
  strip); bytes are taken verbatim (no trimming, no BOM stripping — content
  is content) and the shared content pipeline (fence strip, edge trim,
  balance walk with its verified-tail claim, write-gate enrichment,
  stale-handle recovery) fires identically to the inline route. Provenance
  is echoed in the success envelope as additive result keys: `contentSource`
  (`inline` | `stdin` | `path:…`) for the content modes and
  `oldTextSource`/`newTextSource` for patch (batch ops are inline-only and
  carry none).
- **Exit codes** (the wrapper branches on these):
  - `0` — success (including a clean `--dry-run`; for `balance`: the
    fragment is balanced, with or without an accepted `--tail`, §4.5)
  - `1` — parse/structure error: `parse-error`, `not-one-form`,
    `truncated-content`, `shape-violation`, `detector-fatal` (under
    `--strict`), `annotate-conflict` (`tree` view), `materialize-error`,
    `format-error`, `conflict-markers` (a broken file: git conflict
    markers present — the gate that closes the markers-as-symbols hole,
    §6), `not-supported-broken` (`tree --name` on a broken file) — plus
    `internal-error` (a residual panic caught at the dispatch boundary;
    see the backstop below) — and `balance`'s unbalanced verdicts:
    `kind: "missing-tail" | "mismatch"` on an `ok: true` envelope (the
    diagnostic is the payload, not an error — the verification outcome
    carries the exit code, §4.5)

  **The broken-file state (issue 31).** A file is BROKEN iff it has
  (a) tree-sitter parse errors or (b) conflict markers; both are
  diagnostics. Because tree-sitter reads `<<<<<<<` as a legal symbol, a
  conflicted file *parses* — so the parse gate runs the line-anchored
  conflict scan BEFORE parsing, and markers win over parse errors
  (layered diagnostics: markers first, brackets after the markers are
  resolved by a text edit). Broken: writes REFUSED by every gated op
  (`conflict-markers` or `parse-error`); reads: `check` reports the
  structured `error.diagnostics`, and `tree --recover` renders the
  recovery view (§10.2). `strip` is gated by the markers too (its output
  would carry marker lines as content) but stays a pure byte filter for
  bracket-broken input. Healthy files: the scan finds nothing and every
  envelope stays byte-identical (`error.diagnostics` is an additive key,
  present only when non-empty).
  - `2` — usage error: `usage` (bad args, unknown op, target required but
    missing, `--handle` shorter than 6 hex chars, `--handle` with
    append/prepend, a content field given BOTH inline and from a file —
    the file/inline twins are mutually exclusive, issue 41)
  - `3` — targeting/refusal: `form-not-found`, `ambiguous`, `stale-handle`,
    `ambiguous-handle`, `patch-not-found`, `patch-ambiguous`,
    `unbalanced-content` (unbalanced content, inference is opt-in),
    `repair-refused` (`--strict` beats `--repair`), `target-removed`
    (batch, issue 36: a later op targets a form an earlier op removed or
    replaced)
  - `4` — I/O error: `io` (unreadable file, write failure)
- **Residual-panic backstop (issue 30).** Every envelope op runs inside
  a `catch_unwind` at the dispatch boundary. A residual panic (always a
  tool bug: panicking constructs are lint-denied with audited site-level
  allowances, and release builds run with `overflow-checks = true`) is
  converted to an `ok: false` envelope with code `internal-error`, exit
  1, a message carrying the panic payload, and a note that this is a tool
  bug and the file was NOT written. **No-mid-write statement:** the only
  file write in the binary is the single atomic write at the very END of
  the `edit` pipeline — every fallible computation (read, parse, target
  resolution, payload building, splice planning/application, I1–I3
  verification) completes before it, and every other op is read-only.
  Panics can therefore only fire before the write step, and a caught
  panic proves the file is unchanged (no partial write is possible).
  The profile keeps the default `panic = unwind` (never `abort`): the
  backstop requires the payload to be catchable.
- **JSON envelope** (success):
  ```json
  { "ok": true, "op": "edit", "file": "/abs/path",
    "file_hash": "blake3:…", "forms": [ … ],
    "result": { … }, "warnings": [ … ], "notes": [ … ] }
  ```
  Exception (issue 32, payload-first): the `get` envelope replaces the
  whole-file `forms` array with a `formsCount` integer — the result object
  is what the caller asked for; the count is cheap context. Every other
  envelope (forms, check, edit, …) keeps the `forms` array (the edit
  array's rationale: §4.2). Breaking change, per the house rule.
- **JSON envelope** (error):
  ```json
  { "ok": false, "op": "edit",
    "error": { "code": "stale-handle" | "form-not-found" | "usage" | "io" | …,
               "line": 46, "col": 1, "message": "…",
               "hint": "…",
               "diagnostics": [ … ] } }  // issue 31: broken-file diagnostics, additive
  ```
  Lookup and patch-mismatch errors carry a recovery payload —
  `suggestions` (did-you-mean candidates) on `--name` lookups, and the
  target form's exact bytes on `patch-not-found` — so the agent can re-aim
  without a second round-trip; every other error re-aims via `tree` or
  `forms`.

  **`error.diagnostics` (issue 31).** Broken-file failures carry the full
  diagnostic list — conflict regions (per-side line spans: `head` /
  `base` (diff3 only) / `incoming`, plus `malformed` for unpaired /
  unterminated regions) and, after the markers are resolved, ALL
  parse-error spans (not just the first — the `parse-error` message
  itself stays the first-error text, byte-identical). Additive key:
  absent on every healthy-file envelope.

  **Warning-delta attribution (issue 38).** The EDIT envelope only
  (single op and batch) additionally carries `warningsDelta:
  {new, preExisting}` plus an additive `new: true|false` flag on every
  entry of its `warnings` array — the post-edit detector walk
  attributed against the pre-edit parse (matching rule and loud-side
  fallback: §10.3). Both are additive keys; every non-edit envelope
  (check, forms, …) stays flat — a check-only call has no before/after,
  and its `warnings` entries carry no `new` key.

### 4.2 Form table (the `forms` array)

```json
{ "addr": 7, "kind": "defn", "name": "handle-thing",
  "line": [165, 196], "hash": "blake3:…",
  "contains": { "deftest": 0, "defn": 0, "let": 2, "if": 1 } }
```

The `forms` array is the **top-level listing**. The **nested view** is the
`tree --json` node table (§10.2): `kind`, `head`, `name`, `line`,
`depth`, `handle` per collection node (the structural `path` is internal and
not serialized — it is not addressable).

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

**Which envelopes carry the table (issue 32).** The `forms` array rides
every success envelope EXCEPT `get` — payload-first: a single requested
form's bytes beside the whole-file table was the F16/F17 friction; `get`
carries `formsCount` instead. The **EDIT envelope deliberately keeps the
array**: it is the summary-vs-table cross-check. A summary line that
contradicted the post-edit table (issue 19's stale-head bug, where the
summary reported a pre-edit head as the current identity) is caught by
any consumer diffing the summary against the table; `get`'s result carries
no derived summary that needs the cross-check, so only `get` drops it.
`tree` (the node table) and `strip` are unaffected.

### 4.3 Operations

| Op | Purpose | Mutates | Key args |
|----|---------|---------|----------|
| `forms <file>` | top-level form table (§4.2) | no | — |
| `tree <file>` | annotated view: source with `⟦handle⟧` after each marked collection's opening delimiter (§10.2); `--recover` renders the recovery view for a broken file instead (verbatim source + diagnostics + intact forms, no handles) | no | `--depth N\|all`, `--full`, `--json` (flat node table; `--depth N` filters it to `depth ≤ N`, §10.2), `--name SYM` (selector, §10.2), `--start-line N` / `--end-line N` (line window, §10.2), `--recover` (broken-file recovery view, §10.2) |
| `strip [file]` | delete every `⟦…⟧` marker → the exact original bytes; pure stdout filter, no envelope (§10.2) | no | file or stdin |
| `get <file>` | one form's exact bytes + metadata, including its handle | no | `--name sym` \| `--handle H` |
| `check [file]` | parse + form table + nesting warnings | no | file or stdin |
| `edit <file>` | whole-form / patch / insert / delete, handle-targeted (§5, §10.3) | yes | `--handle H` (append/prepend excepted), `--mode replace\|patch\|insert-after\|insert-before\|append\|prepend\|delete`, `--content` / `--content-file` / stdin (mutually exclusive with the inline twin, §4.1), `--old-text`/`--new-text` (patch) or `--old-text-file`/`--new-text-file` (patch, from a path — same exclusivity/UTF-8/verbatim contract, issue 41), `--strict`, `--repair`, `--dry-run`, `--format-content` / `--no-format-content` (content reindent, §10.3; default on) |
| `materialize` | draft→candidate via indent mode (§4.4) | no | `--content` / `--content-file` / stdin; emits candidate + unified diff |
| `format [file]` | parinfer paren-mode reindent, candidate-first (§10.5) | no | file or stdin |
| `balance [file]` | bracket-balance a fragment, read-only, stdin-first (§4.5): missing closers → the exact mechanical tail; misplaced closer → a line:col diagnosis (no tail offered) | no | file or `--stdin`; `--tail STRING` tests a candidate closing tail |

**Content rule (edit):** content is normalized (markdown fences stripped,
blank edges trimmed) and may carry several top-level forms — the I3 window
generalizes to an N-form allowed change (the v1 "exactly one complete
balanced form" gate is gone; §14). Bracket inference is **opt-in**:
unbalanced content is refused by default (exit 3, `unbalanced-content`,
nothing written) with the inferred candidate and its diff attached and the
hint leading with the `balance` mechanical facts of the SUBMITTED content
(the candidate is balanced by construction — the walk must see the
submitted text): missing closers → `content is missing N closer(s);
mechanical tail (placement is yours to verify): <tail>`; a misplaced
closer → the line:col diagnosis (`mismatch at line L col C: …`),
followed by the standing affordance `pass --repair to apply the inferred
brackets, or submit balanced content (clj_draft can help)` (§4.5);
`--repair` enables the inference (forced missing trailing closers, and
mid-file dedent closures at the caller's explicit risk); `--strict` +
`--repair` refuses it (`repair-refused`, exit 3, with the declined diff —
`--strict` wins). The inference decision and
outcome are **independent of indentation**: inference runs on the content
as submitted (markers/fences stripped only); the base-shift dedent is
computed from the submitted content and applied after the decision; and
the parinfer reindent below runs on the balanced prepared content — none
of it feeds back into the inference. Other refusals: `not-one-form`
(exit 1 — needs a human eye), `truncated-content` (exit 1, an unterminated
opening fence). Content modes then reindent
the prepared content in parinfer paren mode (default on; `--no-format-content`
disables it): the candidate must re-parse and pass the token gate (the
`format` gates, §10.5), otherwise the prepared content is kept with a note
saying the edit was written unformatted — never a failure, and never
silent (issue 14). The content is then base-shifted to the target's column.
`patch`'s `--old-text`/`--new-text` and `delete` carry no reformat of any
kind. `patch` mode is exact-match:
`--old-text` must occur exactly once inside the target form's bytes,
`--new-text` may be empty; there is no repair. `patch-not-found` (exit 3)
returns the form's exact bytes; `patch-ambiguous` (exit 3) names the
occurrence count.

**Content provenance (issue 41):** the single-op edit success envelope
additionally carries the content provenance — `result.contentSource`
(`inline` | `stdin` | `path:…`) for the content modes, and
`result.oldTextSource`/`newTextSource` for patch — additive keys, absent
from every other envelope (batch ops are inline-only; delete takes no
payload). The file route is a REFERENCE, not a payload: a `path:…` value
names a scratch file (the draft artifacts, §4.5), so the caller can correct
the content as a tiny delta on the file and resubmit the same path.

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

**Stale-handle recovery report (issue 40 A, report-only):** the stale
refusal is NEVER retried (the refusal is the freshness guard) — but the
hint re-trees the CURRENT file in the same response and, when the
submitted identity signal finds the form, re-gets it for free (kills the
re-tree→re-get loop). The hex handle is a one-way content hash, so identity
comes only from what the call submitted:
- **patch** — from `--old-text` (the bytes the agent saw): def-like needle
  → match by var name; otherwise a structural token-stream match of
  oldText against the current form's own bytes (the reformat case). A
  unique match reports `this form is now ⟦H⟧ (head name) — current bytes:
  <exact current bytes>`; when oldText's token stream equals the current
  form's, the hint adds `your oldText differs only in whitespace (e.g. a
  formatter ran) — copy the bytes above` (token-stream equality is the
  verified basis — unverified, the claim is OMITTED, never asserted).
  Duplicate names report ALL candidates with spans and pick nothing.
  **Whole-inner-node verbatim match (issue 42 owner fix):** a unique
  token-stream match whose CURRENT bytes ARE the needle verbatim is NOT
  asserted as identity — an unchanged form's content hash still resolves,
  so the stale handle cannot have been its; the stale target is
  elsewhere, and the identity candidates are the node's ENCLOSING forms
  (its ancestor chain, preferring def-like ancestors): one → `your oldText
  sits inside ⟦H⟧ (head name, lines a–b) — if that is the form you were
  editing, use that handle`; several → all candidates with spans, pick
  nothing; none (the match is top-level) → the matched form itself,
  phrased as a candidate. `this form is now` is never asserted in this
  branch. When the matched form's bytes differ from the needle (the 40A
  reformat: tokens equal, bytes rotated), the matched form itself IS the
  plausible stale target and keeps the assertive report.
  A fragment needle that covers no whole form is matched by token-subsequence
  containment (issue 42 C, report-only): its token stream (delimiters
  included — the `token_stream` machinery's tokens) as a contiguous run
  inside EXACTLY ONE current form's stream reports that enclosing form
  (`this form is now ⟦H⟧ (head name) — your oldText is a fragment inside
  it; current bytes: <exact bytes>`) when that form is DEF-LIKE (the name
  pins the plausible stale target); a non-def-like unique containment hit
  is phrased as a candidate (`your oldText sits inside ⟦H⟧ (head) (lines
  a–b) — if that is the form you were editing, use that handle`) and never
  asserts identity. The whitespace claim rides on the same verified
  token-stream equality, so it never fires for a proper fragment.
  Several containing forms report all candidates with spans and
  pick nothing; none → the plain message. Cost guard: the scan runs over
  the leaf-token arrays of the ONE parse already done (early-exiting per
  form — the deep-tree error path stays byte-identical and bounded).
- **replace** — from the content's def name; the content is the NEW bytes,
  so the report stays NEUTRAL (`forms named X in the current file: … — if
  this is the form you were editing, use that handle`) — a rename can
  point the name at a different form, and an identity assertion would be
  unverifiable.
- **get / delete / insert / batch** — no identity signal: the hint is the
  plain message `no form matches the previous identity — re-run tree`.
Nothing is written on any branch; the refusal, exit code, and message are
unchanged.

**Boundary check (the I2 extension for nested edits, §10.3):** the splice may
only touch its window — the node's byte range for replace/patch/delete, the
insert position for inserts. Prefix and suffix equality are verified after
the splice; a violation → `shape-violation` (exit 1), nothing written.

**Changed-region diff (issue 27):** every mutating op returns a unified
diff of its changed region in the result (`"diff"`), in patch's style
(`--- before` / `+++ after` headers, 2-line context): `patch` over the
target form's before/after bytes (as before), the whole-form ops
(replace / insert-after / insert-before / append / prepend / delete) over
the splice window the pipeline already computes (the node range for
replace/delete, the insert position for inserts, the file edge for
append/prepend). The no-op case — the file bytes come back unchanged —
keeps the empty diff. **The human output is result-first (issue 32):**
the changed-region diff (when present), then the summary line (text
unchanged), then the AFFECTED form's table row(s) with handle — the new
form for replace/patch, the inserted form(s) + the anchor's row for
inserts, the deleted form's label + `was lines a–b` for delete — then the
counts line (`N forms; C changed, U untouched — tree <file> for the full
table`), then notes, then warnings. NO whole-file table in human mode, at
any file size (`forms` / `tree` are the full-table surfaces). The JSON
envelope is unchanged (the `result.text` field keeps its issue-27
composition: summary line + repair diff + changed-region diff).
Issue 38: the EDIT human output additionally LEADS with the verdict line
(`verified — C changed, U untouched · warnings: 0 new (P pre-existing)`,
or `N new warning(s) (P pre-existing):` + the new entries) — §10.3

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
this is the tool's candidate-only inference mode (the other path, `edit
--repair`, applies the same inference only behind an explicit flag, §4.3),
and the output is explicitly labeled a candidate. The skill's "never use
smart mode" rule is preserved as: no inference path exists without an
explicit, attributable invocation whose output the agent must verify.

### 4.5 balance (the mechanical bracket-balance primitive)

Born from an incident: a worker drafting a deeply-nested replacement form
got a WRONG draft-mode bracket inference ("closed virtual-future too
early") and fell back to hand-rolled Python paren counters + clj-kondo on
a /tmp fragment. Those fallbacks are string-blind when done naively;
`balance` is not — it walks the SAME lexer as the materialize path
(strings, comments, char literals, and regex never count: a `(` inside a
string, a `; ( (` comment, a `\(` charlit, or a `#"…( …"` regex is not
structure). This is the whole point vs a naive counter, and why the two
facts below are separable:

1. **MISSING closers** — the open stack at EOF, reported LIFO. A count
   cannot see ORDER, a stack can; the answer is the exact mechanical tail.
2. **MISPLACED closers** — a closer that is not the innermost opener's
   partner. A count cannot see these at all; a stack can. The diagnosis
   names the closer's line:col AND the innermost opener it failed to
   close (char + line:col). **No tail is offered for a mismatch**: a tail
   cannot fix a misplaced closer, so suggesting one would be wrong.

Input: a file path, or `--stdin` (mutually exclusive with the path; with
neither, stdin is read — stdin fragments are the primary use), and
optionally `--tail <string>` — a candidate closing tail to TEST
(appended mechanically, then re-walked; accepted iff its chars match the
open stack's LIFO order exactly — same length, same partners, same
order).

Outcomes (the envelope is flat: `kind`/`counts`/`stack`/`tail`/`mismatch`
beside `ok`/`op` — the diagnostic itself is the payload, not a `result`
subtree; `ok` is `true` in all three — the verification outcome, not a
tool error, carries the exit code):

```json
{ "ok": true, "op": "balance", "kind": "missing-tail",
  "counts": { "open": 16, "close": 15, "paren": 15, "bracket": 0, "brace": 0 },
  "stack": [ { "ch": "(", "line": 9, "col": 15 } ], "tail": ")" }

{ "ok": true, "op": "balance", "kind": "mismatch",
  "counts": { "open": 6, "close": 4, "paren": 1, "bracket": 2, "brace": 0 },
  "mismatch": { "closer": "]", "line": 12, "col": 3,
                "innermost": { "ch": "(", "line": 9, "col": 15 },
                "message": "mismatch at line 12 col 3: ] closes nothing — innermost open is ( from line 9 col 15" },
  "stack": [ … remaining openers, open order bottom first … ] }

{ "ok": true, "op": "balance", "kind": "balanced",
  "counts": { "open": 16, "close": 16, "paren": 16, "bracket": 0, "brace": 0 } }
```

- `kind: "balanced"` — stack empty at EOF, no mismatch. Exit `0`. With an
  accepted `--tail` (including an empty tail on an already-balanced
  fragment): still `balanced` (the tail is echoed on `tail`), exit `0`,
  human `tail accepted — fragment balances`.
- `kind: "missing-tail"` — openers remain at EOF, no mismatch on the way.
  Exit `1`. `stack` lists every opener (char, line:col, open order bottom
  first) and `tail` the EXACT string to append (LIFO order). A `--tail`
  whose chars all match but is SHORTER is not a mismatch — the fragment
  still needs its tail — so it stays `missing-tail` (full tail reported,
  a `notes` line rejects the candidate: `too short`).
- `kind: "mismatch"` — the FIRST closer that is not the innermost opener's
  partner (including a `--tail` char: a wrong-order tail, a non-closer in
  the tail, and an EXTRA tail char on an already-balanced fragment all
  land here, at the append position, as loud mismatches). Exit `1`.
  `mismatch.innermost` is `null` when the closer arrived with an empty
  stack ("closes nothing — no openers remain"). `stack` carries the
  openers still open after the mismatch. A tail is NEVER offered for a
  mismatch (and `--tail` on a raw mismatch is not evaluated — a `notes`
  line says so).

Human lines: `balanced — 16 openers, 16 closers` ·
`unbalanced — 16 openers, 15 closers (missing 1: ))` ·
`mismatch at line 12 col 3: ] closes nothing — innermost open is (
from line 9 col 15` · `tail accepted — fragment balances`.

Read-only, never writes, no file needed. Exit `2` usage (`--stdin` with a
path; bad flags), `4` I/O (unreadable file / stdin). The op is intercepted
in `main` before the dispatch backstop on the same basis as `strip`:
the walker is provably panic-free (byte iteration and `pop()` only).

**Wiring (affordances only; edit semantics unchanged).**
- `edit` unbalanced-content refusal (§4.3): the hint leads with the
  mechanical facts of the SUBMITTED content (missing-tail → the `content
  is missing N closer(s); mechanical tail (placement is yours to verify):
  <tail>` line; mismatch → the line:col diagnosis); the inferred-candidate
  display stays as-is — repair stays opt-in and heuristic-labeled.
  **Verified-tail claim (issue 40 B):** after computing the mechanical
  tail, it is VERIFIED by an actual in-memory splice + parse of the
  resulting bytes (content stage: submitted content + tail; write gate:
  resulting file + tail — the real check, not the walk's tautology).
  Parses → `verified: with this tail <content|file> parses` + the standing
  loud caveat `placement is yours to verify: parses-ok ≠ intended
  structure (a tail that closes the wrong form early also parses)` (never
  softened); does not parse → the honest `with this tail <content|file>
  still does not parse — check for a misplaced closer`. The claim never
  auto-applies; the mismatch path never offers tails, as before.
- `edit` resulting-file parse refusal (I1 write gate): the SAME walk runs
  on the submitted content (every mode that takes content) and its verdict
  LEADS the message — the field named, the exact mechanical tail
  ("placement is yours to verify"), or the mismatch line:col diagnosis —
  while the file-level `resulting file does not parse` layer is demoted to
  context (still present, with its own coordinates; the refusal itself is
  unchanged). Content that balances is left alone: the parse failed for
  other reasons and the file-level message stands.
  **Relative delta + repair preview (issue 42 A+B, patch payloads only):**
  for a patch the walk's ABSOLUTE verdict is closer-heavy by nature (tail
  patches), so the lead becomes the scalar RELATIVE delta against the
  COUNTED oldText — `newText has N fewer/more closer(s) than the text it
  replaces (absolute: …)` — with the existing absolute/tail/mismatch
  machinery intact under it; a ZERO relative delta that still fails gets
  `deltas balance relative to oldText — the mismatch is inside newText
  itself: <existing diagnosis>`. The delta runs on FULL delimiter counts
  (a never-aborting count: the walk's early-exit scalars undercount the
  text after a mismatch point, which would make the zero-relative reading
  lie), and oldText is NOT assumed balanced — a patch oldText can carry
  the enclosing form's closer(s). **Balanced-newText tail fumble (issue 42
  owner fix):** when walk(newText) is balanced in ISOLATION but the
  relative delta is nonzero (oldText carried a closer the splice
  consumed), the refusal still leads with the relative delta and derives
  the tail from the resulting FILE's walk (its EOF stack), verified by
  file-level splice as in 40B — the previewed candidate (newText + tail)
  carries the enclosing closer and can never parse on its own, so the
  claim rides on file + tail (`verified: with this tail file parses`);
  the preview is the dry-run old→new+tail diff, except when the candidate
  reproduces oldText exactly (restoring the dropped closer), in which
  case the preview says `identical to oldText (restoring the dropped
  closer)` instead of printing an empty diff. When the missing-tail
  diagnosis fires, the mechanical tail appended at END is also previewed
  as a dry-run candidate diff — `preview (tail appended at end — verify
  placement):` + the unified diff of old→new+tail — with the verified-tail
  claim (40B) riding on THAT splice, stated once (`verified: with this
  tail newText parses` + the standing loud caveat); if the end-append does
  not parse, the refusal says so honestly (`tail at end does not parse —
  the correct placement is inside the form; see the mismatch diagnosis`)
  and shows no preview. Never applied: the refusal is a refusal; whole-form
  content modes keep the absolute framing (nothing to be relative to).
- **No new extension tools (DECISION, issue 42 D):** the extension's
  stale-handle guidance was updated to the post-40A truth (the refusal
  REPORTS the recovered form when identity is verifiable — read the
  refusal before re-running `clj_tree`), and `clj_balance` stays OFF the
  extension tool surface: the CLI (`cljform balance`) remains the
  canonical surface for the balance primitive (tool lists freeze at
  session start anyway; the bash-hop is the designed fallback).
- The `clj_draft` extension tool (issue 41: dual-artifact handoff + the
  prepare-time vote): draft NEVER touches the repo or any user file — the
  scratch artifacts in the system temp dir are its ONLY writes. The
  extension writes `cljform-draft-<n>-sent.clj` (the draft verbatim, exact
  bytes as submitted; `<n>` a per-session counter) and, when inference
  applies, `cljform-draft-<n>-rebalanced.clj` (the completed candidate);
  a no-op inference writes sent only (rebalanced == sent, said so in the
  response). Both paths are reported; the artifacts are additive to the
  candidate + diff + (if landed) structural echo, and they are NEVER
  deleted — the commit is a REFERENCE (`cljform edit <file> --handle <h>
  --content-file <artifact>`), not a re-typed payload, and corrections are
  cheap deltas on the artifact. **The vote (2PC phase 1):** when the agent
  supplies a target (`path` + `handle`, opt-in by giving one; no target →
  artifacts only), the extension runs the splice dry-run — `cljform edit
  <file> --handle <h> --mode replace --content-file <artifact> --dry-run` —
  AT PREPARE TIME: phase 1 carries every check doable without the target,
  abort is always clean (lock-free, nothing written), and the ONLY residual
  commit failure is the between-phases window (target moved → `stale-handle`
  → the §4.5/issue-40A recovery report re-votes rather than aborting). Vote
  yes → `prepared — commit-ready: edit --content-file <path>` (the commit is
  a formality modulo the window); vote no → the dry-run diagnosis surfaces
  AT PREPARE TIME (the cheapest possible moment: the fix is a tiny delta on
  the artifact), with the same recovery affordances as any edit refusal.
  Never auto-commits — the commit call stays the agent's. When inference is
  applied (a candidate + a hunk-carrying diff returned), one line is
  appended: `verify balance before use: cljform balance --stdin` (the draft
  itself is unchanged; it never writes).

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
  The write is the LAST step of the pipeline and the only one that can fail
  on I/O; a residual panic (tool bug) is caught before the envelope is
  printed — `internal-error`, exit 1, nothing written (§4.1 backstop).
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
- **I5 — no silent inference.** Bracket inference exists candidate-only and
  labeled in `materialize` (§4.4), and in `edit` only behind the explicit
  `--repair` opt-in — with a reported diff, never silent; the default
  refuses unbalanced content (`unbalanced-content`, §4.3), and the
  inference decision never depends on the reindent or target column (§4.3).
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

**Broken-file recovery workflow (issue 31).** git conflict -> `check`
names the regions (`conflict-markers`, per-side spans in
`error.diagnostics`) -> `tree --recover` shows both sides (verbatim
source + per-side spans + the intact top-level forms) -> resolve with a
TEXT edit (built-in edit/shell; cljform's write path stays gated by
design) -> `check` ok (or `parse-error` if the resolution broke
brackets — layered diagnostics: markers first, then brackets) ->
cljform flow resumes. cljform never automates the semantic merge and
never writes a broken file.

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
`--content-file` — never argv, since pi's exec has no stdin channel):

| Tool | Params | Maps to |
|------|--------|---------|
| `clj_forms` | `{path}` | `cljform forms --json`; refreshes the fingerprint cache |
| `clj_tree` | `{path, name?, depth?, json?, startLine?, endLine?, recover?}` | `cljform tree` — the primary handle-discovery view (`--depth N\|all`); `name` maps to `--name SYM` (the §10.2 selector: matched subtrees at full depth, zero matches is an ok empty result); `json` returns the structured node list; `startLine`/`endLine` map to `--start-line`/`--end-line` (the §10.2 line window: complete forms only, real file line numbers, the effective span echoed — page large files; the window header/echo passes through unchanged); `recover` maps to `--recover` (the §10.2 broken-file recovery view: on a broken file the JSON path renders the diagnostics + intact-form labels + window echo — no handles; on a healthy file the normal view). **Human path (default, `--human`) is a pass-through:** the annotated source is not a JSON envelope, so on exit 0 the wrapper returns `stdout` directly (BOM-stripped, `trimEnd`) with no `isError` — a successful default `clj_tree` is never an error and is never prefixed `cljform failed:`. Only a nonzero exit (or, in `json` mode, an unparseable/`ok:false` envelope) yields `isError`, using `errorText` when the output parses as an error envelope, else the `cljform failed: ${stderr \|\| stdout}` fallback. **Large-file default (issue 33 Part B):** a DEFAULT call (no `name`/`json`/`depth`/`startLine`/`endLine`/`recover`) on a file with MORE than `TREE_INDEX_THRESHOLD_LINES` (300, a wrapper constant) lines returns the compressed **FORM INDEX** instead of the whole annotated dump: a header `large file: N lines, M top-level forms — showing the form index (handles are live). Annotated view: pass startLine/endLine; a specific form: name.` then one `⟦handle⟧ head name (lines X–Y)` row per top-level form, sorted by line (the `--name`-block label style), with NO source body. The rows come from `cljform tree --json --depth 1` (a small payload since issue 33 Part A); a nonzero exit from that call surfaces as the same error result as the normal path. The index scales with form count, the annotated view with file size — the default picks the right tool instead of the biggest dump. Explicit calls are honored exactly and uncapped (`startLine`/`endLine` → the windowed annotated view; `name` → the selector view; `json` → the node table; `depth` → the cutoff), and below the threshold the pass-through is byte-identical (the CLI's own `tree` contract is unchanged — the wrapper owns the agent ergonomics) |
| `clj_get` | `{path, name? / handle?}` | `cljform get --json` — exact bytes + the form's `⟦handle⟧` |
| `clj_edit` | `{path, handle?, mode?, content? / oldText? + newText?, dryRun?, strict?, repair?, autoFormat?, ops?}` | `cljform edit --handle …` — mode auto-selects `patch` when `oldText` is present; append/prepend take no handle; `dryRun` ⇒ `--dry-run`, `strict` ⇒ `--strict`, `repair` ⇒ `--repair`; `autoFormat` (default true) maps to the in-edit content reindent: `false` passes `--no-format-content`, `true` (the default) passes nothing and lets `cljform edit` reindent the content itself in parinfer paren mode (§10.3/§10.5; a refused reindent is reported by the CLI as a note, the edit never fails for it); the wrapper no longer runs a separate `cljform format` call; `oldText`/`newText` are exact patch text and are never reformatted. **Batch (issue 36):** `ops` (an array of `{handle?, mode?, content? / oldText? + newText?}`) takes precedence over the single-op params (passing both is a wrapper-side usage error) and maps to `cljform edit --batch <tempfile>` — N ops in one atomic call, every handle resolved against the ORIGINAL file and each target tracked across the batch (§10.3 batch contract); the wrapper validates each op's shape up front (same contract as the single-op params, the error names the op index) and renders `result.text` (the composed per-op blocks + aggregate line) as-is, then appends a `next handles (post-batch):` block sourced from the per-op `result.ops[*].summary` (replaced/patched → the new handle; inserted → the inserted handle) so the agent chases post-batch handles without a `clj_tree` round-trip; a successful multi-op batch's `result.text` carries the end-state echo (the `end state (post-batch):` block, §10.3) which the as-is rendering surfaces; a batch ABORT renders `error.batch_ops` (the relabeled would-apply per-op blocks, §10.3 batch abort rendering) directly under the error line — both sourced from the JSON envelope, never from re-parsing human text. **Issue 37 patch-not-found hints:** the wrapper passes the CLI's `hint`/`message` through verbatim, so the indentation-delta diagnosis (B) and the sub-form steering hint (D) reach the agent without wrapper logic. **Next-handle affordance (issue 35):** on every successful (non-dry-run) edit the wrapper appends a final line sourced from the JSON envelope (`result.summary`), never from re-parsing the human text — `replace`/`patch` append `next handle: ⟦H⟧ — use it for the next edit to this form` (from `summary.handle`; falls back to `summary.wasHandle` only when `handle` is absent) so the agent chases the returned handle instead of re-fetching; `insert-after`/`insert-before` render `summary.inserted` (one labeled entry per top-level inserted form, §10.3 insert response contract) as a self-sufficient block — `inserted after ⟦anchor⟧:` / `  ⟦handle⟧ head name (lines a–b)` per entry / `— use these for the next edit` — and a SINGLE-entry insert collapses to the established `next handle:` line instead; `delete` appends nothing (the form is gone — no stale handle). Old envelopes (pre-issue-35 `summary.handles`) still render the legacy bare list |
| `clj_draft` | `{content, path?, handle?}` | `cljform materialize --content-file <sent-artifact> --json` — returns candidate + diff (it never invents missing open brackets; a fully bracket-less draft comes back unchanged with a note). **Dual-artifact handoff (issue 41):** the extension ALSO writes the scratch artifacts to the system temp dir — `cljform-draft-<n>-sent.clj` (the draft verbatim, exact bytes as submitted) and `cljform-draft-<n>-rebalanced.clj` (the completed candidate; when inference is a no-op, sent only — one file, said so) — and reports both paths; these temp files are the ONLY writes draft performs (the repo and every user file are never touched), they are never deleted, and the response still carries candidate + diff + note (the files are additive). **The prepare-time vote (issue 41 C):** when `path` + `handle` are supplied (opt-in; both or neither — one without the other is a wrapper usage error), the extension runs the splice dry-run against the target — `cljform edit <path> --handle <handle> --mode replace --content-file <artifact> --dry-run` — after writing the rebalanced artifact: vote yes → the response says `prepared — commit-ready: cljform edit <path> --handle <handle> --mode replace --content-file <artifact>` (the commit is a formality modulo the between-phases window); vote no → the dry-run diagnosis surfaces AT PREPARE TIME (the fix is a tiny delta on the artifact; nothing was written to the target); no target → no vote, artifacts only. Never auto-commits — the commit call stays the agent's. When inference is applied (the diff carries a hunk) the wrapper appends one line — `verify balance before use: cljform balance --stdin` (the mechanical balance check, §4.5) — and no line when the draft came back unchanged |

**Fingerprint cache:** in-memory `Map<realpath, forms>`. Refreshed on every
successful `clj_forms`/`clj_get` and post-edit shape check (the CLI result
carries the new table — no re-parse). It exists only to compute the guard
hook's lost/gained-forms delta; correctness never depends on it — a stale
view is refused by handle resolution (`stale-handle`), and the CLI
re-verifies against disk on every operation.

**Guard hook (the reason the wrapper earns its keep even when the agent uses
plain `edit`/`write`):** on `tool_result` for built-in `edit`/`write` whose
target matches `\.(clj|cljs|cljc|cljx [legacy]|edn)$` and the file exists:
run `cljform check --json` (time-box 2 s; on timeout, skip with a note) and
append to the tool result:
- parse failure → prominent `BLOCKING:` line with line/col from the CLI,
  followed by one line per `error.diagnostics` entry (issue 31):
  `conflict region lines 30–41 (head lines 28–32 · base lines 33–35 ·
  incoming lines 36–41)` / `parse error lines 7–9: unclosed open-paren
  (form reaches end of file)` — progressive multi-conflict feedback:
  after each built-in edit the hook restates the remaining regions;
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
  forms, so any handle resolves under the default cutoff. **Issue 33 (Part A):**
  the depth gate now applies to the machine surface too — the UNwindowed
  `--json` node table is filtered to `depth ≤ N` when `--depth N` is given.
  No flag (heuristic) and `--depth all`/`--full` keep the unconditional full
  table (byte-identical), and the windowed JSON table is governed by the
  window (the depth flag marks inside the region, it does not filter the
  table — nested nodes of an included subtree stay in it).
- Emitted from the **same parse the resolver uses**, so every handle is
  guaranteed to resolve.
- **Lossless** — `strip` deletes every `⟦...⟧` and recovers the exact original
  bytes (BOM/CRLF preserved). Property test over the fuzz corpus:
  `strip(annotate(x)) == x`.
- **Collision policy** — if the source already contains the marker glyphs,
  refuse to annotate (`annotate-conflict`, exit 1) and fall back to `--json`.
- Markers go only at AST delimiter positions, never inside strings, regexes,
  comments, or char literals.
- **Top-level atom literals (no opening delimiter)** — a top-level char /
  string / number / keyword / symbol / regex / bool literal, or a
  quote/unquote wrapper around one (`'1`, `~1`), has no opening delimiter to
  anchor the handle. It is rendered as an *opaque leaf*: its bytes are emitted
  verbatim with `⟦handle⟧` immediately **before** them (no delimiter
  bookkeeping). This is decided per node from the node's own bytes (a
  `NodeShape::OpaqueLeaf`), so the delimiter lookup is unreachable for leaves.
  issue 29: such a form used to panic the human path (rc 101) while `--json`
  stayed fine; nested occurrences (a char/string inside a collection) are
  content, not nodes, and are untouched.
- `tree --json` emits the flat node table (`kind`, `head`, `name`,
  `line`, `depth`, `handle`; null `head`/`name` omitted; the structural
  `path`, the hash, and the byte offsets are internal and not serialized)
  derived from the same tree. Annotated source is the canonical read view;
  JSON is derived.
- **`--name SYM` selector (issue 26)** — discovery for nested *named* forms.
  `SYM` is matched by exact equality against the def-like name of every
  collection (top-level and nested; no substring matching; `defmethod` never
  names a form — it extends a `defmulti` var). The result contains only the
  matches: the human view is a count header, then each match's annotated source
  block with its line range, sorted by line; each block is annotated **at full
  depth** (the matched form and every descendant carry a `⟦handle⟧`, so the
  inner handles are directly copyable) — `--depth`/`--full` are view concerns
  the selector overrides. `--json` returns the filtered node table (same node
  shape as `tree --json`, plus a `count`); a nested match echoes the queried
  name in `name` (the full table's serialized `name` stays top-level-only). Zero
  matches is `ok:true` with a zero count and empty node list — not an error.
  **Enclosing context (issue 27):** each human match block carries one line
  naming the enclosing top-level form — its handle + label + line range
  (`inside form ⟦7d21⟧ defn outer (lines 3–6)`); top-level matches say so
  (`top-level form`). The JSON match nodes carry `parentHandle` (the
  enclosing form's handle, `null` for top-level matches). Display context,
  not an address: the match's own handle stays the only edit address.
  Discovery-only: handles remain the ONLY edit address. Two-case rule for
  callers (worker guidance): a named nested form → `tree --name X`, copy the
  handle, edit; anonymous nested content → patch within the enclosing form's
  handle.
- **`--start-line S` / `--end-line E` window (issue 28)** — incremental
  viewing of large files. `S`/`E` are 1-based, inclusive-inclusive **line**
  bounds (default: start 1, end EOF; lines, not bytes — the same line
  ranges the view reports; byte offsets deliberately not added, one
  unambiguous way). **Complete forms only:** a top-level form is included
  iff its line span INTERSECTS the window; it is rendered in full even
  when its span extends past the window ("increase the bounds to allow
  it") and its subtrees come with it; no partial form, no handle for a
  fragment. The effective region — the union of the included forms' spans
  — may be LARGER than the requested window, and the output echoes both.
  The windowed view is labeled with **REAL file line numbers**: each
  included form is a block headed by `⟦handle⟧ <label> (lines S–E)` in the
  true file range (never renumbered from the slice start), and the JSON
  nodes keep their true `[start,end]` untouched. Human: a header line
  `forms in lines S–E (complete forms span lines X–Y)` precedes the
  blocks; `--json`: the node table filtered to the included forms' subtrees
  (same node shape) plus `window: {requested: [S,E], effective: [X,Y]}`
  (`effective` is `[]` when nothing intersects — a window past EOF or in
  whitespace between forms is `ok:true` with zero forms and the echo
  explaining it, never an error). `start > end` and 0-valued lines are
  usage errors (exit 2). Composes with `--depth` (the depth cutoff applies
  inside the region) and `--json`; with `--name` the name filter applies
  first and the window then filters matches by span, both reported in the
  echo. With no window given the output is byte-identical to the unflagged
  views (no header, no `window` key).
- **`--recover` — the broken-file recovery view (issue 31).** Opt-in. A
  file is BROKEN iff it has parse errors or conflict markers (§6); plain
  `tree` on a broken file keeps erroring with the gate's envelope.
  `--recover` on a HEALTHY file renders the normal tree view (documented;
  the output is byte-identical with or without the flag). On a broken
  file it renders, deliberately, VERBATIM source + tables — not
  in-source marker insertion: line-true tables are the payload, there is
  no marker-glyph conflict, no annotate complexity, and the source block
  is byte-verbatim (assertable against the on-disk bytes, BOM
  re-prepended for windowed line 1):
  - **Human:** header `file does not parse — N conflict region(s), M
    parse error(s); handles appear when the file is repaired` → the
    verbatim source (composable with `--start-line`/`--end-line` exactly
    as the healthy window path; the effective span echoes as usual — the
    slice IS the window, so requested == effective) → the diagnostics
    table (one line per diagnostic: kind, true line span, message;
    conflict regions show the per-side spans) → the intact-forms table:
    `head/name (lines X–Y)` for every top-level form whose span does NOT
    overlap any diagnostic span (side-region forms inside conflicts are
    CANDIDATES, not agreed content — the region rows carry them
    implicitly via line spans; they are never labeled intact).
  - **JSON:** `result: {diagnostics: [...], forms: [labels],
    window: {requested, effective}}` (the window key only when windowed);
    diagnostics serialize with `kind`, the line spans, `message`, and the
    per-side spans (`head`/`base`/`incoming`, `malformed` when the
    region could not be paired or terminated).
  - **NO handles anywhere (design note, owner-confirmed).** (1) The
    broken region is the edit target and the write path is gated, so
    handles would be inert decoration inviting a doomed edit call. (2)
    Issue 13's "never show a coordinate the tool cannot accept" applies
    literally in the broken state. (3) Content-addressing makes omission
    free: the clean forms' post-repair handles are IDENTICAL to anything
    this view could have shown (unique forms; the position-folded
    duplicates that would move go stale — omission is actively safer).
    The post-repair `tree` returns the same addresses for free.
  - **Composition:** composes with `--start-line`/`--end-line` (the
    source slice + the intact-form labels are windowed) and `--json`;
    `--depth`/`--full` are accepted as a documented no-op (the intact
    list is top-level only; the nested structure lives in the verbatim
    source), but a bad `--depth` value is still a usage error. Does NOT
    compose with `--name` on a broken file: `not-supported-broken`, exit
    1 — the broken file's name table is unreliable (deliberately not a
    usage error); on a healthy file `--name` + `--recover` is the normal
    name view.
  - **Accepted false positive:** a marker-exact line inside a multi-line
    string is treated as a conflict marker — the standard editor
    heuristic (line-anchored, git's exact 7-char convention, optional
    ` <label>`; eight-or-more marker chars or a label without the space
    are NOT markers).

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
- Unknown or stale handle -> `stale-handle`, exit 3, nothing written, with
  the issue-40-A recovery report in the hint (re-tree + re-get for free,
  report-only, never retried; §4.3 stale-handle recovery report).
- **In-edit reindent (issue 11):** for content modes (replace / insert-
  before / insert-after / append / prepend — never patch or delete), the
  prepared content is reindented with the `format` parinfer paren-mode pass
  (§10.5) **by default**, gated exactly as `format` gates (candidate must
  re-parse and pass the token gate, `format_preserves_tokens`; on refusal
  — verification failure or a `format` pass error — the prepared content is
  kept and a note is added saying the edit was written with unformatted
  content, so a refused reindent is never silent, issue 14; the edit never
  fails), and the result is then base-shifted to the target's column. The reindent and the base-shift only
  ever see the balanced, prepared content (issue 12): they are applied
  after the inference decision and never feed back into it, so the
  inference outcome is identical with or without the reindent and at any
  target column. `--no-format-content` disables the
  reindent (`--format-content` is accepted as explicit opt-in); the wrapper's
  `autoFormat: false` maps to that flag, and the wrapper no longer runs a
  separate `cljform format` call.
- **Insert response contract (issue 35):** the `insert-after`/`insert-before`
  JSON summary is self-describing — one labeled entry per TOP-LEVEL form of
  the inserted content: `{action: "inserted", side, wasHandle (the anchor),
  line: [a, b] (the inserted span), inserted: [{handle, head?, name?, line}]}`,
  entries in document order. The labels come from the summary builder's own
  view of the inserted nodes: a nested insert's forms are NOT in the
  post-edit top-level forms table, so the labels can only come from the
  builder. `head`/`name` are omitted when absent. The old bare `handles`
  array (opaque hashes, no binding to which inserted form each names) is
  replaced outright — one way; the only consumer was the wrapper's
  next-handle rendering. The wrapper renders one labeled row per entry
  (`inserted after ⟦anchor⟧:` / `  ⟦handle⟧ head name (lines a–b)` /
  `— use these for the next edit`); a single-entry insert collapses to the
  established `next handle:` line. The affected-rows block (issue 32) is
  unchanged.
- **Batch contract (issue 36).** `edit <file> --batch OPS.json` — N edit
  ops in ONE atomic call. `OPS.json` is a JSON array of per-op objects
  shaped like the single-op flags: `{"handle", "mode", "content" |` /
  `{"handle", "mode", "oldText", "newText"}` (`mode` defaults to
  `replace`; `append`/`prepend` take no handle; `delete` takes none
  else). The batch is a usage error (exit 2) if it is not an array, any
  op is not an object with a valid mode, or any op violates a single-op
  field rule — the error names the offending op's index.
- **Every op resolves against the ORIGINAL file.** Each op's handle is
  resolved against the pre-batch node table — never an intermediate
  state — so one `tree` run feeds the whole batch: docstring-then-body
  on one form is two ops sharing one handle. For each target the engine
  tracks its own position chain (its address in the evolving node table)
  and re-resolves it before every op: earlier ops that MOVE the target
  (an insert before/after its parent, a `prepend`, a top-level delete
  before it, a nested restructure keeping it inside a rewritten form)
  are transparent; a later op on a form an earlier op REMOVED or
  REPLACED is a clean targeted failure — `target-removed` (exit 3),
  naming the op that removed it — never a silent mis-aim and never a
  `stale-handle` false alarm.
- **Sequential application, per-op verification, atomic write.** Ops are
  applied in order to the evolving in-memory file, each through the full
  single-op pipeline (resolve → patch/replace → I1 parse → I2
  untouched-forms byte-identical + boundary check → I3 form-count window
  → strict/repair gates). Any failing op fails the whole batch: nothing
  is written, and the error envelope is the failing op's single-op
  envelope with the abort line leading (`batch aborted at op N of M
  (…) — nothing was written: …`, N 1-based) plus the relabeled
  would-apply blocks of the ops that had run (`error.batch_ops` — see the
  batch abort rendering above); a pre-execution refusal (a handle that
  never resolves) keeps the bare `op N of M (…) : …` prefix.
- **Result shape.** The `--json` envelope keeps the issue-32 result-first
  shape: `result` gains `applied` (op count) and `ops` — one block per
  op: `{op (0-based), handle (null for append/prepend), mode, …the
  single-op result fields…}` (text, summary, changed/untouched,
  repaired/repairDiff, diff, affected rows). The human `result.text` is
  the composed view: one block per op (summary line, changed-region
  diff, affected rows) plus a single aggregate line `N ops applied;
  file: F forms; C changed, U untouched`. Top-level `forms`/
  `formsCount`, `warnings`, and `notes` reflect the post-batch file.
- **Non-goal (by design).** Batch ops cannot target forms the batch
  CREATES — create-then-target stays separate calls (re-run `tree`, or
  use the handles the insert summary reports). Cross-op templating (an
  op referencing another op's output text) is likewise out of scope: ops
  are independent edits sharing one atomic file.
- **Batch-of-one is the single op.** `--batch` with exactly one op writes
  a byte-identical file to the equivalent single-op call (the pipeline
  is shared; only the response shape differs).
- **Batch abort rendering (issue 37).** When a batch ABORTS at op N (an
  op's execution fails), the error message LEADS with the abort line —
  `batch aborted at op N of M (…) — nothing was written: …` (the failing
  op's own message follows) — and `error.batch_ops` carries every op that
  had already run as a relabeled block: `{op, handle, mode, summaryLine,
  diff?, affected?}` where the summaryLine is `op k/M: would apply (not
  written — batch aborted at op N): <op verb> form …` — the accomplished
  tense is dropped (patched → patch, replaced → replace, …), so a failed
  batch never reads as accomplishments. The blocks (and the CLI/wrapper
  human renderings of them) ride the standard error envelope — the code,
  exit, position, hint, and recovery affordances of the failing op are
  untouched; the refusal stands.
- **End-state echo (issue 37).** A SUCCESSFUL multi-op batch appends a
  final `end state (post-batch):` block to `result.text`: one entry per
  DISTINCT touched form (a form edited by k ops appears once, with its
  post-batch handle), each entry the form's post-batch handle, label,
  line span, and the first line of its final bytes — all intended changes
  verifiable in one place. A deleted form is named with its pre-batch
  handle and `— deleted`; a form whose chain no longer resolves after its
  last touching op (restructured in place) falls back to that op's
  summary (no first line). The single-op batch and the single edit carry
  no echo.
- **Head-change note (issue 37).** A `replace` whose NEW content's head
  symbol differs from the target form's head appends the advisory note
  `form head changed: A -> B (check you targeted the intended form)` —
  the observed mis-aim (a handle passed on the outer `(is …)` wrapper
  believing it was the call). Advisory only: head changes are legitimate
  (defn -> def), so it is a note, not a warning, and it never gates
  `--strict` (no refusal, no detector interaction). Same-head replaces are
  silent; vectors/maps and reader-prefixed content carry no head (silent).
- **Warning-delta verdict (issue 38).** The edit pipeline runs the
  detector walk on BOTH parses: the pre-edit parse it already performs for
  I1's start form table, and the post-edit parse of the verification
  tail. Post warnings are matched to pre warnings as a **multiset by
  (detector id, message with the line spans stripped)**: line spans shift
  with the edit, so raw-line or full-message matching would misclassify
  every shifted pre-existing warning as new, and multiset multiplicity is
  what handles duplicate identical warnings (two identical pre warnings
  match two post ones; a third is new). Every UNMATCHED post warning is
  NEW — uncertainty falls loud (an unmatchable warning is reported, never
  silently dropped; a lying clean would be an issue-19-class bug in
  reverse). Envelope: per-warning additive `new: true|false` +
  `warningsDelta: {new, preExisting}` (§4.1). The `check` op is
  unchanged (no before/after: flat warnings, no delta); the BATCH
  envelope's delta is against the pre-batch ORIGINAL (the batch is one
  call; its envelope attributes the batch). Human: a VERDICT line leads
  the edit response (before the diff), derived from verified facts —
  parse ok + I2 untouched + the warning delta — never a heuristic:
  clean → `verified — C changed, U untouched · warnings: 0 new (P
  pre-existing)` (plus `; R resolved` when the edit removed pre-existing
  warnings — the resolved count of the pre side); not clean → `N new
  warning(s) (P pre-existing):` followed by the NEW entries only, with
  the pre-existing ones (labeled `(pre-existing)`) after the diff/rows.
  The pi extension's guard hook carries the same delta against its cached
  pre-edit check snapshot (`warnings: 0 new (21 pre-existing)` on the
  success path; new warnings listed first on the attention path; BLOCKING
  behavior unchanged). `--strict` is unchanged (warnings still refuse;
  the delta decorates the refusal: "N of these are new").
- **patch-not-found hints (issue 37).** On top of the exact-form-bytes
  handback, two non-inferential diagnostics fire on `patch-not-found`
  (composable — both may appear): (B) INDENTATION DELTAS — if the
  whitespace-normalized `--old-text` (leading whitespace per line removed;
  trailing CR tolerated for CRLF) matches EXACTLY ONE region of the
  target form's bytes, the message reports the region's form line and the
  per-line leading-space deltas (`line k of oldText: expected m leading
  spaces, got n`; a tab counts as 2 spaces, as in the §10.5 reindent);
  zero or several normalized regions keep the current message untouched
  (no guessing), as does token-sequence matching (ignoring whitespace
  anywhere) — out of scope by design, riskier. (D) SUB-FORM STEERING — if
  the trimmed `--old-text` matches EXACTLY ONE nested sub-form's bytes in
  the target form, the hint appends `this region is sub-form ⟦h⟧ (label)
  — replace it by handle` (the sub-form exists with those exact bytes:
  the handle is a fact, not an inference); zero or several exact
  sub-forms never steer. The refusal semantics are unchanged in both
  cases — diagnosis and steering ride the message/hint.

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
parinfer's **paren mode** does — the only op that imposes a style on a whole
file; the edit path reindents its submitted content with the same pass (by
default; `--no-format-content` disables it, §10.3) and then base-shifts it.
Adopted rules (reference: the local parinfer-rust
checkout's `src/parinfer.rs`, not added as a dependency):

- at a line's first code character (not inside a string, comment, regex,
  or char literal), the indent is clamped to
  `[innermost-open.col + 1, most-recently-closed-child.col]` (or the
  top-level max when nothing is open); leading whitespace is rewritten as
  spaces, tabs counted at display width 2;
- leading closing delimiters move up onto the previous content line (the
  paren trail), and whitespace between trailing closers is removed so
  closers become contiguous;
- a line the pull-up empties is deleted, not left whitespace-only
  (issue 14 — a deliberate extension beyond parinfer-rust, which leaves
  the vacated line as-is; cljfmt-land removes it, and the pass itself
  creates the line); a blank line the input already had, or a line that
  still carries content after the lift, is left alone;
- comment lines, string interiors, and blank lines are left alone.

Candidate-first: the result mirrors `materialize`'s shape
(`candidate`, `diff`, `note`) and is never written. Verification before
emission: the candidate must re-parse clean, and only whitespace and
closing-delimiter positions may have changed. The gate is
`format_preserves_tokens` (issue 14): the non-closer token stream is
identical in order, the closer count is unchanged, and no closer moved
LATER in the file (closer trails lift upward only). The raw
non-whitespace byte sequence is deliberately NOT the gate: a closer
lifted across a comment line reorders against the comment's bytes — a
comment is not a token — and rejecting that reorder made comment-adjacent
pull-ups `format-error` while the reference formatted them fine.

**Differential gate:** `format_matches_parinfer_rust` runs both cljform and
the installed parinfer-rust binary (`--input-format json --output-format
text`, paren mode) over a fixture corpus (existing fixtures plus
flat / over-indented / under-indented / nested / standalone-closer /
comment-line / string-with-newline / regex / `#(...)`/`#{...}` / CRLF /
tab cases) and asserts byte equality. The test skips (never fails) when
the binary is absent, so the suite stays hermetic. The vacated-line
shapes (standalone closer line, CRLF/tab variants, the closer-after-
string line, and the two comment-adjacent pull-ups) are excluded from
that byte-equality corpus and asserted instead by
`format_vacated_lines_documented_divergence`, which pins both sides:
parinfer-rust's exact output (vacated line kept) and cljform's (vacated
line deleted).

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

1. **Handle length** — settled: shortest unique prefix (≥ 6 hex chars).
   Two dogfoods (T14–T16) used the ≥6-hex affordances with zero copy-paste
   incidents; revisit only if a transcript incident appears.
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
  indent-mode inference when the caller opts in (`--repair`) — with a
  reported diff; the default refuses with the candidate attached. Only
  structurally ambiguous content is refused (exit 1, line/col, nothing
  written). Comments-only/empty content is still rejected (interpolation
  accidents).
- **Inference is opt-in and truncated content is never repaired.**
  Unbalanced content is refused by default (exit 3, `unbalanced-content`,
  carrying the inferred candidate and its diff, nothing written; the hint
  points at `--repair` and `clj_draft`). `--repair` enables the indent-mode
  inference with a reported diff, deliberately narrow in scope: it
  **completes** missing trailing closers and, at the caller's explicit
  risk, closes an inner form at a mid-file **dedent** (a placement guessed
  from indentation alone). `--strict` beats `--repair` (exit 3,
  `repair-refused`, with the declined diff). Content from an opening
  markdown fence with no closing fence is refused outright (exit 1,
  `truncated-content`) rather
  than repaired — a dangling fence means the paste was likely cut off.
  Complete content under a dangling fence is still accepted.
- **Inference is independent of the reindent and target column (issue 12).**
  The inference decision runs on the content as submitted (markers/fences
  stripped only); the base-shift dedent is computed from the submitted
  content and applied after the decision; the parinfer reindent (§10.3) runs
  on the balanced prepared content. The repair outcome is therefore
  identical with or without `--format-content` and at any target column.
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
  `clj_draft` (candidate + diff, never writes to the repo — its only writes
  are the temp-dir scratch artifacts, §4.5/issue 41).
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
  gated on re-parse + the token gate (only whitespace and
  closing-delimiter position may move; a lifted closer may reorder against
  comment bytes — §10.5, issue 14). Gated by the differential test
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
- **Edit output is format-canonical (issue 14).** A project format gate
  caught a file cljform wrote: parse-valid but format-non-canonical (a
  top-level closing paren alone on its own line). Four root causes
  removed: (1) the verification gate rejected a closer lifted across a
  comment line (the raw token stream reorders — comments are not tokens);
  the gate is now `format_preserves_tokens` (non-closer stream equal, no
  closer moves later), and the in-edit reindent uses it too. (2) A refused
  in-edit reindent (gate failure or a `format_paren` error) is always
  reported by a note naming that the edit was written unformatted —
  best-effort, never silent. (3) The splice seam no longer leaves
  displaced parent closers on their own line: the base-shifted content
  never ends on a whitespace-only line, so insert-after / insert-before /
  replace carry the displaced closers onto the last content line, and a
  delete whose tail line holds only the displaced closers pulls them onto
  the previous content line (re-parsed before commit; reverted on
  failure). (4) A line emptied by a pull-up is deleted rather than left
  whitespace-only — the documented §10.5 extension beyond parinfer-rust,
  asserted by `format_vacated_lines_documented_divergence` on exactly the
  vacated shapes (the differential byte-equality corpus keeps everything
  else). New gate: `edit_output_is_format_canonical` — a canonical fixture
  matrix × {replace, patch, insert-after, insert-before, delete, append},
  and every successful edit must come back from `cljform format` as a
  no-op (candidate == content); the R1–R4 repros are golden tests in
  `tests/canonical.rs` and `tests/format.rs`.
- **`tree` line window (issue 28).** `tree <file> --start-line S --end-line E`
  pages large files by line with the **complete-forms-only** invariant: a
  top-level form is included iff its line span intersects the window and
  renders in full; the effective region (union of included spans, which may
  be larger than the window) is what is rendered, and the output echoes
  requested vs effective. Human: header `forms in lines S–E (complete forms
  span lines X–Y)` + one labeled block per included form, each labeled with
  its **TRUE file line range** (`⟦handle⟧ defn f (lines 100–122)` — never
  slice-relative); JSON: the node table filtered to the included forms'
  subtrees (real line ranges untouched) + `window: {requested, effective}`
  (`effective: []` when empty). `start > end` / 0-valued lines: usage error
  (exit 2). Composes with `--depth` (cutoff applies inside the region),
  `--json`, and `--name` (name filter first, window filters matches; both
  echoed). No window: byte-identical legacy output (verified by the
  old-vs-new battery). The wrapper's `clj_tree` gains `startLine`/`endLine`
  (pass-through, real ranges preserved; guidance: page large files, windows
  expand to complete forms).
- **Response scoping (issue 32).** Human-mode outputs are result-first and
  never dump the whole file: `edit --human` is diff → summary line → the
  affected form's table row(s) with handle → counts line
  (`N forms; C changed, U untouched — tree <file> for the full table`) →
  notes → warnings (the whole-file table is gone at every size; the rows
  are pre-composed in `run_edit` via `summary::human_affected_block` from
  the post-edit changed window + the post-edit node table, and ride the
  envelope in a `serde(skip)` field so the JSON stays byte-identical);
  `get --human` prints the form's exact bytes first, then one compact
  metadata line (handle, kind, name, lines, hash, file); `get --json`
  replaces the `forms` array with `formsCount` (the EDIT envelope keeps
  its array — the summary-vs-table cross-check, §4.2); `format`/
  `materialize --human` print `already canonical (no changes)` when the
  candidate is unchanged (no redundant dump, no diff headers). D audit:
  notes/warnings were warnings-first in every human op — normalized to
  result → notes → warnings (CLI + wrapper); the wrapper's `errorText`
  printed a "current form table" header over an always-empty table —
  now conditional; the wrapper's clj_edit double-renders the
  changed-region diff — owner-reported, QUEUED as issue 34 (both issues
  touch the wrapper; not fixed here); and a pre-existing base-suite flake
  (the issue-30 L3 hook tests shared a process-global flag across test
  threads) was fixed with a test-only serialization guard. New tests:
  `edit_human_is_result_first`, `edit_human_affected_rows_no_full_table`,
  `edit_human_notes_before_warnings`, `edit_json_still_carries_full_forms_array`,
  `get_json_carries_forms_count_not_forms`, `human_get_is_payload_first`,
  `format_human_unchanged_is_one_line`, `format_human_changed_stays_candidate_first`,
  `materialize_human_unchanged_is_one_line`. Old-vs-new battery (serial,
  bounded, 63 scenarios × read/JSON/human/exit/stderr): JSON byte-identical
  EXCEPT the four get envelopes' `forms` → `formsCount`; human edit/get/
  format/materialize-unchanged outputs = the intended reshapes; everything
  else (forms/check/tree/strip/broken-file views, error envelopes, all
  other JSON, exit codes) identical.
