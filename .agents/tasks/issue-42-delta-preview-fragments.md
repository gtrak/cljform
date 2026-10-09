# Issue 42 — delta-aware patch refusals, repair preview, fragment-needle recovery
(Follow-up to the same session's verified-gaps list. Three report-only
fixes + prompt-text truth. House constraints bind: report-only, never
auto-apply; verified facts only; relative delta is the honest signal for
tail patches.)

## A. Relative paren-delta in patch refusals (the sharp one)
Today the balance walk computes newText's ABSOLUTE balance vs zero. For
tail patches newText is inherently closer-heavy, so absolute diagnosis
misleads — the session reproduced "too few" and "too many" closers
printing the IDENTICAL "mismatch: ) closes nothing". Fix: when oldText is
available (patch mode), compute balance(oldText) too and report the
RELATIVE delta alongside the absolute:
- `newText has 1 fewer closer than the text it replaces (absolute: 2
  open, 3 close)` — the relative line LEADS for patch; keep the existing
  absolute/tail/mismatch machinery intact under it.
- Both-signs cases: if relative delta is zero but the absolute walk still
  fails, say so (`deltas balance relative to oldText — the mismatch is
  inside newText itself: <existing diagnosis>`).
- Applies at the write-gate enrichment (patch path) and anywhere the
  patch payload diagnosis renders; single/whole-form content modes keep
  the absolute framing (nothing to be relative to).
- Multi-line delta summarization stays out of scope (per-line depth
  trace remains parked) — the scalar relative delta is the fix.

## B. Patch repair preview (dry-run, verified, never applied)
When the missing-tail diagnosis fires for a patch payload (write gate or
content stage with oldText known):
- Show the CANDIDATE DIFF of newText with the mechanical tail appended at
  its end (the only placement the tool can compute mechanically), as a
  preview block in the refusal: `preview (tail appended at end — verify
  placement): <unified diff of old→new+tail>`.
- The verified-tail claim (40B) rides on the same splice: state it once
  for the previewed candidate.
- Loud caveats unchanged: placement is yours to verify; parses-ok ≠
  intended structure; never auto-applied — the agent still submits.
- If the tail appended at END does not parse (e.g. the true placement is
  mid-form), say so honestly (`tail at end does not parse — the correct
  placement is inside the form; see the mismatch diagnosis`) and show no
  preview.

## C. Stale recovery for fragment needles (token-subsequence containment)
40A recovers when oldText is def-like or covers the whole form; fragment
needles fall to the plain message. Stretch, report-only:
- If the needle's token stream appears as a contiguous run in EXACTLY ONE
  current form's token stream → report that enclosing form: `this form is
  now ⟦H⟧ (head name) — your oldText is a fragment inside it; current
  bytes: <exact bytes>` (whitespace claim per 40A rules iff token streams
  relate appropriately — reuse the existing machinery).
- Multiple containing forms → all candidates with spans, pick nothing.
  Zero → existing plain message.
- Cost guard: containment scan is over forms' token arrays of ONE parse;
  early-exit; keep the error path bounded (deep-tree battery cell must
  stay byte-identical and ~flat in RSS).

## D. Prompt-text truth (extension)
- Tool descriptions still teach the pre-40A loop: "a stale-handle error
  means the form changed — re-run clj_tree, never retry the old handle"
  (and similar at other sites). Update to reflect reality: stale-handle
  refusals now REPORT the recovered form (new handle + bytes) when
  identity is verifiable — read the refusal before re-running tree.
- Add clj_balance to the extension tool surface? DECISION: no new tools
  this issue (bash-hop is the designed fallback; tool lists freeze at
  session start anyway) — note in SPEC that the CLI remains the
  canonical surface.

## Tests
A: relative-delta lead on patch write-gate (fewer-closers case now reads
  distinctly from too-many); zero-relative + absolute-mismatch case;
  content modes unchanged (absolute framing); battery: the
  write-gate-mismatch intended diff now splits into the two
  distinguishable shapes.
B: preview present + verified claim on the previewed candidate;
  end-append-does-not-parse → honest no-preview branch; never applied.
C: fragment needle unique containment → enclosing form reported;
  multi-containment → candidates; zero → plain message; whitespace claim
  rules; deep-tree cost cell byte-identical.
D: description text updated; no schema changes.
## Gates
Build, lint gate, cargo test (299 + new), node --check, harness; battery
serial/bounded, intended diffs enumerated (patch-refusal messages +
preview blocks + D text only). SPEC §4.3/§4.5 rows. Report: decisions,
test list, gates, battery intended-diff list.
