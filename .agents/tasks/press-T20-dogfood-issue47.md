# T20 — dogfooding the issue-47 diagnostics on a real repo (kaocha, 106 clj files)

Target: kaocha @ 1b1b025 (clone in /tmp/t20/repo), binary `target/release/cljform`.
Purpose: the three issue-47 fixes (nets line, tail-patch headline, aborted-batch
handles) had only been probed against synthetic fixtures. Verify they hold on a
real codebase, and find what the fixtures missed.

## Cells and outcomes

1. **Tail replacement, one closer short** — `defn test-plan` in
   `src/kaocha/api.clj` (a 6-closer tail), patch dropping one closer.
   - Intended: net line + file-level headline, no phantom "closes nothing".
   - **Held**: `newText has 1 fewer closer(s) … (nets: oldText -5, newText -4)`
     led, the standalone scan was absent.
   - **Defect found (high)**: the *location* in the headline was the raw
     parser's, i.e. `@ line 1 col 1`, while the walker's precise answer
     (`(defn test-plan [config]` at 60:1, swallowing `resolve-reporter`
     (line 69) and `run` (line 97)) sat unused in the JSON `unclosed` key.
     The envelope also printed `(line 1, col 1)`, contradicting the message.
     An agent told "line 1 col 1" would edit the wrong place.
   - **Fixed**: the headline uses issue 44's walker diagnosis (`d.lead()`),
     and the envelope's `line`/`col` are set to the walker's innermost opener
     so message, envelope, and `unclosed[0]` agree.
   - **Defect found (medium)**: the refusal offered only prose advice — the
     verified candidate artifact + one-tap confirm that issue 46 added to the
     MissingTail arm was missing here (the new early return bypassed it).
     **Fixed**: when the file-level walk knows the missing tail and the
     splice verifies, the refusal appends the `newText+tail` preview, writes
     the candidate artifact, and prints the `--new-text-file` reference call.
2. **Net-zero closer reordering** — probe only: replacing
   `(testable/load-testables tests)` with `)testable/load-testables tests(`
   yields a *parse-clean* file with scrambled semantics. The write gate is
   structural by contract ("parses-ok ≠ intended structure") — not a defect,
   recorded so nobody "fixes" it into a semantic checker.
3. **Aborted batch, 3 ops, op 3 unbalanced** — previews printed the
   pre-patch handle `a6dd55` tagged `[pre-patch handle — retry-valid]`, and the
   `affected` rows were aligned to the same handle in this session's follow-up.
   **Retry with the printed handles succeeded** (2 ops, verified line, write).
4. **One-tap confirm round trip** — refused tail patch → artifact
   `/tmp/cljform-edit-…-candidate.clj` containing the balanced replacement →
   `--new-text-file <artifact>` → `verified — 1 changed, 11 untouched ·
   warnings: 0 new (1 pre-existing)` and the intended content on disk.
5. **Patch-by-name** (no handle) — succeeded with the verdict line; the
   freshness guard behaved (no stale resolution).

## Verdict

A competent agent's first retry is right for all five cells. The two defects
found were both *location/affordance* defects in the new tail-patch path — the
class issue 47 was about — and both are fixed with regression tests
(`tests/balance.rs::tail_patch_refusal_names_the_unclosed_form_and_offers_the_candidate`).

Method note: the net-zero probe mutated a real file with semantically wrong
but parse-valid content. That is the documented contract, but it is worth
saying plainly in the docs: the gate certifies STRUCTURE, never intent.

## T21 (adversarial round, local model @ xhigh) — timed out at 30 min

The run explored ~26 probe groups in /tmp/t21 (tail replacements, docstring
parens, CRLF, dry-run, batch target-removed) but never wrote its report; the
findings below are harvested from its transcript.

- **Real (medium, fixed)**: when the verified candidate reproduces oldText
  exactly (the dropped-closer fumble), the new tail-patch affordance printed
  an EMPTY diff block (`--- oldText / +++ newText+tail / <blank>`), while the
  issue-42 MissingTail arm has the "identical to oldText (restoring the
  dropped closer)" line for exactly this case. Ported; regression assertion
  added.
- **Not a defect (checked)**: a claimed double-space around the em dash did
  not reproduce (single spaces live; its pipeline's artifact).
- **Judgment call (not changed)**: a corrupted token whose replacement also
  shifts the closer count reads as "1 more closer(s) … (nets: oldText 0,
  newText -1)" plus the standalone mismatch and the file-level location. The
  net delta IS true; the hint's "add the missing closer(s)" is generic advice
  for a shape that is really a token error. Low severity; recorded, no change.
- **Categories C/D/E: none found** — no tail replacement was told "closes
  nothing" as a lead, the secondary scan never fired with a non-zero net, and
  the batch handles it retried resolved.

Lesson: the brief was too broad for a 30-minute local budget. Next adversarial
round gets 3 categories and an explicit "write the report first, then probe"
ordering.
