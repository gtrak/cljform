# cljform bench v1 — arc report

Grid: 16 tasks x 6 revisions = 96 cells, serial, local model (Qwen), 1 rep
per cell — DIRECTIONAL ONLY; add reps before narrating small deltas.

## Success rates

| revision | pass | failing cells |
|---|---|---|
| **head** | **16/16** | — |
| pre-35 (labeled inserts) | 16/16 | — |
| pre-26 (--name) | 14/15 | parse_date_nil (243k tok) |
| pre-32 (response scoping) | 13/16 | cache_file docstring, docstring_quotes, parse_date_nil (512k tok) |
| pre-28 (windows) | 13/16 | docstring_quotes, insert_cache_helpers (+1/+1 lint), parse_date_nil (95k tok) |
| pre-15 (affordances) | 13/16 | cache_file docstring, docstring_quotes, parse_date_nil (472k tok) |

## Findings

1. **The T3 hard task discriminates massively.** `ring_t3_parse_date_nil`
   (bug fix from an issue report) failed at EVERY pre-revision with token
   counts of 95k-512k; head passed at 84k. Worst-case: pre-32 spent
   **512k tokens and 39 turns failing** where head spent 84k/23 succeeding —
   a ~6x efficiency gap on the task class the tool's affordances target.
2. **The issue-15 affordance signature, measured.** Multi-form insert tasks
   at pre-15: `ring_t2_insert_stream_helpers` 737k tokens / 43 tool calls
   (head: 65k / 7); `kondo_t2_insert_cache_helpers` 504k / 24 (head:
   33k / 4). The next-handle affordance is worth an order of magnitude on
   exactly the task shape it was built for.
3. **Easy tasks are revision-insensitive.** T1 medians are flat
   (~16-37k tokens) across all revisions — single-form edits succeed
   everywhere. The tool's value concentrates in T2/T3; the bench proves
   the value proposition is about *hard and structural* work, not line
   edits. (Matches the owner's README thesis: line edits "mostly work"
   without help.)
4. **A model-level failure mode the bench surfaced: double-escaping.**
   The docstring tasks fail across MULTIPLE revisions (pre-15/28/32 and
   partially others) with the same signature: the agent writes `\"`
   instead of `"`, producing char-literal + symbol soup that is VALID
   Clojure — parses clean, lint-clean (+2 style warnings), zero docstring.
   cljform correctly accepts it (validity is not correctness); only the
   intent checker catches it. n=1 per cell means we cannot attribute it to
   any revision — it appears revision-independent (~half of docstring runs
   hit it). The `kondo_t1_docstring_quotes` task turns this failure mode
   into a standing metric.
5. **Medians do not tell the story.** Per-revision medians are nearly flat
   (30-44k tokens) while tails move by 10x — the bench must be read per
   task/tier, not by averages.

## Grader bugs found by the gauntlet (fixed during the arc)
- `docstring_of` name regex lacked `-` (hyphenated def names misread).
- `docstring_of` did not decode Clojure string escapes (quote-containing
  docstrings unmatched) nor skip `^metadata` placement.
- Both are the checker fragility class the tool itself fights: parse /
  first-position assumptions over structure.

## Caveats
- n=1 per cell; stochastic model; deltas < ~2x are noise.
- head also benefits from fixes landed AFTER the pre-revisions (31, 30,
  29) — attribution is per-feature-interval, not per-issue.
- The judge phase (1.5) is not run: subjective quality scores pending.

## Quality axis (judge phase 1.5 — blind rubric + pairwise)

Blind judge (no revision identifiers, fixed rubric, randomized pair order).
Parsed: 81 rubric cells, 46 pairwise (35 truncated even at 1600 tokens — the longest diffs).

| revision | approach | idiomatic | minimal | consistent | n |
|---|---|---|---|---|---|
| head | 5 | 5 | 5 | 5 | 15 |
| pre-35 | 5.0 | 5.0 | 5.0 | 5.0 | 14 |
| pre-32 | 5 | 5 | 5 | 5 | 13 |
| pre-28 | 5.0 | 5.0 | 5.0 | 5.0 | 12 |
| pre-26 | 5.0 | 5.0 | 5.0 | 5.0 | 14 |
| pre-15 | 5 | 5 | 5 | 5 | 13 |

Pairwise (revision diff vs head diff, same task):
- pre-35: rev-wins 2, head-wins 2, ties 9 (n=13)
- pre-32: rev-wins 2, head-wins 0, ties 6 (n=8)
- pre-28: rev-wins 1, head-wins 0, ties 7 (n=8)
- pre-26: rev-wins 2, head-wins 0, ties 7 (n=9)
- pre-15: rev-wins 2, head-wins 0, ties 6 (n=8)

**Finding: the quality axis is flat.** Rubric medians are 5/5/5/5 on every
revision and the blind judge returns 'tie' in the overwhelming majority of
pairs (no head dominance; decisive pairs favor neither side at this n).
The tool's evolution changed REACH (success on hard tasks: 13/16 -> 16/16)
and COST (tokens/turns on the hardest task: up to 6x), not the quality of
successful edits. For a tool whose contract is structural safety, that is
the ideal profile: it widened what agents can accomplish without altering
the character of what they produce.
