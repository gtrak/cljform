# Issue 33 — clj_tree verbosity: depth-honoring JSON + large-file index default

Two parts. Part A is a CLI contract fix; Part B is the wrapper's large-file
default (owner decision: "by default don't return a huge response without
opt-in — sensible threshold; a compressed representation could work").
Base: current main, 194 tests. QUEUED behind issue 32 (one writer; both
touch ops.rs and the wrapper).

## Part A — honor `--depth` in JSON (CLI contract fix, small)
Today `tree --json --depth 1` returns the FULL node table (6294 nodes on
analyzer.clj, ~500 KB) — depth gates only the human marking. Fix: the JSON
node table filters to `depth <= N` when `--depth N` is given. No flag and
`--depth all` stay byte-identical (unconditional full table). Intended
diffs: exactly the `--depth N --json` cells (previously ignored). Check
test consumers: tests/common `tree_full_nodes` uses plain `--json`
(unaffected); audit any `--depth N --json` uses.

## Part B — clj_tree large-file default: the compressed form index
Owner decision: never return a huge response without opt-in; the
compressed representation is the **form index** (a table of contents), not
a truncated slice.

Behavior (wrapper, clj_tree, human/agent path):
- If the file's line count exceeds a threshold (pick ~300 lines; document)
  AND the caller gave no `startLine`/`endLine`/`name`/`json`/`depth`
  overrides (i.e., the call would dump the whole annotated view): return
  the INDEX instead:
  - header: `large file: 4744 lines, 159 top-level forms — showing the
    form index (handles are live). Annotated view: pass startLine/endLine;
    a specific form: name.`
  - one row per top-level form, same style as the --name blocks:
    `⟦handle⟧ head name (lines X–Y)`, sorted by line — real, immediately
    usable handles (fetch the data from `tree --json --depth 1`, which
    Part A makes a small payload).
  - NO source body.
- Explicit calls are honored exactly: `startLine`/`endLine` -> the windowed
  annotated view (the agent can cover the whole file with a full-span
  window if they truly want it); `name` -> selector view; `json` -> node
  table (machine surface stays exact, uncapped); `depth` -> honored.
- Below the threshold: byte-identical to today's pass-through.
- The threshold is a wrapper constant; document it. Do NOT change the CLI's
  own default `tree` contract (the CLI stays the honest primitive; the
  wrapper owns agent ergonomics).

## Rationale (for SPEC/README)
The annotated view scales with file size; the index scales with form count.
For discovery the index is strictly better on large files (whole-file shape
at a glance, every row a live handle); for reading a region the window is
strictly better (bounded, true-line labels). The default should pick the
right tool instead of the biggest dump.

## Tests / gates
- Part A: `--depth N --json` filtered (N=1 on the multi-depth fixture;
  N beyond max depth == all); no-flag and `--depth all --json`
  byte-identical; windowed JSON unaffected.
- Part B (hx harness, real extension + binary): large fixture (> threshold)
  default call -> index rows with live handles + hint, no source; small
  file -> byte-identical pass-through; explicit window/name/json honored;
  index handles actually work as edit targets (edit by an index handle).
- Gates: build, clippy (issue-30 gate), cargo test (194 + new), node
  --check; battery (serial, bounded): CLI no-flag healthy scenarios
  byte-identical; `--depth N --json` cells = intended diffs; wrapper cells
  per harness. SPEC §4.3/§8/§10.2 + README (clj_tree row + a line in the
  tour: "large files return the form index; page with startLine/endLine").

## Report
Part A diff + consumer audit; Part B threshold + rendering + harness cells
(incl. edit-by-index-handle proof); gates; battery intended-diff list.
