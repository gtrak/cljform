# Bench v1 — measuring cljform's agent-facing performance across git history

Owner proposal: a corpus of tasks + starter code as a fixed baseline; run
the same tasks against multiple revisions of cljform; track success +
efficiency metrics; attribute deltas to the changes between revisions.

## Why this can work
The tool's changes are feature-attributable commits (15: next-handle
affordances; 26: --name; 28: windows; 32: response scoping; 33: index
default; 34/35: rendering; 31: recovery). Revisions between them are
natural experiment points. Prior dogfoods produced ANECDOTAL versions of
these deltas ("refetches went 3-4x per form -> 1 in 8"); the bench makes
them counted.

## Metrics (per run)
- success (graded: intent landed, file parses, no collateral diff) — PRIMARY
- context usage: total tokens from the run's session log (minimize)
- tool calls: count of clj_* / edit calls by name (minimize)
- turns (minimize)
- rework signals: refusal count by code, stale-handle events, refetches
  (discovery calls re-deriving known handles), retry loops
- wall clock (secondary; model-server noise)

## Corpus (target: 12-16 tasks, 3 difficulty tiers)
On pinned starter repos (ring, datascript, clj-kondo at fixed SHAs):
- T1-tier (single-form): docstring fix, small body change, rename within a
  form — checker: parse-ok + AST/text assertion of the intended change.
- T2-tier (multi-form / structural): insert 3 related defns; delete a form
  and its stale comment; rename a defn across its call sites.
- T3-tier (hard): fix a bug from a written issue report (intent only);
  resolve a scripted merge conflict; add a feature touching 2 files.
Checker design rule: every task gets a machine-checkable done() (grep or
cljform-based structural assertion) + a collateral check (git diff scope).
Tasks describe INTENT, never the diff (starter repos are fresh clones at
pinned SHAs; no solution leakage).

## Revisions (feature-attributable checkpoints, ~6)
HEAD; pre-35 (labeled inserts); pre-32 (response scoping); pre-28
(window); pre-26 (--name); pre-15 (affordances). Build each via
`git archive <sha>` (binary AND wrapper extension from that revision —
several features are wrapper-side; holding the wrapper at HEAD would
hide 33/34/35).

## Execution model (the feasibility question)
Each cell = (task, revision) = one FRESH agent session (metrics are
per-session; contamination forbidden). Two options:
- Option A (orchestrated): I dispatch one subagent per cell; parse the
  session file for metrics; run the checker. Autoplay with my turns.
- Option B (headless): if `pi -ne` (or equivalent headless mode) can load
  extensions from ~/.pi/agent, a coordinator worker runs all cells
  autonomously: for each cell, swap the extension symlink to the revision's
  copy, set CLJFORM_BIN to the revision binary, run pi headless with the
  task, parse that run's session log, grade, append results.
Phase 0 decides: a feasibility spike for Option B (headless + extension +
per-run env + metrics extraction). If B works, the arc automates; else A.

## Statistics (honest constraints)
The local model is stochastic; n=1 per cell is noise. Plan: 1 rep for the
first full pass (directional deltas only), then add reps (2-3) on the
cells where deltas look interesting. Attribute ONLY large effects
(e.g. the T8-measured 3-4x refetch reduction should show up; a 5% token
delta should not be narrated). Serial execution (resource discipline);
grid cost at 6 revs x 14 tasks x 1 rep ~= 84 runs x 5-15 min ~= 7-21 h —
a background arc across sessions, batched.

## Deliverables
- bench/ in-repo: task specs + checkers, runner script, results store
  (bench/results/<rev>/<task>.json), a report generator (markdown table
  per revision: success rate, median tokens/tool-calls/turns/refusals).
- The arc report: per-feature delta narrative backed by the table.

## Phases
0. Feasibility spike (Option B): headless pi + extension + per-run env +
   metrics extraction from session logs. Fallback: Option A.
1. Corpus + checkers + runner + report generator (worker task).
2. Pilot: 2 tasks x 2 revisions (HEAD vs pre-32) — validates measurement
   end-to-end; sanity: does the pilot "see" the known response-scoping
   delta?
3. Full grid, batched; then reps on interesting cells.
4. Arc report.

## Out of scope (v1)
- Other models (local only — that is the dogfood-parity condition).
- Tasks requiring domain judgment beyond code edits (no "design an API").
- Wall-clock SLOs (model-server load noise dwarfs tool effects).

## v1.1 additions (owner): correctness depth + subjective quality

### Correctness, tier 2 — lint delta (the corpus grades the edits)
The starter repos are Clojure; clj-kondo itself becomes the correctness
instrument. Per cell, on the touched files: run clj-kondo before (starter)
and after (result); NEW warnings/errors introduced by the edit are
deductions (new warning = -1 per instance, new error = fail); warnings
that disappear are neutral. Vendored clj-kondo release binary once into
bench/tools/ (network OK; java 25 + clojure CLI 1.12.5 present on the box;
no clj-kondo on PATH). This catches what parse-ok cannot: unresolved
symbols, wrong arity, unused vars introduced by an edit that "looks done".

### Correctness, tier 3 — test suites (optional, per-task flag)
ring/datascript carry test suites; tasks MAY declare a test command
(clojure CLI present). Off by default (suite noise/speed); on for tasks
where the done() assertion is too weak.

### Subjective quality — blind LLM judge
Every cell stores its final unified diff + the task statement. A separate
fresh judge session (same local model, blind to which revision produced
the diff) scores each diff on a rubric:
- approach correctness (does the diff actually implement the intent)
- idiomaticity (would a Clojure reviewer approve)
- minimality (smallest reasonable change)
- consistency with the surrounding code's conventions
1-5 each + one-line rationale. Aggregate per revision: median rubric
scores + win-rate via PAIRWISE blind comparisons (revision diff vs HEAD
diff for the same task — pairwise is more reliable than absolute for LLM
judges; run head-referenced pairs, extend to round-robin on interesting
cells). Self-preference bias is bounded by blindness (the judge does not
know the revision) and by the fixed rubric.

### Grading schema update
Cell result = {success, checks: {...}, lintDelta: {newErrors, newWarnings},
diff (stored for judging), metrics: {tokens, toolCalls, turns, refusals,
refetches}, wall}. The judge phase reads stored diffs — cells must retain
them.
