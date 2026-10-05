# Press-test friction log (running; batch into issues after T12)
## T9 (ring) — 10 edits, 1 refusal, 0 refetches, canonical
- F1 insert summaries: "inserted form(s) after the form ⟦H⟧ (lines a–b)" — the
  span is the INSERTED form's, but reads as the target's. Say "new form at lines".
- F2 `cljform format` on an already-canonical file dumps the full candidate
  (~350 lines) with an empty diff section; worker misread it as "changed
  everything". When unchanged: short "already canonical" line instead.
- F3 patch summary label shows the pre-edit name after a rename (cosmetic).
- F4 trailing-space oldText cost one retry; the patch-not-found refusal printing
  exact form bytes made recovery 1-shot. (Possible hint, must stay non-inferential.)
## T10 (datascript) — 12 edits, 3 refusals (1 planned, 1 spec-error mine, 1 planned), 2 self-inflicted refetches
- F5 edit --human prints the FULL forms table (161 lines on db.cljc) BEFORE the
  one result line; workers tail-ing output miss it → spurious refetches.
  Candidate: result-first ordering or a --quiet mode.
- F6 CONFIRMED NOT A BUG: patch-ambiguous is within-form only (my T10 spec was
  wrong); handle-less patch is a usage error by design (handle is the only
  target). Verified: duplicate oldText inside one form -> patch-ambiguous.
- F7 delete seam: deleting a form preceded by a comment left a double blank
  line (comment survived, correct). Candidate: collapse the introduced blank run.
- F8 --repair placed the inferred closer exactly where the worker would
  (end of last submitted line) — candidate bytes == intent, byte-for-byte. Win.
- F9 minor: handle-less patch usage error could hint "or pass --handle from tree".
## External adoption (other mission, W21 workers, real worktree edits)
- 7 clj_edit ops on real deliverable .clj files (mode=delete, insert-before,
  4x patch from clj_get bytes): zero stale-handle, handles valid throughout.
- Worker initially fled to python after the BUILT-IN edit tool gave spurious
  "Could not find exact text" (od-matched bytes) on a scratch script — same
  invisible-byte failure class as F4, but without cljform's exact-bytes
  refusal affordance. Self-corrected to clj_edit for .clj even in /tmp;
  shape report caught its bracket bugs faster than manual counting.
- Implication: F4-class hint (exact bytes on mismatch) is the load-bearing
  affordance; it is what converts byte-mismatch retries into 1-shot recovery.
## T11 (clj-kondo) — 16 edits, 0 refusals, 0 in-chain refetches, sub-200ms at 4744 lines
- F10 insert-after/insert-before return empty result.diff (replace/patch
  populate it). Reporting gap: the edit summary is the only seam visibility.
- F11 check D2 warning noise on data-structure-of-defs files (21 warnings,
  ok:true) — consider a --quiet or warnings summary line.
- Confirmed pre-existing format non-emptiness on clj-kondo source (parinfer
  +1 vs cljfmt +2) — worker proved via pristine-vs-post candidate diff;
  edits added zero reflow. Not a cljform regression.
- #_ discards: invisible to tree/get (no handle), bytes preserved — correct.
- Two-forms-on-one-line insert-after: split cleanly, sibling form's content
  hash unchanged. Correct seam behavior.
## Older-build findings re-tested against current build (user report, pre-T12)
Both reported silent-corruption modes DO NOT reproduce (repros in /tmp/repro17):
- "Reindent moved an if-let else-branch out to defn level": NOT reproducible.
  Lying indentation (else at col 2 under a col-2 if-let) -> structure preserved,
  else still inside (get's contains: if-let:1); only an alignment nudge. Mixed
  tabs/spaces -> normalized to spaces, structure correct. Col-0 closer ->
  pulled up to the last content line, same form structure. Credible fixes:
  paren-mode-only reindent (paren mode never inserts closers mid-line, so it
  cannot relocate a branch), plus the format_preserves_tokens gate (closers
  may only move earlier) and I1-I6 post-splice verification.
- "Multi-form insert-after silently dropped a trailing form": NOT reproducible.
  4-form insert (comment + 3 defs): all inserted, count in table; no-trailing-
  newline variant: both forms inserted. contentForms count is visible in the
  JSON for sent-vs-written comparison.
- The reporter's defensive rules (re-get to verify after deep inserts;
  autoFormat:false for pre-balanced content) remain sensible belt-and-braces,
  and the next-handle affordance makes the re-get cheap. Kept as guidance,
  not requirements.
## T12 (break-it hunt) — ~2400 invocations. A/B/D/E clean; C-1, F-1, F-2, F-3, P-1 CONFIRMED by me
- All five repros re-run independently: C-1 (od shows mixed endings; format
  diff real — my first grep was wrong: diff is embedded in single-line JSON),
  F-1 (head "defn" vs forms kind "let"), F-2 (materialize-error on a string
  interior ")"), F-3 (pure content hash across dups -> lying stale-handle),
  P-1 (35.8s/39.2s at 50k depth).
- Core guarantees HELD: never-unparsable (A), untouched-form integrity (B),
  repair never writes wrong structure (D), exit codes/envelopes/no panics (E)
  — including 1500 fuzz drafts + independent Python reader validator.
