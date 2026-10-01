# Issue 03 — `--handle` edit resolution

Design authority: `SPEC.md` §5, §10.3, §10.4. Depends on issue 02
(`src/handle.rs` with `collect`, `annotate`, `strip`; `Node.handle` +
`Node.raw` + `Node.path`).

## Deliverable
`cljform edit <file> --handle H ...` editing any form (top-level or nested),
with the boundary-check invariant and marker auto-strip. The existing
`--addr`/`--name`/`--expect` paths stay untouched (issue 04 removes them).

## Read first
- `src/main.rs` — `Op::Edit`, `run_edit` (the whole function), `resolve_target`,
  `check_expect`, `find_all`, `prepare_fail`.
- `src/splice.rs` — add a range splice (below).
- `src/invariants.rs` — `Allowed`, `verify_untouched`.
- `src/handle.rs` (issue 02).
- `SPEC.md` §10.3/§10.4.

## 1. `splice.rs` — add `Range`

```rust
Splice::Range { start: usize, end: usize, content: Vec<u8> },
```
Apply: `bytes[..start] ++ content ++ bytes[end..]`, applying the same
trailing-comment newline guard as `Splice::Edit` (if `content` ends in a
comment and `end < bytes.len()` and `bytes[end] != b'\n'`, push `b'\n'`).
`start == end` is an insert; `content` empty is a delete.

## 2. `Op::Edit` gains `--handle H: Option<String>`

Resolution (only when `handle` is `Some`; `--handle` conflicts with
`--addr`/`--name`/`--after`/`--before`):
- Reject `H.len() < 6` as a usage error (exit 2).
- `let nodes = handle::collect(&bytes);` match `node.raw.starts_with(H)`
  (so a full 64-hex hash works too).
  - exactly 1 -> target node
  - 0 -> `Fail(3, "stale-handle")`, message names the file and says the form
    changed or is gone, hint `"re-run tree to get current handles"`.
  - >1 -> `Fail(3, "ambiguous-handle")` listing the candidate paths.
- `top_level` = first dot-segment of `node.path` parsed as `usize`.
- `append`/`prepend` with `--handle` -> usage error (they are file-level).

Modes, all over the node's `[start_byte, end_byte)`:
- **replace**: `content::prepare` as today; `Splice::Range { start, end, content }`.
- **patch**: scope `--old-text` to `bytes[start..end]` (reuse the existing
  exactly-once logic and the `patch-not-found`/`patch-ambiguous` errors, with
  the node's line range and exact bytes in the message); build the new node
  bytes; `Splice::Range`.
- **delete**: `Splice::Range { start, end, content: vec![] }`.
- **insert-before**: `Splice::Range { start, end: start, content }`.
- **insert-after**: `Splice::Range { start: end, end, content }`.

Allowed-change window: nested edits never change the top-level form count, so
use `invariants::Allowed::Replace { addr: top_level, n: 1 }` for **all** modes
(replace/patch/delete/insert) on the handle path.

## 3. Boundary check (I2 extension, §10.3)
After `splice::apply`, assert the replacement touched only `[start, end)`:
`new_bytes[..start] == bytes[..start]` and
`new_bytes[start + content_len ..] == bytes[end..]`. On failure ->
`Fail(1, "shape-violation")`, nothing written. (True by construction; this is
the explicit proof.)

## 4. Marker auto-strip on ingest (§10.4)
Run `handle::strip` on `content`, `--old-text`, and `--new-text` before use.
If anything was removed, push the note
`"stripped ⟦…⟧ view markers from the submitted text"`. This makes the `tree`
view copy-pasteable back into an edit.

## 5. New handles in the result
On success, re-run `handle::collect(&new_bytes)` and add to
`result.summary`:
- replace/patch: `"handle": <handle of the node now at the same path>`;
- insert-before/after: `"handles": [<handles of the inserted collection(s)>]`
  (any node whose path lies inside the inserted byte span);
- delete: omit.
Best-effort: if it cannot be computed, omit the field and add a note; never
fail the edit over it.

## 6. Base-shift reindent (default — Rust manages the base indent)
The caller sends an isolated form (any indentation). For whole-form content
(replace and insert; **not** patch), before splicing:
- `target_col` = the target node's start column (0-based byte column in its
  line). For `insert-after`, emit a leading newline and use the target line's
  leading indent, so the new form lands as a sibling.
- Dedent the content by its **common leading whitespace** across non-blank
  lines (so a caller-indented or flat block both normalize).
- Emit line 0 with no leading whitespace (it lands at the splice point, after
  the existing prefix) and every later line prefixed with `target_col` spaces.
- Whitespace-only, preserves the content's relative shape, deterministic. For
  a top-level target `target_col == 0`, so it is a no-op and v1 behavior is
  unchanged.
- Add the note `"reindented submitted content to the target column"` when the
  bytes actually changed.

Do **not** apply parinfer-style reformatting here — style stays with cljfmt
(SPEC §13). The explicit `format` op (issue 06) is where parinfer's rules
live.

## Tests (`tests/handle.rs` or `tests/cli.rs`)
- `edit_handle_replaces_nested_form`: `tree` a fixture, take a nested handle,
  replace it; assert the file changed only there and `result.summary.handle`
  is present.
- `edit_handle_patch_delete_insert`: one nested patch, one nested delete, one
  insert-before, all verified.
- `edit_handle_stale_is_refused`: edit the target out-of-band (so its content
  changes), then `--handle` -> exit 3 `stale-handle`, file unchanged.
- `edit_handle_reaims_moved_form`: content-addressed — a top-level form moved
  by an earlier insert still resolves by its old handle.
- `edit_strips_view_markers_from_content`: pass content containing `⟦…⟧`;
  assert the markers are gone from the file and a note is present.
- `edit_handle_boundary`: assert every top-level form except the containing
  one is byte-identical (reuse the `untouched_forms_byte_identical_after_ops`
  pattern).
- `edit_reindents_isolated_content`: replace a nested node with content at
  column 0; assert the result is indented to the node's column and the
  relative shape is preserved.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
Commands + outcome; the resolution/`Allowed` choice; test names; any place the
nested insert spacing differs from top-level and why.
