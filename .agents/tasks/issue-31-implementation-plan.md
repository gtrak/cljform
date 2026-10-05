# Issue 31 — IMPLEMENTATION PLAN: broken-file recovery (general mechanism)

Owner-directed restart: the first implementation attempt drifted conflict-first
and was discarded. This plan is the single source of truth for the rework.
General mechanism FIRST; conflicts are one diagnostic source (the motivating
one). Base: git 8e1982c, 158 tests green, tree clean. Resource discipline file
binds (serial, bounded-run, amendment rules).

## 1. Model (write this first — everything hangs off it)

A file is BROKEN iff it has (a) tree-sitter parse errors or (b) conflict
markers. Both are diagnostics; the tool's state machine:

    healthy -> (any op) normal behavior
    broken  -> writes REFUSED (all ops gated); reads: check reports structured
               diagnostics; tree --recover renders the recovery view

Diagnostic model (new `src/broken.rs`):

```rust
pub enum Diagnostic {
    ParseError { span: (usize, usize), message: String },      // 1-based inclusive lines
    ConflictRegion { head: Option<(usize, usize)>,
                     base: Option<(usize, usize)>,             // diff3 only
                     incoming: Option<(usize, usize)>,
                     malformed: bool },                        // unpaired/unterminated
}
pub struct Broken { pub diagnostics: Vec<Diagnostic> }          // ordered by line
```

- `collect_parse_diagnostics(root) -> Vec<Diagnostic>`: walk ERROR/MISSING
  nodes (parser.rs already does `is_error()/is_missing()` at ~line 148 — reuse
  the walk; ALL spans, not just the first).
- `scan_conflicts(bytes) -> Vec<Diagnostic>`: line-anchored scanner (below).
- `broken(bytes, tree) -> Broken`: union of both, ordered by line.
- Serde shape for the JSON envelope (`error.diagnostics`): additive key, only
  present when non-empty (healthy-file envelopes must stay byte-identical).

## 2. Conflict scanner (in broken.rs)

