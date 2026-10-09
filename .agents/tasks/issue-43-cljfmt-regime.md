# Issue 43 — cljfmt as a native formatting regime (--fmt flag + env default)
(OWNER DECISION, overriding the wrapper recommendation: full native
reimplementation of cljfmt's whitespace rules, a --fmt parameter override,
and a per-repo env var default. Maintenance cost accepted; mitigated by
differential testing that pins our output to real cljfmt — divergence is
a test failure, not silent drift. Binding resource discipline applies.)

## Scope
### 1. The cljfmt engine (the meat)
- Line-based reindentation + whitespace normalization with cljfmt's
  semantics, operating with the existing lexer's respect for string /
  comment / charlit / regex interiors (never reformat inside them).
  Transformations (cljfmt defaults): indentation per rule table;
  remove-surrounding-whitespace; insert-missing-whitespace;
  remove-trailing-whitespace; collapse consecutive blank lines; final
  newline. ALL are token-preserving — the existing token gate validates
  both regimes.
- Indentation rules: cljfmt's :block [n] (first n args on the opening
  line, body at +2) and :inner [n] (children at nesting depth n get +2
  relative) semantics, argument-alignment default for unknown lists, and
  the SHIPPED default rule table. Vendor the table as data with VERIFIED
  attribution (cljfmt is weavesjesser/cljfmt — verify repo URL, license
  compatibility, and record the commit/versions in the source header and
  CONTRIBUTING/attribution notes BEFORE commit; the justinj/parinfer-rust
  404 lesson applies: verify links before landing them).
- Reader conditionals / metadata edge cases: match cljfmt's documented
  behavior; where behavior is undocumented, choose the simple reading and
  record the decision in the report.

### 2. Regime selection
- `cljform format [file] [--fmt cljfmt|parinfer]`: explicit override;
  unknown value → usage error (exit 2). Default: parinfer (back-compat).
- `CLJFORM_FMT` env var (cljfmt|parinfer): per-repo default the user sets
  (direnv/.envrc etc.). Precedence: flag > env > parinfer. Invalid env
  value → usage error, LOUD (a silently-ignored regime default would
  corrupt trust in the flag). Envelope echoes the effective regime
  (additive `fmt` key) + its source (flag|env|default) — battery-visible.
- check/edit unchanged; the regime affects `format` only. Edits remain
  byte-surgical (I2 untouched) — a cljfmt-clean file stays clean through
  ordinary edits.

### 3. Gate dry-run (free payoff)
- `cljform format --fmt cljfmt` on a cljfmt-gated repo IS the pre-push
  check: empty diff = gate passes. Document as the intended workflow.

## Acceptance — the differential bar
- PROBE the environment for real cljfmt (bb -m / clojure -M -m
  cljfmt.tool / lein). If AVAILABLE: differential-test our cljfmt regime
  vs real `cljfmt fix` on the bench corpus (ring + clj-kondo files) —
  byte-equal output is the bar; every mismatch is a bug or a recorded
  edge-case decision. If UNAVAILABLE: golden corpus from cljfmt's
  documented behavior (hand-built fixtures per rule: block, inner,
  arg-align, surrounding/missing/trailing whitespace, blank-line
  collapse, final newline) + note in the report that differential
  validation is pending a cljfmt-bearing environment.
- The bench lesson stands: empty-diff on non-cljfmt-canonical corpora is
  NOT the bar for the parinfer regime; for the CLJFMT regime on a
  cljfmt-clean corpus, empty-diff IS the bar (that is the regime's
  contract).
- Invariants for both regimes on every battery cell: token stream
  verified unchanged, parse ok, nothing written unless the existing
  write path (if any) is explicitly invoked.

## Tests
- Engine: rule-table coverage (block/inner/arg-align, nested, multi-arity
  style), string/comment interiors untouched, charlits/regex, CRLF,
  BOM-as-error, idempotence (format twice == once), whitespace transforms
  each individually.
- Selection: flag beats env beats default; invalid env → exit 2; envelope
  echo (fmt + source); back-compat (no flag + no env == parinfer, existing
  cells byte-identical).
- Differential or golden per the probe outcome; idempotence on
  cljfmt-clean ring/analyzer files (second run empty).
## Gates
Build, lint gate, cargo test (318 + new), node --check, harness; battery
serial/bounded with intended diffs enumerated (format cells only +
additive envelope keys); peak RSS in the report. SPEC §6/§10 rewritten
(two regimes, precedence, env var, attribution); README format section +
tour line; attribution notes updated only after link verification.
## Report
Engine decisions (rule semantics, edge cases), probe outcome (cljfmt
available?), differential results or golden justification, attribution
verification, test list, gates, battery intended-diff list, peak RSS.

## 4. Re-runnable differential suite (owner addition — both regimes)
tests/cljfmt-diff/ + scripts/cljfmt-diff.mjs (harness-style). Committed
corpus: synthetic rule fixtures (block, inner, arg-align, nested, each
whitespace transform, CRLF, comments/strings/charlits) + committable bench
excerpts (ring, clj-kondo) with attribution headers. THREE-WAY per regime:
ours-vs-SNAPSHOT (regression guard — in the default cargo suite, no
external tool needed), ours-vs-REFERENCE-NOW (live differential, when the
reference tool exists), reference-NOW-vs-snapshot (upstream drift — a
finding, never our bug, reported distinctly). Runner modes
regression|differential|snapshots|all, per-file verdicts, nonzero exit on
any ours-vs-X mismatch. Snapshot mode pins the reference VERSION in a
manifest. NOT in default gates; re-run recipe (babashka install,
snapshot regeneration) documented in script header + SPEC.

## 5. Parinfer regime in the same suite (owner addition)
Reference: parinfer-rust (eraserhd upstream; local checkout
/home/gary/dev/parinfer-rust, cargo build). Same three-way design,
runner --regime cljfmt|parinfer|all. cljform's parinfer format is
reindent-only — any INTENTIONAL deviation from parinfer-rust paren-mode
becomes an ENUMERATED divergence manifest (expected-mismatch + reason)
inside the suite; byte-faithful ⇒ empty manifest, byte-equality is the
bar. Attempt to make BOTH regimes' suites live now (network available;
try babashka + bb.edn git dep for cljfmt; build parinfer-rust locally)
and generate snapshots with versions pinned; if a reference is
unavailable, commit runner + corpus + loudly-marked pending manifest and
use the golden-corpus bar.

## Report additions
Suite status per regime (live vs pending), snapshot manifests, divergence
manifest contents (or empty), probe outcomes.
