# Issue 15 — wrapper honesty + the next-handle affordance

From live agent use (28 clj_edit calls): the worker's #1 friction was
stale-handle refetch chains — three sequential patches in one large form each
cost a fresh `clj_get` — even though every edit already returns the new
handle. And `clj_tree` (default, non-JSON) labels its own successful output
`cljform failed:`.

## 1. clj_tree non-JSON false-failure
`extension/clojure-forms.ts` clj_tree: the non-JSON path runs `tree --human`
(raw annotated text, not a JSON envelope) and then `parseEnvelope(result.stdout)`
— which returns null → `isError: true` with text `cljform failed: ${result.stdout}`
— i.e. the full annotated tree labeled as a failure. Fix: for the non-JSON
call, do not parse an envelope. Success path: exit code 0 → return the
`stdout` text directly (strip BOM/trimEnd as now). Failure path: if stdout
parses as an error envelope, use `errorText(out)`; otherwise use
`result.stderr || result.stdout` with the `cljform failed:` prefix. Keep the
JSON path as is. Verify with a real file: the default clj_tree response must
have `isError` unset/absent and must not contain `cljform failed:`.

## 2. clj_edit next-handle affordance
On every successful clj_edit, append a final line to the tool output:
- replace/patch: `next handle: ⟦H⟧ — use it for the next edit to this form`
  (from `result.summary.handle`; fall back to `wasHandle` only if `handle` is
  absent).
- delete: nothing (the form is gone) — do not print a stale handle.
- insert-after/insert-before: list the inserted forms' handles
  (`result.summary.handles` / `contentForms`) as `inserted handles: ⟦…⟧, ⟦…⟧ — use these for the next edit`, plus the anchor's handle if present.
Source the values from the JSON envelope (do not re-parse the human text).

## 3. Guidance (docs)
- `extension/agents/clojure-worker.md` (and the installed copy at
  `~/.pi/agent/agents/clojure-worker.md` — it is a symlink; check): add a
  line — "for sequential edits to the same form, use the handle returned by
  the previous clj_edit (`next handle:` line); do not re-fetch between
  patches."
- SPEC §8 (wrapper): document the next-handle affordance and the clj_tree
  pass-through fix.

## Gates
`node --experimental-strip-types --check extension/clojure-forms.ts` ·
run the tools end-to-end against the binary (default clj_tree on a fixture
file, clj_edit replace/patch/insert showing the new final lines; simulate
what the agent sees, e.g. with a small node script invoking the registered
tool or by inspecting the constructed output).

## Report
The exact final output strings for replace/patch/insert; proof the default
clj_tree no longer reports isError; the guidance diff.