Line-anchored, 1-based inclusive line spans. A line is a marker iff, after
stripping a trailing `\r`, it matches `^(<{7}|={7}|>{7}|\|{7})( .*)?$`
(git's exact 7-char convention + optional label).

State machine over lines:
- `<<<<<<<` opens a region (head start). `|||||||` records base start.
  `=======` closes head (or base) and opens incoming. `>>>>>>>` closes the
  region and pushes it.
- ROBUSTNESS (this runs on garbage — hard requirement): `<<<<<<<` while
  already open -> push the current region as `malformed` (span = open..line-1)
  and open a new one. `=======`/`>>>>>>>`/`|||||||` with no open region ->
  push a standalone `malformed` region covering that line. EOF while open ->
  push `malformed` (span = open..last line). Never panic, never loop forever,
  never drop bytes silently.
- False positives (a marker-exact line inside a multi-line string) are
  ACCEPTED and documented — the standard editor heuristic.

## 3. Gate integration (src/ops.rs parse_or_fail — the gate ALL ops share)

Order (layered diagnostics, per owner):
1. `scan_conflicts` first (linear, trivial cost). Regions non-empty ->
   `Fail(exit 1, "conflict-markers")`: message
   `"N conflict region(s) (lines 5-12, 30-41, ...)"` (+ per-side detail in
   `error.diagnostics`), hint: "resolve the conflict(s) with a text edit;
   cljform resumes when the file parses".
2. No markers but parse errors -> code stays `parse-error` (existing
   contract), message UNCHANGED byte-for-byte (first-error text as today),
   and `error.diagnostics` gains the full ParseError list (additive key).
   Gate even when tree-sitter would accept the file (markers-as-symbols) —
   THIS CLOSES THE SAFETY HOLE: today check says ok:true on a conflicted
   file and edits succeed (verified repro; regression test required).

## 4. Recovery view: `tree --recover` (run_tree branch + render in broken.rs)

- Opt-in flag on Tree CLI. Plain `tree` on a broken file keeps erroring
  (byte-identical envelope). `--recover` on a HEALTHY file: renders the
  normal tree view (documented; diagnostics empty).
- Rendering decision (deliberate deviation from the draft attempt):
  VERBATIM source + tables, NOT in-source marker insertion. Reasons:
  line-true tables are the payload; no marker-glyph conflicts; no annotate
  complexity; the source block must be byte-verbatim (assertable).
  - Human: header `file does not parse — N conflict region(s), M parse
    error(s); handles appear when the file is repaired` -> verbatim source
    (composable with --start-line/--end-line exactly as the healthy window
    path; effective-span echo as usual) -> diagnostics table (one line per
    diagnostic: kind, true line span, message; conflict regions show per-side
    spans) -> intact-forms table: `head/name (lines X-Y)` for every
    top-level form whose span does NOT overlap any diagnostic span
    (side-region forms inside conflicts are CANDIDATES, not agreed content —
    the region rows carry them implicitly via line spans; do not label them
    as intact).
  - JSON: `{diagnostics: [...], forms: [labels], window: {...}}`; no handles
    anywhere. DESIGN NOTE (owner-confirmed): no handles because (1) the
    broken region is the edit target and the write path is gated; (2) issue-13
    "never show a coordinate the tool cannot accept"; (3) content-addressing
    makes omission free — clean forms' post-repair handles are identical to
    anything shown pre-repair (position-folded duplicates that move would go
    stale; omission is actively safer).
- COMPOSITION: --recover composes with --start-line/--end-line and --depth
  (windowing applies to the source slice + intact-form labels). Composes
  with --json. Does NOT compose with --name (a broken file's name table is
  unreliable; document the refusal as a usage error? NO — simpler: --name
  wins an error "not supported on broken files"; document).

## 5. check enrichment + guard hook + wrapper

- check's broken-file envelope carries `error.diagnostics` (sections 3).
- Guard hook (extension): on check failure, if `error.diagnostics` present,
  render one line per diagnostic ("conflict region lines 30-41 (HEAD/base/
  incoming)", "parse error lines 7-9: unclosed open-paren") instead of only
  the current single BLOCKING line. Progressive multi-conflict feedback falls
  out free: after each built-in edit, the hook states the remaining regions.
- Wrapper: `clj_tree` gains `recover?: boolean` -> `--recover`; guidance line
  for the merge workflow. Single file (jiti constraint); deploys via symlink.

## 6. Workflow being enabled (SPEC section, verbatim intent)

git conflict -> check names regions -> tree --recover shows both sides +
intact structure -> resolve with a TEXT edit (built-in edit/shell; cljform's
write path stays gated by design) -> check ok (or parse-error if the
resolution broke brackets — layered diagnostics: markers first, then
brackets) -> cljform flow resumes. cljform never automates the semantic
merge and never writes a broken file.

## 7. Tests (each maps to an acceptance bullet)

Scanner (unit, in broken.rs): all four markers; pairing; diff3; malformed
(double-open, close-without-open, EOF-unterminated); CRLF; false-positive
string line; no markers.
Gate: THE SAFETY-HOLE REGRESSION (conflicted-parsable fixture: check/edit/
strip/format/tree all refuse with conflict-markers — old behavior was
ok:true + successful edit); conflicted-unparsable; healthy file untouched.
Recover: pure-parse-error files (unclosed at EOF; stray closer mid-file;
corrupted MIDDLE of a large file with intact forms on BOTH sides — the
generality proof; truncated file); conflicted multi-region (segmentation +
side attribution); window composition; verbatim-source assertion (rendered
source block == file bytes for the window); no handle-format markers added;
--recover on healthy file == normal view; --name on broken file -> documented
usage error. Wrapper harness: clj_tree recover param; guard hook names
regions. Layered diagnostics: markers present -> conflict-markers even with
parse errors; markers resolved but brackets broken -> parse-error.

## 8. Battery + gates

- Gates: build; clippy WITH the issue-30 gate (-D warnings -W
  unwrap_used/expect_used/panic/unreachable -D todo); cargo test (158 + new);
  node --check; hx harness (wrapper recover + guard hook regions).
- Battery (serial, bounded-run, amendment rules; old = git archive HEAD
  release build): healthy-fixture no-flag scenarios byte-identical (the
  scanner must be invisible on clean input — fixtures incl. BOM/CRLF/20k-deep);
  intended diffs ONLY: (a) conflicted fixtures (ok:true -> conflict-markers,
  or parse-error envelope gains diagnostics key), (b) parse-error fixtures'
  enriched envelopes (additive key), (c) new --recover surface.
- SPEC: §4.1 conflict-markers; parse-error diagnostics statement; §tree
  --recover (complete outline of rendering + no-handles design note);
  §6 workflow; §8 wrapper rows; guard-hook note. README gates/ops lines.

## 9. Implementation order (suggested commits)

1. broken.rs: Diagnostic model + scanner + collect_parse_diagnostics + unit
   tests (no integration yet).
2. Gate integration + safety-hole regression + gate tests.
3. tree --recover rendering + window composition + tests.
4. check enrichment + guard hook + wrapper + harness.
5. SPEC/README + full battery.

Each step gates green before the next; the battery runs once at the end
(plus a mid-point run after step 3 if the worker prefers).

## 10. Out of scope (do not do)
- No write-path changes; no exception to parse_or_fail for writes; no
  automated merge resolution; no comment-form changes; dis_expr stays
  skipped; no handles in the recovery view; no byte offsets.
