# Cleanup C6 — split the big files (pure code motion, zero behavior change)

Read .agents/tasks/press-resource-discipline.md (batteries are SERIAL +
bounded). Gates for every part: cargo build, clippy --all-targets --
-D warnings, cargo test (124) green; byte-identity battery C1-style
(HEAD-from-git-archive vs new; serial; old-binary probes capped at 4096M;
heavy runs bounded; intended diffs = none — this is pure motion).

## Part 1 — src/main.rs (2030 lines) -> entry + modules
Current contents: CLI parsing (clap), dispatch, envelope printing, the
run_* handlers (forms/get/check/materialize/tree/strip/format/edit), the
edit pipeline steps (resolve_edit_target, build_patch_payload,
build_prepared_payload, plan_splice, verify_edit, find_all),
resolve_target/resolve_handle + suggestions, Output/ErrorBody/Fail,
prepare_fail, load_parsed/require_parse/positional_body.
Split proposal (adapt as the code dictates, report the final shape):
- src/cli.rs — clap structs, dispatch, envelope printing, fail_envelope,
  exit codes if they live here
- src/edit.rs — run_edit + its five pipeline steps + find_all
- src/errors.rs (or keep in cli.rs if tightly coupled) — ErrorBody, Fail,
  Output, prepare_fail, positional_body, suggestions
- main.rs — main() + shared read/parse helpers (load_parsed,
  require_parse, read_file, read_content) OR move those to a src/ops.rs
  with the run_* handlers. Decide by dependency direction (modules may
  depend on cli/errors; main depends on all; no cycles).
- src/ops.rs — run_forms/run_get/run_check/run_materialize/run_tree/
  run_strip/run_format if not in edit.rs.
Inline #[cfg(test)] tests move with their code; assertions untouched.

## Part 2 — handle.rs/parser.rs shared table preamble (deferred item)
The form-table preamble duplicated between handle.rs and parser.rs ( scout
finding). Unify into one shared helper if the contracts match; if they
genuinely differ, document why in both module headers and skip.

## Part 3 — extension/clojure-forms.ts (739 lines) — PROBE FIRST
Pi loads the extension as a single file via node --experimental-strip-types.
PROBE: does the loader resolve RELATIVE IMPORTS from the extension file
(build a minimal two-file extension in the /tmp/hx-style harness and
register it)? If YES: split into clojure-forms.ts (entry: registration,
tools) + lib/ modules (runClj/execCljform, envelope parsing, guard hook,
guidelines text) and re-run the C5-style 30-case harness before/after —
byte-identical tool behavior + metadata. If NO (single-file loader):
document that in the task file and skip Part 3 with the reason — do NOT
break the extension to split it.

## Deliverable
Final module map with line counts, battery results (expect ZERO intended
diffs), gates, anything that did not move and why.

## Part 3 — result (probed 2025-10-03, C6)
PROBE DONE — relative imports are NOT usable in the deployed layout; Part 3
SKIPPED, extension/clojure-forms.ts left as a single file (no change).

Probe (hx-style, /tmp/p3probe/): minimal two-file extension
(probeext/entry.ts importing `./lib/probe-lib.ts`, relative, .ts-suffixed)
plus a symlinked entry (symext/entry.ts -> ../probeext/entry.ts), mirroring
the real deployment (~/.pi/agent/extensions/clojure-forms.ts ->
/home/gary/dev/cljform/extension/clojure-forms.ts). Three runs:

1. node --experimental-strip-types, native ESM, via the symlink: PASS —
   relative .ts imports resolve (realpath-based).
2. pi's ACTUAL loader (dist/core/extensions/loader.js uses jiti 2.7
   createJiti(...).import(path, {default:true}), NOT strip-types), via the
   symlink: FAIL — `Cannot find module './lib/probe-lib.ts'`, require stack
   rooted at the symlink path; jiti resolves relative specifiers against
   the entry path as given (symlink dir ~/.pi/agent/extensions), so
   `lib/` would have to exist next to the link, not in the repo.
3. Control: same jiti loader, entry as a REAL file (no symlink): PASS.

Conclusion: pi loads the entry through the global-extensions symlink and
jiti has no symlink-resolution option (no `symlink*` option exists in jiti
2.7), so a repo-side lib/ split cannot load in the deployed layout. Making
it work would require restructuring the user-global extensions dir (shared
with other extensions) or the deployment symlink — out of scope for a
pure-motion task, and would risk breaking the live extension.

If a future pass wants the TS split anyway: the loader change or a
dir-based deployment (extensions/clojure-forms/ with a pi manifest)
would be the decision to make first; the native-ESM path (probe 1) works
today and would make `lib/*.ts` imports with explicit .ts extensions valid.
