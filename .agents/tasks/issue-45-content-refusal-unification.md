# Issue 45 — unify content-refusal diagnostics through the walker (the not-one-form gap)
(Second real-issue report on the content path: a ~270-line 6-form replace
with two dropped closers hit `not-one-form @ line 185 col 44: content
structure is ambiguous for bracket repair — mismatched close: found ']'
but '(' (opened col 25) is still open` + a clj_draft hint. Reproduced
(byte-different, same shape): 6-form blob with a `]` arriving while `(` is
open → `not-one-form @ line 14 col 11 ... (opened col 0)` + clj_draft
hint, while `balance --stdin` on the SAME blob instantly reports
`22 openers, 18 closers (missing 4)`. The walker already computes
everything; this refusal path never calls it. The refusal was correct and
atomic; every gap is diagnostic.)

## Behavior
### 1. Route bracket failures through the walker (all content paths)
Wherever content preparation fails for BRACKET reasons — single-form,
multi-form, repair-candidate construction, the ambiguous-structure path —
run the balance walk and diagnose with it:
- Mismatch: closer char + line:col, then the FULL open stack at the
  mismatch point (not just innermost): every opener with char + line:col
  + HEAD TOKEN (first symbol after the opener, e.g. `(try @179:25`,
  `(catch @181:29` — extracted from the source slice; cheap). Owner item
  1+2: the culprits are the still-open openers, and they carry lines.
- Net-unclosed count at mismatch/EOF from FULL counts (paren/bracket/
  brace breakout) — reportable even when no tail can be offered (LIFO
  rule for tails unchanged: mismatch ⇒ no tail).
- Missing-tail content keeps the existing 39/42 treatment (tail, preview,
  verified claim).
### 2. Error-code taxonomy (breaking, per house rules)
- Bracket failures of content → `unbalanced-content` (mismatch vs
  missing-tail distinguishable via the diagnosis + additive `kind` field).
- `not-a-form` (NEW code): content parses but is structurally wrong for
  the target (form-count/arity) — the only legitimate home of the old
  not-one-form. The ambiguous-structure-for-repair case is a bracket
  failure → unbalanced-content, NOT not-one-form.
- Exit codes: unbalanced-content stays 3; not-a-form = 3 (targeting
  class). SPEC exit table updated.
### 3. Hint routing
- Mismatch hints: the stack + `the ] at line 14 is the first close that
  no longer matches — a closer was likely dropped earlier` + standalone
  iteration pointer `cljform balance --stdin` (exists, verified on this
  blob). NO clj_draft suggestion for bracketed-content typos — draft is
  for bracket-less drafts (its own contract); keep the draft pointer only
  on paths where inference is genuinely the tool (missing-tail with clean
  indentation keeps 42's preview which supersedes it).
- Opener lines ALWAYS present in mismatch diagnoses (col-0-style gaps are
  the bug class here).
### 4. Standalone validate (item 5)
- `balance --stdin` IS the blob checker (verified: correct counts on the
  6-form repro). Ensure the edit-refusal hints name it; add one line to
  the README/cli help (`balance --stdin` = validate a blob without a
  target). No new op.

## Tests
- The exact repro (6-form blob, ]-while-( mismatch): code
  unbalanced-content; stack = full remaining openers with head tokens +
  line:col (no col-0); net-unclosed {paren: N}; no tail offered; hint
  names balance --stdin, never clj_draft.
- Multi-form MISSING-tail blob: unchanged good path (tail + preview +
  claim) — regression.
- not-a-form: construct a genuine arity/count failure (if none exists for
  replace, document which op produces it; keep the code for API
  completeness + patch-into-form misuse).
- Single-form content paths: unchanged behavior (regression).
- Healthy multi-form replace: unchanged (back-compat).
- balance --stdin README/help line.
## Gates
Build, lint gate, cargo test (361 + new), node --check, harness; battery
serial/bounded: intended diffs enumerated (the reclassified refusal cells
+ hints); peak RSS. SPEC §4 (error taxonomy + routing), exit table,
README/help line.
## Report
Path inventory (which content paths existed, which now route through the
walker), taxonomy mapping old→new codes, head-token extraction decision,
test list, gates, battery intended-diff list, peak RSS.
