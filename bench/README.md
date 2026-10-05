# cljform bench v1

Measures cljform's agent-facing performance across git-history revisions:
the same task corpus runs against pinned revisions; success + efficiency
metrics are compared per revision. See `.agents/tasks/bench-v1-plan.md`
(includes the v1.1 addendum: lint-delta grading, retained diffs, judge phase).

## Execution model (phase-0 decision: Option B — headless)

Each cell = (task, revision, rep) = one **fresh** headless pi session:

```
pi -p --offline -ne -e <rev extension> --session <workdir>/session.jsonl \
   --append-system-prompt <fixed> --        # task prompt on stdin
```

- `-ne` disables ambient extension discovery; the tool surface is exactly
  builtins + the revision's `extension/clojure-forms.ts` (identical across
  revisions — fairness).
- `CLJFORM_BIN=<rev binary>` pins the Rust binary per run (the extension
  resolves `CLJFORM_BIN` first, then PATH).
- The session log carries per-turn `usage` (tokens), tool calls by name
  (assistant content blocks), stop reasons, and tool results (refusals
  parse from the `cljform <code>` error text).
- One wall guard per run: `PI_TIMEOUT_SEC` (1500s) in `run_cell.py`.

## Layout

| path | what |
|---|---|
| `revs.json` | pinned starter SHAs (ring/datascript/clj-kondo) + revision registry (HEAD + pre-N feature checkpoints; `built:false` until phase-2 build) |
| `starter-src/<repo>` | full clones, source of `git archive <sha>` snapshots (gitignored) |
| `tools/clj-kondo` | vendored clj-kondo v2026.08.04 (lint-delta grader) |
| `tasks/<id>.py` | 15 task specs: `SPEC` dict + `done(dest)` + optional `setup(dest)` fixture |
| `metrics.py` | session log -> {tokens, toolCalls, turns, refusals, refetches} |
| `lint_delta.py` | clj-kondo before/after on touched files -> newErrors/newWarnings |
| `check.py` | grader: done + collateral (diff scope) + lint-delta + optional testSuite |
| `run_cell.py` | one cell end-to-end (materialize -> run -> parse -> grade -> store) |
| `report.py` | markdown per revision (success rate, medians, per-task table) |
| `results/<rev>/<task>_r<rep>.json` | cell results — **diff + task statement retained** for the later blind-judge phase; raw session log alongside (`*.session.jsonl`) |
| `work/` | scratch cell dirs (gitignored, deleted after each cell) |

## Cell result (v1.1 grading schema)

```
{task, tier, repo, rev, rep,
 success,
 checks: {done: {pass, detail}, collateral: {pass, changedFiles, detail},
          tests: {run, pass?, wallSec?, detail?}},
 lintDelta: {newErrors, newWarnings, newFindings},
 diff,            # final unified diff vs starter (retained for judging)
 taskStatement,   # the prompt that was sent (for blind judging)
 metrics: {tokens {input,output,cacheRead,cacheWrite,reasoning,total},
           toolCalls {name:count}, turns, refusals {code:count,total}, refetches},
 wall, run: {exitCode, session}}
```

`success` = done.pass AND collateral.pass AND lintDelta.newErrors == 0
AND (tests not run OR tests.pass). New lint warnings are deductions
(score column in the report), not a failure gate.

## Tasks

15 tasks, 3 tiers. T1 = single-form (docstring fix, small body change,
within-form rename); T2 = multi-form structural (insert related defns,
delete form + stale comment, rename across call sites); T3 = hard (bug from
a written issue report, scripted merge conflict, 2-file feature).
Tasks describe INTENT only; checkers are machine assertions (cljform
structural lookups or text checks) + collateral scope + lint delta.
`testSuite` is off by default; on for `ring_t3_parse_date_nil` (behavior
check via clojure CLI; needs network once for the ring-core-protocols jar).

Fixture tasks (`setup()`): `ring_t2_delete_dead_helper` injects a dead form
+ stale comment; `kondo_t3_resolve_conflict` injects conflict markers
(replaces `cache-file`). Fixtures are committed into the "starter" commit,
so the agent diff is measured cleanly.

## Usage

```
python3 bench/run_cell.py --task ring_t1_close_docstring --rev head --rep 1
python3 bench/report.py                       # stdout
python3 bench/report.py --out bench/results/report.md
```

Revision binaries (pre-N) are built in the phase-2 prep step:
`git archive <cljformSha>` -> `bench/revs/<rev>/`, `cargo build --release`,
flip `built` to true in `revs.json`.

## Validation (phase 1)

- End-to-end cell `head / ring_t1_close_docstring` run through `run_cell.py`:
  PASS, 3 turns, 17,509 tokens, 2 tool calls (clj_tree + clj_edit), 0
  refusals, 0 refetches, lint-delta 0e/0w; diff + task statement + raw
  session retained in `results/head/`.
- All 15 checkers verified to FAIL on the unmodified starter (no
  self-passing checkers).
- Metrics schema exercised on two spike sessions (5-turn / 36k-token run and
  a 2-turn hermetic `-ne -e` run); refusal parsing reads the wrapper's
  `cljform <code>` error text from tool results.
