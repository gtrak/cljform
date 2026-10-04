# Issue 28 — `tree` line-window: incremental viewing, complete forms only

Motivation: on large files (analyzer.clj: 159 top-level forms, 4744 lines)
the full annotated view is as long as the file. Add a line window so an
agent can page. The invariant: **output contains only COMPLETE forms** —
any form overlapping the window is included in full ("increase the bounds
to allow it"); no partial form, no handle for a fragment.

## CLI: `cljform tree <file> [--start-line S] [--end-line E]`
- 1-based, inclusive-inclusive line bounds; defaults: start=1, end=EOF.
  (Lines, not bytes: they match the line ranges tree already reports and
  are the natural paging unit. Byte offsets deliberately not added — one
  unambiguous way; can revisit later if a real need appears.)
- Selection rule: a form is included iff its [start,end] line span
  INTERSECTS the window. Because spans nest contiguously, the included
  set is contiguous; the effective rendered region is the union — from
  the first included form's start line to the last included form's end
  line — which may be LARGER than the requested window (that is the
  "increase the bounds" case) and is echoed in the output.
- Human output: the annotated view of the effective region only (handles
  inline, full-depth marking of included forms' subtrees per --depth as
  usual), preceded by a header line: `forms in lines S–E (complete forms
  span lines X–Y)` — so the agent knows what expansion happened. With no
  window given, output is byte-identical to today (no header).
- JSON output: the node table FILTERED to included forms (same node
  shape) plus `window: {requested: [S,E], effective: [X,Y]}`. No window:
  unchanged (byte-identical), no window key.
- Composition: composes with --depth (marking cutoff) and --json.
  With --name: the name filter applies first; the window then filters
  matches by their span intersecting the window (report both in the
  echo). If composition gets awkward, ship without --name composition
  and document it — do not half-compose.
- Edge cases: start > end -> usage error (exit 2). Window entirely past
  EOF or in whitespace between forms -> ok:true with zero forms and the
  echo explaining (effective window may be empty). Window fully inside
  one huge form -> that one form, complete, is the output.

## Wrapper: clj_tree gains optional startLine/endLine
Maps to the flags; description/guidelines gain one line: on large files,
page with startLine/endLine — windows expand to complete forms, and the
echo tells you the effective span. Single file (jiti constraint).

## Tests / gates
- CLI tests: window inside one form (expands out), window over several
  forms, boundary-straddling form included in full (its lines extend past
  the window), window past EOF -> zero forms, start>end -> usage, --json
  filtering + window echo, --depth composition, no-window outputs
  byte-identical (human + json, default/--depth/--full).
- Gates: cargo build, clippy -D warnings, cargo test (137 + new) green;
  node --check on the wrapper; hx-style harness for clj_tree window
  behavior (paged view returns complete forms + effective span).
- Byte-identity battery (serial, bounded): old vs new — every scenario
  WITHOUT the new flags byte-identical (the flags are additive); window
  scenarios are new surface.
- SPEC.md: tree op section gains the window flags, the complete-forms
  rule, and the echo semantics.
