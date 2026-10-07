# Issue 39 — `cljform balance`: the mechanical balance primitive

(User-session feedback, incident as the example: a worker drafting a
deeply-nested replacement form got a WRONG draft-mode bracket inference
("closed virtual-future too early") and fell back to hand-rolling Python
paren counters + clj-kondo-on-/tmp-fragment as the oracle. That fallback
is unsafe-looking and string-blind when done naively; cljform already has
the lexer to do it right. Two distinct facts must be separable:
(a) MISSING closers -> a mechanical tail (pure count, string/comment/
charlit/regex-aware); (b) MISPLACED closers -> a stack diagnosis (a count
cannot see these; a stack can). The draft tool's indentation inference is
a heuristic for (a)'s PLACEMENT and cannot fix (b) at all.)

## New op: `cljform balance` (stdin or file)
- Input: `--stdin` (mutually exclusive with a file path arg), optional
  `--tail <string>` (candidate closing tail to TEST — appended
  mechanically, then re-walked).
- Lexer walk with the existing materialize lexer semantics (strings,
  comments, char literals, regex — parens inside strings MUST NOT count;
  this is the whole point vs a naive counter).
- Walk the open-stack; outcomes:
  1. BALANCED MODULO TRAILING (stack non-empty at EOF, no mismatch on the
     way): report per-delim counts, the open stack (each opener: char,
     line:col), and `tail: <exact string to append>` (LIFO order, exact
     chars). With `--tail`: accept iff the tail's chars match the stack
     LIFO order exactly (a tail that closes in the wrong order is a
     MISMATCH report, not a pass).
  2. FULLY BALANCED (stack empty, no mismatch): `balanced` (+ counts).
     With `--tail` on an already-balanced input: report extra/unmatched
     tail chars as mismatch (loud).
  3. MISMATCH (a closer arrives that is not the innermost opener's
     partner): the headline diagnostic — closer char + line:col, the
     innermost opener it failed to close (char + line:col), and the
     remaining open stack. This is the "closed virtual-future too early"
     diagnosis. No tail is offered for a mismatch (a tail cannot fix a
     misplaced closer; do NOT suggest one).
- Exit codes: 0 balanced (incl. balanced-with-accepted-tail), 1
  unbalanced/mismatch, 2 usage. Read-only; never writes; no file needed
  (stdin fragments are the primary use).
- JSON envelope: ok, kind (balanced|missing-tail|mismatch), counts
  {open, close, paren, bracket, brace}, stack entries, tail (when
  missing-tail), mismatch (when mismatch). Human: one of
  `balanced — 16 openers, 15 closers (missing 1: ))` / `mismatch at line
  12 col 3: ] closes nothing — innermost open is ( from line 9 col 15`
  / with --tail: `tail accepted — fragment balances`.

## Wiring (affordances, no behavior change to edit semantics)
- `clj_edit` unbalanced-content refusal: hint gains the mechanical facts —
  missing-tail case: `content is missing N closer(s); mechanical tail
  (placement is yours to verify): <tail>`; mismatch case: the line:col
  diagnosis. Keep the existing inferred-candidate display unchanged
  (repair stays opt-in and heuristic-labeled).
- Extension `clj_draft`: when inference applied (candidate + diff
  returned), append one line: `verify balance before use:
  cljform balance --stdin` (draft itself unchanged; it never writes).

## Tests
- The incident as a golden case: deeply nested draft with a misplaced
  closer -> mismatch diagnosis names the line and the innermost opener.
- Missing-tail: worker's exact arithmetic (16 opens / 15 closes -> tail
  `)`); deep tail (group-by + store-call nesting -> multi-char tail).
- String/comment/charlit/regex traps: parens inside strings, ; comments,
  \(-literals, #"regex( with parens" — none count.
- --tail: correct tail accepted (exit 0); wrong-order tail -> mismatch;
  tail on balanced input -> mismatch.
- JSON shape; exit codes; stdin + file paths.
- Extension: draft hint line present when inference applied.

## Gates
Build, issue-30 lint gate, cargo test (248 + new), node --check, harness;
battery: read-cells byte-identical, edit-refusal hint change enumerated as
intended diff. SPEC: new op section + exit-code table row; README: one
line. Report: outcome taxonomy decisions, test list, gates, battery
intended-diff list.

## Follow-up (deferred by owner — do NOT implement in the main pass)
Write-gate diagnosis (second incident): a replace with unbalanced content
was correctly refused by the write gate ("resulting file does not parse:
unclosed open-paren @ line 1 col 1" — correct gate, useless diagnosis;
agent fell back to Python counters). After the main pass lands and
verifies: on the resulting-file parse-failure path, run the balance walker
on the submitted content and lead with the CONTENT-side verdict (which
field, tail or mismatch line:col), demoting the file-level message to
context. Rationale: balanced-in-isolation content always splices safely
(balanced bytes replacing balanced form bytes ⇒ balanced file), so the
content-stage check is sufficient — the write-gate path exists for when
it is skipped or bypassed. Test: incident shape (unbalanced replacement →
refusal leads with content-side tail/mismatch, not line 1 col 1).
