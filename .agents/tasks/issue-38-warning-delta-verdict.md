# Issue 38 — warning-delta attribution + computed clean verdict
(Owner-approved. Motivation: agents second-guess edits that are fine because
responses carry un-attributed verification signal — on a file with 21
pre-existing D2 warnings, the agent cannot tell "I caused this" from "this
was always here", so it re-audits instead of moving on. The fix is
classification, not subtraction: every signal stays, each is labeled.)

## Mechanism
The edit pipeline already parses the file BEFORE splicing (I1 starts from
the pre-edit form table). Run the detector walk on BOTH parses and diff:

- Match post-edit warnings to pre-edit warnings by (detector id, message)
  as a multiset — line spans shift with the edit, so raw-line matching is
  wrong; multiset counting handles duplicate identical warnings. A post
  warning with no pre counterpart is NEW (loud side: unmatched -> new).
- Envelope: each warning entry gains additive `"new": true|false`; add
  `warningsDelta: {new: N, preExisting: N}` at the envelope level (both
  additive; healthy no-flag envelopes change ONLY by these keys — battery).
- Human: verdict line FIRST (before the diff), computed from verified facts:
  - no new warnings: `verified — C changed, U untouched · warnings: 0 new
    (P pre-existing)`
  - new warnings: `N new warning(s) (P pre-existing):` followed by the NEW
    entries only, then pre-existing (labeled) after the diff/rows as today.
- The verdict is DERIVED (parse ok + I2 untouched + warning delta), never a
  heuristic; uncertainty falls loud (unmatchable -> new). A lying clean
  would be an issue-19-class bug in reverse.

## Guard hook
Same delta in the shape report: `warnings: 0 new (21 pre-existing)` on the
success path; new warnings listed first on the attention path. BLOCKING
behavior unchanged.

## Edge cases
- stdin edits: pre-edit set = stdin parse (works).
- A warning on a form the edit touched: if it existed pre-edit (same
  id+message), it is pre-existing even though the form changed — the
  content the warning describes is the agent's to review anyway (it is in
  the diff).
- A warning that DISAPPEARS (edit fixed the nesting): count drops — the
  verdict line may read `warnings: 0 new (20 pre-existing; 1 resolved)`.
  Resolved-count optional; report if cheap.
- `--strict`: unchanged (warnings still refuse; the delta decorates the
  refusal's message: "N of these are new").
- check op: same delta (pre-parse = stdin/file as-is; a check-only call has
  no before/after — delta applies to EDIT only; check keeps flat output).

## Acceptance
- Tests: new-warning-introduced (nested defn via edit) -> new:true + verdict
  leads with the new entry; pre-existing warning shifted by the edit ->
  new:false; warning eliminated -> resolved counting; multiset duplicates
  (two identical warnings, edit removes one -> 1 pre-existing remains);
  verdict-line leading; JSON additive keys; guard hook delta; stdin path.
- Battery (serial, bounded): healthy no-flag scenarios byte-identical EXCEPT
  additive envelope keys + the verdict line in human output (intended diffs
  enumerated); fixtures with pre-existing warnings (clj-kondo analyzer-style)
  show the 0-new case.
- Gates: build, issue-30 lint gate, cargo test (239 + new), node --check;
  harness guard-hook cells updated.
- SPEC §4.1/§10.3 (verdict + delta contract, matching rule, loud-side
  fallback); README tour: add one line showing the verdict line.

## Report
Matching-rule implementation, edge-case decisions, test list, gates,
battery intended-diff list.
