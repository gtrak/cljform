# Issue 31 — final battery report

Run: serial, every cell through `scripts/bounded-run.sh 2048` (RSS-capped
systemd user scope + `/usr/bin/time -v` readout). Old binary =
`git archive 8e1982c` release build in an isolated target dir; new =
current HEAD release build. Comparison: stdout bytes + exit code per cell
(stderr carries only the `time -v` readout, used for the RSS report).

## Fixtures
- Healthy: fresh, golden (BOM), BOM, CRLF, lookalike (`<<<<<<<<` 8-char,
  NOT a marker), marker-glyphs (`⟦⟧` in strings — annotate-conflict path),
  deep (20k nesting + 200 maps).
- Broken: conf-par (conflicted-parsable, 1 region), conf-multi (2 regions,
  one diff3 with base), conf-unpar (conflicted AND unparsable — layered
  precedence), two-err (2 parse-error spans), resolved-broken (parse
  error only), mid-corrupt (stray `)` mid-file, intact forms both sides).

## Result — 100 compared cells + 21 new-surface cells
- **Healthy: 64/64 byte-identical** (stdout + rc) across forms / check /
  tree (json, human, --full, --depth, --start-line/--end-line) / get
  (--name, --handle) / tree --name / strip / format / edit --dry-run
  (append + handle replace). The conflict scanner is invisible on clean
  input; the additive `error.diagnostics` key never appears.
- **Broken: 33 intended diffs, confined to broken-file states:**
  - conflicted fixtures (6 fixtures × check/forms/tree/edit-dry):
    `ok:true` (old) → `conflict-markers`, exit 1, with
    `error.diagnostics` (per-side spans). Old had ACCEPTED these files —
    the safety hole; now closed on every gated op.
  - conf-unpar: old `parse-error` → new `conflict-markers` (markers win;
    layered precedence).
  - parse-error-only fixtures (two-err, resolved-broken, mid-corrupt):
    same code/exit, envelope gains `error.diagnostics` with the full span
    list (2 spans for two-err; mid-corrupt keeps the old first-error
    message text).
  - strip on conflicted fixtures: old emitted marker lines as bytes →
    new refuses `conflict-markers` (gated by markers, not parse errors).
  - strip on parse-error-only fixtures: byte-identical (strip stays a
    pure byte filter there, by design).
- **New `--recover` surface (21 cells, all exit 0):** json + human +
  windowed-json for all 6 broken fixtures and the healthy fixture.
  Healthy `--recover` == normal view. Human view: verbatim source +
  diagnostics table + intact-forms table, no handles. JSON:
  `{diagnostics, forms, window}` (window only when windowed).
- Old binary given `--recover`: clap unknown-flag usage error (expected;
  documented diff).

## Peak RSS
- Old: 39 MiB · New: 39 MiB (cap 2048 MiB; both far under).

## Verdict
Healthy-file byte-identity holds; every diff maps to a plan-listed
intended change; the safety-hole cells (conflicted-parsable accepted by
old) are the core regression fix.