- Filed: issue-18 (C-1), issue-19 (F-1), issue-20 (F-2), issue-21 (P-1).
  F-3 recorded as minor (message wording on an unreachable-by-tree path).
## T13 (re-break of fixed build) — 5/5 old repros PASS; 3 new findings (S1/M1/C1, all confirmed by me)
- Fix validation: C-1/F-1/F-2/F-3/P-1 all hold under direct re-attack;
  detector fuzz (3700 docs, old vs new) zero divergence; 3200-edit battery:
  0 crashes, 0 unparsable writes, 0 untouched-form corruption.
- S1/M1/C1 filed as issues 23/24/25. Note C1's EOF-newline geometry quirk.
- UX: nested-handle discovery requires python one-liners over tree JSON
  (no path, index-based); a --name-addressed nested delete or stable
  nested addressing would remove recurring friction. printf escaping for
  byte-exact content error-prone; content-file + documented exact-bytes
  recipe preferred.
## T14 (dogfood of tree --name + two-case rule, ring) — 14 edits, 0 refusals, 0 refetches
- VERDICT: two-case rule sufficient as discovery/address guidance; --name
  fully removed the python-one-liner step for named nested forms; patch
  scoping + exact-bytes refusal made the anonymous branch safe.
- F12 (twice-reported now, T11 F10 + here): whole-form ops (replace/
  insert/delete) emit empty result.diff while patch shows a unified diff —
  worker reached for git diff to confirm a replace. Candidate fix: populate
  the diff for whole-form ops (old form bytes vs new form bytes are both
  in hand at summary time).
- F13: delete of the last form at EOF leaves a trailing blank line and
  format calls it canonical (parinfer does not trim EOF blanks). Cosmetic;
  related to T10's double-blank seam note.
- F14 (minor): --name blocks render the matched subtree only, not the
  enclosing form; depth labels are collection depth, not def-nesting level.
- Observation: D2 nesting detector did not flag a defn inside an
  extend-protocol method body (did flag one inside a let) — recorded as
  data, plausibly intended host scoping.
## T15 (dogfood of issues 26/27/28 post-T14 changes, clj-kondo) — 8 edits, 1 refusal, 0 refetches, 0 git-diff usage
- ALL FOUR CHANGES ADOPTED: paging (3 windows on analyzer.clj, true-line
  labels correlated against sed), --name enclosing context (enclosing-form
  edit without a second tree call — the T14 "impossible" move), whole-form
  diffs (all verification from the tool's own hunk + header), EOF trim
  (delete+append restore byte-identical, clean single-\n tail).
- F15 comment asymmetry: insert of comments-only content is refused
  (not-one-form) while comment FORMS are first-class table entries
  deletable/editable. Trivial workaround (wrap in (comment ...)) but the
  asymmetry deserves either a documented exception or support.
- F16 edit envelope volume on big files: full 159-line forms table prints
  before the diff (echoes T10 F5 result-buried). Candidate: --quiet or
  result-first ordering.
- F17 get --name --json returns the full forms array beside the single
  result — heavy for routine lookup on big files.
- F18 window expansion math: a 100-line window rendered 252 lines
  (correct complete-forms semantics; document that page-of-N != N lines
  of output; step from the EFFECTIVE end when paging).
- F15 RESOLUTION (user question, verified): comments ARE insertable via
  patch in both placements — inside a form (oldText anchored at an inner
  line) and between top-level forms (patch the neighbor with newText =
  ";; note\n" + the form's first bytes; the comment becomes seam bytes,
  summary reports the def's own span). The insert-mode refusal is coherent:
  insert builds a forms table and a comment is not a form (no handle to
  return). Guidance line if we want it: "comment -> patch the anchor form,
  include the comment in new-text; insert modes splice forms only."
## T16 (dogfood of issue-31 recovery, REAL git conflicts on clj-kondo) — ALL FOUR PHASES PASS
- Gate: every op refused conflicted files with structured region spans
  matching git's marker lines exactly; safety hole (edit on one conflict
  side) demonstrably closed.
- View: per-side head/base/incoming spans matched git's diff3 layout
  line-for-line on a 4766-line file; intact-form count = baseline minus
  conflict-hosting defns; parse-error spans pre-announced the deliberate
  trap. Verdict: "I could plan the entire 3-region resolution from the
  view plus two 25-line source slices, never dumping the whole file."
- Progressive: 2->1 with correctly re-based spans; count survived edits
  made by the EXTERNAL (built-in) edit tool; layer flip conflict-markers
  -> parse-error verified with a deliberately-wrong resolution.
- Resume: real handles returned, cljform patch landed cleanly, loop closed.
- CORPUS CAVEAT (important for future dogfoods): "format empty-diff" is
  NOT a valid acceptance bar on non-parinfer-formatted upstream code —
  worker proved the diff pre-exists on the pristine baseline. The
  meaningful invariants: format runs, token stream verified, nothing
  written.
- Minor: bounded-run prints the timed command on stderr (jq-piping patterns
  can drop it); --recover does not compose with --name (documented).
- F16/F17/T9-F2 (response scoping family) -> issue 32 in flight: edit --human
  result-first with affected-row + counts (no whole-file table), get
  payload-first + formsCount (edit envelope's forms array STAYS — it is the
  cross-check that caught issue 19's stale-head bug), format unchanged-case
  one-liner, related-surface audit mandated.
