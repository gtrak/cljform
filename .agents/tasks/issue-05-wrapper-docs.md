# Issue 05 — pi wrapper + docs

Depends on issues 03 and 04. Design authority: `SPEC.md` §5, §8, §10.

## Deliverable
The pi extension speaks the v2 surface (`handle`, `tree`), and README/SPEC
match the shipped CLI.

## Read first
- `extension/clojure-forms.ts` — the whole file (`clj_forms`, `clj_get`,
  `clj_draft`, `clj_edit`, the guard hook, `resolveBin`, `parseEnvelope`,
  `remember`/`shapeDelta`/`formTableText`, the `before_agent_start` note).
- `README.md`.
- `SPEC.md` §5, §8, §10, §14.
- `src/main.rs` — the final `Op`/flags (after 03/04) so the wrapper matches.

## 1. `extension/clojure-forms.ts`
- **New tool `clj_tree`** — read-only: run `cljform tree <path> --human`
  (annotated source). Optional `depth` (number or `"all"`) → `--depth`; and
  pass `--json` only when a structured node list is explicitly wanted. This
  is the primary way an agent discovers handles; describe it as such.
- **`clj_get`** — add an optional `handle` parameter (alongside `name`). Its
  result line should show the form's **handle** and tell the agent to pass it
  to `clj_edit` via `handle`. Remove the stale "via expect" wording.
- **`clj_edit`** — remove `addr` and `expect`; add `handle`. Drop the
  `after`/`before` numeric anchors. `mode` keeps
  replace/patch/insert-after/insert-before/append/prepend/delete; inserts
  anchor by `handle`. Update `promptGuidelines`:
  - "Read the file with `clj_tree` first; copy a `⟦handle⟧` and pass it as
    `handle` — handles are the only edit target."
  - "Handles are content-addressed: an unchanged form keeps its handle across
    edits elsewhere; if it changed, the edit refuses with `stale-handle` —
    re-run `clj_tree`."
  - "Send content as an isolated form; the tool reindents it to the target."
  - Keep the patch-first guidance and the D1/D2 warning guidance.
- **Guard hook** — keep firing on built-in `edit`/`write` and running
  `cljform check`; it does not need handles. Verify it still compiles/loads.
- Remove any now-dead helper or type that referenced `addr`/`expect`.

## 2. `README.md`
- CLI table: add `tree` (annotated view; `--depth N|all`, `--full`, `--json`)
  and `strip`; add `format` if issue 06 has landed (candidate-first); update
  `edit` to `--handle` (drop `--addr`/`--expect`, note base-shift reindent).
- Exit codes: add `stale-handle`/`ambiguous-handle` (3) and
  `annotate-conflict` (1); remove `stale-form`/`stale-generation` if listed.
- Extension tools list: add `clj_tree`.

## 3. `SPEC.md` §14
Add a short "v2 as built" entry: handle algorithm, `tree`/`strip`, base-shift
reindent, `format` (if landed), and that `--addr`/`--expect` were removed.
Keep it factual and short; do not restate the whole design.

## Tests
The extension has no harness; verify by hand and report the commands:
- `node --experimental-strip-types --check extension/clojure-forms.ts` (or the
  project's TS check) if available; otherwise at minimum confirm the file
  parses with the installed `tsc`/`node`.
- A manual round-trip through the built binary: `tree` → copy a handle →
  `edit --handle` → `tree` again.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`
(the extension change must not break the Rust gates) plus the TS check above.

## Report
Commands + outcome; the new/changed tool parameter list; anything in the
wrapper that still referenced `addr`/`expect`.
