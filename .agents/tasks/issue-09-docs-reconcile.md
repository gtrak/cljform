# Issue 09 — reconcile SPEC/README with the v2 contract

Docs only. Run after issue 06 (so `format` is already documented). No behavior
changes.

## Problem
SPEC.md still presents the v1 edit contract as current in several sections:
`--addr`, `--expect`, `--expect-file`, `--after N`/`--before N`,
`stale-generation`, `stale-form`, the `insert`/`delete` ops, and a
"generation guard". The shipped CLI is handle-only (issues 03/04), and `tree`/
`strip` exist. README was updated by issue 05; verify it has no leftovers.

## Read first
- `SPEC.md` in full (especially §2, §4.1, §4.2, §4.3, §5, §6, §7, §8, §9,
  §10, §12, §13, §14).
- `README.md`.
- `src/main.rs` — the final `Op` enum, `Mode`, exit codes, and error codes, so
  the docs match what is shipped.

## What to fix (match the shipped CLI; do not invent)
- **§2 goals** — replace "generation guard" with content-addressed handle
  resolution (a handle pins position+content; resolution matches or refuses).
- **§4.1 exit codes / error codes** — exit 3 is targeting/refusal:
  `form-not-found`, `ambiguous`, `stale-handle`, `ambiguous-handle`,
  `repair-refused`, `dedent-repair`. Remove `stale-generation`/`stale-form`.
  Add `annotate-conflict` (exit 1) and `stale-handle`/`ambiguous-handle` (3).
- **§4.2 form table** — keep as the top-level listing; note the `tree --json`
  node table is the nested view.
- **§4.3 operations** — rewrite the ops table to the shipped surface:
  `forms`, `tree`, `strip`, `get` (`--name`|`--handle`), `check`, `edit`
  (`--handle`, modes, `--old-text/--new-text`, `--strict`, `--repair`,
  `--dry-run`), `materialize`. Replace the `--expect` paragraph with handle
  resolution: content-addressed, `stale-handle`/`ambiguous-handle`, the
  boundary check, and marker auto-strip on ingest.
- **§6 invariants** — I4 becomes "content-addressed handle resolution"; note
  the boundary check as the I2 extension for nested edits.
- **§7** — drop `--expect-file`/generation references.
- **§8 wrapper spec** — the v2 tools: `clj_tree`, `clj_get` (`name`|`handle`),
  `clj_edit` (`handle`), `clj_forms`, `clj_draft`, guard hook.
- **§9 workflow example** — a `tree` -> copy `⟦handle⟧` -> `edit --handle`
  flow.
- **§12 open questions** — drop the `--expect` questions; keep anything still
  open (e.g. handle length).
- **§11 cross-cutting acceptance** — the dependency list now includes
  `unicode-width` and `unicode-segmentation`, added by issue 06 so `format`
  matches parinfer paren mode on display-width columns and graphemes (the
  differential corpus includes wide chars). State them as accepted.
- **§14** — keep the historical addenda but label the v1-only ones as v1;
  ensure the v2 entry matches what shipped (handles, `tree`/`strip`,
  handle-only `edit`, base-shift reindent, `format`).

## Rules
- Do not change any code or test. Do not restate the full design where a
  cross-reference to §5/§10 suffices.
- Every flag/op/error code you write must exist in `src/main.rs`; grep to
  confirm before writing.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`
(unchanged, run to confirm docs edits touched nothing).

## Report
Sections changed; the grep you used to confirm each flag/op/error code exists;
any spec statement you found that the code contradicts (flag it, do not
silently pick a side).
