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
