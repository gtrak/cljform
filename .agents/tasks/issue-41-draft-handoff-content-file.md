# Issue 41 — draft dual-artifact handoff + content-from-path on edit
(Owner-approved design discussion: the long-form pain is that the form is
TYPED TWICE — once into clj_draft, then again into clj_edit --content —
and the second typing is where a bad tail gets fumbled. The fix is payload
handoff: draft persists its two artifacts; the continuation call is a
reference, not a re-typed payload; corrections happen as cheap deltas on
the artifact.)

## A. Draft dual-artifact handoff
- clj_draft (extension tool, pi-side) currently returns candidate + diff
  text only. Add: the extension ALSO writes both artifacts to the system
  temp dir (NOT the repo):
  `<tmp>/cljform-draft-<n>-sent.clj` (the draft verbatim, exact bytes as
  submitted) and `<tmp>/cljform-draft-<n>-rebalanced.clj` (the completed
  candidate). `<n>` = per-session counter or short random; both paths
  reported in the draft response.
- When inference is a no-op (draft already balanced), write sent only
  (rebalanced == sent; write one file, say so).
- Contract reword (SPEC + tool description): draft never touches the repo
  or any user file; scratch artifacts in the system temp dir are its ONLY
  writes. Do NOT make emission opt-in — the zero-friction handoff is the
  point.
- Composition: the response still carries candidate + diff + (if landed)
  the structural echo; the files are additive, not a replacement.

## B. Content-from-path on edit (first-class, not draft-specific)
- `cljform edit` gains content-from-path sources:
  `--content-file <path>` for replace/insert, `--new-text-file <path>`
  for patch (and `--old-text-file <path>` for patch oldText — same
  pattern, same reasoning; long needles exist).
  - Mutually exclusive with their inline twins (exit 2 on both given);
  `-` reads stdin? NO — stdin is the primary content route already;
    keep files strictly paths, one unambiguous way.
  - File must exist and be valid UTF-8 (exit 4 I/O envelope on failure);
  bytes are taken verbatim (no trimming, no BOM stripping — content is
  content; document if a BOM shows up as a parse error, which is honest).
- The pipeline treats file-sourced content identically to inline: balance
  walk (mechanical tail / mismatch diagnosis), write-gate enrichment,
  verified-tail claim — all fire unchanged. The cheap loop:
  refusal says "missing 2 closers" → the agent edits the artifact file
  (tiny delta) → resubmit --content-file.
- Stale-handle and all gates unchanged (the artifact being stale changes
  nothing: the handle guard refuses loudly).
- The path is echo'd in the response envelope where content provenance is
  shown (additive key, e.g. contentSource: "path:/tmp/…" vs "inline") —
  battery-visible, enumerate intended diffs.

## C. The vote (optional target on draft — 2PC phase-1 promise)
- Framing (owner): draft+commit is two-phase. Phase 1 must carry every
  check doable without the target; abort is always clean (lock-free —
  the repo is never touched, the target stays mutable by others); the
  ONLY residual commit-time failure is the between-phases window
  (target moved → stale-handle), which issue 40A's recovery report
  already turns into a re-vote rather than an abort.
- Draft gains OPTIONAL target params (file + handle, supplied by the
  agent — they have them from tree/get). When given, after writing the
  rebalanced artifact the extension runs the splice dry-run vote via the
  CLI: `cljform edit <file> --handle <h> --mode replace --content-file
  <rebalanced> --dry-run` (no new splice machinery — composition).
- Vote YES → response says `prepared — commit-ready: edit --content-file
  <path>` (the commit is a formality modulo the window). Vote NO → the
  dry-run diagnosis surfaces AT PREPARE TIME (cheapest possible moment:
  fix is a tiny delta on the artifact), with the same recovery affordances
  as any edit refusal.
- The vote is opt-in by supplying a target — no target, artifacts only,
  exactly as in §A. Never auto-commit; the commit call stays the agent's.

## Tests
- B: file-sourced replace/insert/patch (happy path); both inline+file →
  usage error; missing file / invalid UTF-8 → exit 4 envelope; byte
  verbatim (no trim) — a file with trailing newline must behave exactly
  like the same bytes inline; balance/write-gate/verified-tail all fire on
  file-sourced content; contentSource echo.
- A (extension/harness): draft writes both artifacts, paths exist, sent is
  byte-identical to the submitted draft, rebalanced parses; no-op draft
  writes one; response reports both paths; repo untouched.
- Round-trip integration: draft on a long unbalanced form → agent-style
  tiny fix to the rebalanced artifact (append missing closers) →
  edit --content-file succeeds, file verified (parse + untouched forms).
- C: vote yes path (target given, artifact clean → prepared/commit-ready
  message, no write); vote no path (unbalanced artifact → diagnosis at
  prepare time, nothing written to the repo); no-target → no vote;
  between-phases window: target changes after vote → commit hits
  stale-handle → 40A recovery report (integration test).

## Gates
Build, lint gate, cargo test (274 + issue-40 + new), node --check,
harness; battery serial/bounded, intended diffs enumerated (contentSource
echo + draft paths only). SPEC §4.5/content rules + clj_draft row;
README only if a tour example changes.

## Report
Artifact naming/cleanup decisions, flag matrix, test list, gates, battery
intended-diff list.
