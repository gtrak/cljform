# Issue 46 — one-tap confirm, patch-by-name, the decision rule
(Owner-approved from flailing-agent feedback: "the tool is already
better; it just loses the tie on speed for small changes and doesn't
rescue me from my own bracket mistakes fast enough." Three levers, none
touching repair semantics: the issue-12 default stands.)

## A. One-tap confirm: the refusal writes the candidate artifact
- The `unbalanced-content` refusal (content-stage Unbalanced arm, where
  the 39/42 machinery already runs) ALSO writes the inferred candidate to
  the system temp dir, draft's exact contract: `<tmp>/cljform-edit-<n>-
  candidate.clj`, path reported in the hint, repo untouched, emission
  always-on when the candidate exists.
- The hint's confirm instruction becomes a REFERENCE call:
  `one-tap confirm: cljform edit <file> --handle <h> --mode <mode>
  --content-file <candidate-path> --repair` — a payload reference, not a
  re-typed submit (the 41 handoff pattern, extended to the edit-refusal
  path). For patch-mode write-gate refusals the candidate is the repaired
  NEW-text — same artifact treatment when the pipeline can materialize it
  (write-gate enrich has the bytes; write them).
- The candidate artifact is the VERIFIED candidate (the one the
  candidate/preview display already shows — its parse was already proven
  in 42's claim path).
- Stale-confirmation guard: the hint tells the agent the artifact matches
  THIS refusal; if the file moved since, the commit fails stale-handle
  (re-vote), per the standing two-phase contract.

## B. Patch-by-name (`--name` on patch mode ONLY)
- `cljform edit <file> --name <def-like> --mode patch --old-text …
  --new-text …`: resolves the name against the CURRENT tree (exact
  def-like name, same resolver as `clj_get --name`/`tree --name`) →
  handle → patch. SAFE by construction: patch's `oldText` must match
  current bytes, so the freshness guard is the payload itself — a stale
  mental model fails loudly as patch-not-found exactly as with a handle.
- REPLACE and INSERT stay handle-only (no payload freshness guard there;
  replace would bypass the content-addressed staleness check — the
  standing issue-46-design-call). The flag is rejected on those modes
  with a hint naming the safe path (`clj_get --name … then edit by
  handle`).
- Ambiguity: duplicate names → `form-not-found`-style refusal listing
  candidates (pick nothing), same as tree --name semantics.
- The response envelope echoes the resolved handle (additive
  `resolvedHandle` or via the existing summary handle field — battery
  visible, enumerate).
- Patch-NOT-FOUND on a name-resolved patch: the recovery report (40A)
  fires as usual — name → fresh handle → current bytes, closing the
  formatter loop for the by-name path too.

## C. The decision rule (one line, everywhere it matters)
- `new multi-line form → clj_draft (vote included) · small in-form
  change → clj_edit patch (by handle, or --name for top-level def-likes)
  · whole-form swap → clj_edit replace (handle)`
- Placed: README (edit section), the clj_edit + clj_draft tool
  descriptions, the worker agent doc, and — the exit ramp — the GUARD
  HOOK's parse-failure output: when a built-in edit produces a file that
  does not parse, the hook appends `cljform can complete this: use
  clj_edit (repair: true) or clj_draft` instead of only reporting the
  damage. That is the escape hatch for the defaulting-to-edit trap.

## Tests
A: unbalanced inline submit → candidate artifact exists at the hinted
  path, bytes == the shown candidate, one-tap confirm command works
  (content-file + repair → applied, repaired: true); write-gate variant
  (patch new-text) materializes its candidate; repo untouched; healthy
  and mismatch refusals write nothing (mismatch has no candidate).
B: patch-by-name happy path (1 call, patch applies, envelope echo);
  duplicate names → candidates refusal; name not found → suggestions
  (existing resolver behavior); replace/insert + --name → rejected with
  the safe-path hint; name-resolved patch-not-found → 40A recovery
  report; stale-file after resolve → oldText patch-not-found (the guard
  working).
C: string presence in README/descriptions/worker doc; guard-hook
  parse-failure cell appends the escape-hatch line (harness).
## Gates
Build, lint gate, cargo test (361 + new), node --check, harness; battery
serial/bounded: intended diffs enumerated (unbalanced hints + B echo +
patch-by-name cells; everything else byte-identical). SPEC §4 (artifact
contract + --name scope), README, tool descriptions. Report: decisions,
test list, gates, battery intended-diff list, peak RSS.
