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
