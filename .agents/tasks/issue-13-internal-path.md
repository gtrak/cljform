# Issue 13 — keep the structural path internal

Design authority: `SPEC.md` §5, §10.2, §10.6. The dotted `path` (`9.2.2.5`) is
shown in human/agent output but is not an address — there is no `--path`/`--addr`,
only `--handle`. Showing a coordinate we cannot accept misleads the agent into
pasting it into an edit. Make the path purely internal.

## What to change
1. **`src/handle.rs`** — `Node.path` stays a field (it is the duplicate-folding
   input and the post-edit lookup key) but is no longer serialized: add
   `#[serde(skip)]`. Do not remove it from the struct.
2. **Human summary** (`human_summary` in `src/main.rs`) — the `--handle` branch
   must not print the path. Print the handle plus a semantic label instead:
   - replaced/patched: `{action} form ⟦{handle}⟧ {label} (lines a–b) — C changed, U untouched`
     where `{handle}` is `summary.handle` when present else `summary.wasHandle`,
     and `{label}` is the node's `name`, else `head`, else `kind`.
   - deleted: `deleted form ⟦{wasHandle}⟧ {label} (was lines a–b) — U untouched`.
   - inserted: `inserted form(s) {side} the form ⟦{anchorHandle}⟧ (lines a–b) — U untouched`
     (the anchor node's handle; the inserted handles stay in the JSON).
3. **`build_handle_summary`** (`src/main.rs`) — drop the `"path"` key from every
   summary object. Keep `handle`, `wasHandle`, `handles`, `kind`, `name`, `line`.
   Keep using `node.path` internally to find the node at the same position after
   the edit (`at_path`); that is not exposed.
4. **`ambiguous-handle` error** (`resolve_handle`) — replace the candidate
   `paths` list with the candidate **handles** plus line ranges, e.g.
   `handle "a3f9" matches 2 forms (handles: a3f9c1 lines 4–4, a3f9d2 lines 8–8) — extend the prefix`.
5. **pi wrapper** (`extension/clojure-forms.ts`) — the `clj_tree --json` render
   drops `path ${n.path}`; render `⟦handle⟧ kind [name] · lines a–b`.
6. **Docs** — SPEC §10.2 node-table field list drops `path`; §10.6 wording stays
   (human-facing paths deferred). README: remove any `path` mention from the
   tree/node-table description.

## Tests
- Update `tests/handle.rs` / `tests/edit.rs` assertions that read
  `result.summary.path` to read `handle`/`wasHandle` instead (keep the strength:
  the point was "the same node was targeted", which the handle expresses).
- Add `human_summary_has_no_path`: `edit --handle … --mode patch --human` output
  contains the handle and does not contain a dotted path pattern.
- Add `tree_json_has_no_path`: `tree --json` node objects have no `path` key.
- `ambiguous-handle` test: message lists handles, not paths.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test` ·
`node --experimental-strip-types --check extension/clojure-forms.ts`

## Report
Commands + outcome; the exact new human-summary strings; which tests changed;
confirmation that `handle::collect` still uses the path internally (duplicate
folding + post-edit lookup).
