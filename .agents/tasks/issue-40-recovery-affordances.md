# Issue 40 — edit feedback recovery affordances (the "obvious wins")
(Session feedback triage: of the 7 items, the two small report-only wins.
House constraints bind: report-only recovery, never auto-retry; verified
facts only; parses-ok ≠ intended structure stays loud.)

## A. Stale-handle recovery report (report-only)
On a stale-handle refusal (edit paths; also get), do NOT auto-retry.
Instead, in the same response: internally re-tree the CURRENT file, find
the same form by identity (def-like by name; otherwise position chain /
structural match), and report in the hint:
- `this form is now ⟦new⟧ (defn helper) — current bytes:` + the exact
  current bytes (the re-get for free — kills the re-tree→re-get loop).
- Patch mode only: if the submitted oldText's token stream equals the
  current form's token stream, add `your oldText differs only in
  whitespace (e.g. a formatter ran) — copy the bytes above`. Token-stream
  equality is the verifiable basis for the claim; if not verifiable, omit
  any whitespace claim (never assert unverifiable facts).
- If no matching form is found (form deleted/restructured), say so
  plainly: `no form matches the previous identity — re-run tree`.
- Multi-form ambiguity: def-likes with duplicate names → report all
  candidates with spans, pick nothing.

## B. Verified-tail claim in missing-tail diagnosis
Where the missing-tail diagnosis fires (content-stage unbalanced-content
hint AND the write-gate enrichment), after computing the mechanical tail,
VERIFY it by an actual in-memory splice + parse of the resulting file
(the real check, not the tautology):
- parses → `verified: with this tail the file parses` + the standing
  loud caveat `placement is yours to verify — parses-ok ≠ intended
  structure (a tail that closes the wrong form early also parses)`.
- does not parse → say so honestly: `with this tail the file still does
  not parse — check for a misplaced closer` (and the mismatch path
  already declines to offer tails).
Never auto-apply; never soften the caveat.

## Tests
A: stale-handle on edit reports new handle + bytes (formatter case:
  reformat the file externally, expect the hint to name the new handle);
  token-stream-equal oldText → whitespace claim; token-stream-different →
  no claim; deleted form → plain message; duplicate names → candidates.
B: verified-claim present on content-stage + write-gate missing-tail;
  honest no-parse branch; caveat string present.
## Gates
Build, lint gate, cargo test (274 + new), node --check, harness; battery:
intended diffs enumerated (stale-handle hints + missing-tail hints only;
everything else byte-identical). SPEC §4.x rows; README only if a tour
example changes.
