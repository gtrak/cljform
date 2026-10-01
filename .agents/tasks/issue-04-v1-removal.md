# Issue 04 — remove the v1 edit surface (handle-only edits)

Design authority: `SPEC.md` §5. Depends on issue 03.

## Deliverable
`edit` addresses forms **only** by `--handle`. `--addr`, `--expect`, `--name`,
`--after N`, `--before N` are removed from `edit`. `--name` stays as a **read**
lookup on `get`. This is a deliberate breaking change.

## Read first
- `SPEC.md` §5.
- `src/main.rs` — `Op::Edit`, `Op::Get`, `run_edit`, `resolve_target`,
  `check_expect`, `Suggestion`.
- every test file: `tests/cli.rs`, `tests/adversarial.rs`, `tests/fuzz.rs`,
  `tests/repair.rs`, `tests/golden.rs`, `tests/regression_swallowed_deftest.rs`.

## CLI changes
- `Op::Edit`: delete the `addr`, `after`, `before`, `name`, `expect` fields and
  all their uses. Target is `--handle H` (already added in 03) for
  replace/patch/delete/insert-before/insert-after. `append`/`prepend` take no
  target.
- Delete `check_expect` and the `--strict` staleness branch that used it.
  `--strict` keeps its other meanings (refuse repairs, refuse detector
  warnings) — check the existing call sites before deleting anything else.
- `resolve_target` shrinks to the `get` read path: `get --name X` (unique
  name; `form-not-found` / `ambiguous`) and `get --handle H`.
- Keep the error codes `form-not-found` / `ambiguous` for the read lookup.
  `stale-handle` / `ambiguous-handle` are the edit errors.

## Read surface
`get` loses `--addr` (numbers are gone), keeps `--name`, and gains
`--handle` (resolve the node via `handle::collect`, print its bytes +
metadata). `forms` unchanged.

`resolve_target` is now the `get` name lookup only; `get --handle` is a
separate branch mirroring the edit resolver.

## Tests
The whole suite currently addresses edits by name/addr. Migrate it. Add a
small helper in each test file that needs it:

```rust
fn handles(file: &str) -> serde_json::Value { /* run `tree --json`, return result.nodes */ }
fn handle_of(file: &str, name_or_path: &str) -> String { /* find node by name (top-level) or path, return .handle */ }
```

Then replace each `edit ... --name X` / `--addr N` / `--after N` / `--before N`
with the handle form. Notes:
- A top-level named form's `get --name X --json` returns `result.hash`, which
  is a valid `--handle` input when the form's content is unique (the handle is
  a prefix of the same blake3). Prefer `tree --json` + `handle_of` so the test
  does not depend on that coincidence.
- `expect_prefix_guard_reaims_and_strict_stops` must be rewritten or deleted:
  `--expect` is gone. Replace it with a handle test that proves the
  content-addressed re-aim (edit a form, then edit a *moved* form by its old
  handle and confirm it still resolves).
- The F1 regression tests edit by name; migrate to handles.

Do not weaken any assertion while migrating — if a test's meaning depends on
name/addr semantics, rewrite it to the handle equivalent and keep the
assertion's strength.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Commands + outcome; the count of migrated call sites; any test whose meaning
had to change (and why); any flag you found that also depended on the removed
fields.
